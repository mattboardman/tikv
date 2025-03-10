// Copyright 2023 TiKV Project Authors. Licensed under Apache-2.0.

use engine_traits::{
    KvEngine, Peekable, RangeCacheEngine, ReadOptions, Result, SnapshotContext, SnapshotMiscExt,
    SyncMutable, WriteBatchExt,
};

use crate::snapshot::HybridEngineSnapshot;

/// This engine is structured with both a disk engine and an region cache
/// engine. The disk engine houses the complete database data, whereas the
/// region cache engine functions as a region cache, selectively caching certain
/// regions (in a better performance storage device such as NVME or RAM) to
/// enhance read performance. For the regions that are cached, region cache
/// engine retains all data that has not been garbage collected.
#[derive(Clone, Debug)]
pub struct HybridEngine<EK, EC>
where
    EK: KvEngine,
    EC: RangeCacheEngine,
{
    disk_engine: EK,
    region_cache_engine: EC,
}

impl<EK, EC> HybridEngine<EK, EC>
where
    EK: KvEngine,
    EC: RangeCacheEngine,
{
    pub fn disk_engine(&self) -> &EK {
        &self.disk_engine
    }

    pub fn mut_disk_engine(&mut self) -> &mut EK {
        &mut self.disk_engine
    }

    pub fn region_cache_engine(&self) -> &EC {
        &self.region_cache_engine
    }

    pub fn mut_region_cache_engine(&mut self) -> &mut EC {
        &mut self.region_cache_engine
    }
}

impl<EK, EC> HybridEngine<EK, EC>
where
    EK: KvEngine,
    EC: RangeCacheEngine,
{
    pub fn new(disk_engine: EK, region_cache_engine: EC) -> Self {
        Self {
            disk_engine,
            region_cache_engine,
        }
    }
}

// todo: implement KvEngine methods as well as it's super traits.
impl<EK, EC> KvEngine for HybridEngine<EK, EC>
where
    EK: KvEngine,
    EC: RangeCacheEngine,
    HybridEngine<EK, EC>: WriteBatchExt,
{
    type Snapshot = HybridEngineSnapshot<EK, EC>;

    fn snapshot(&self, ctx: Option<SnapshotContext>) -> Self::Snapshot {
        let disk_snap = self.disk_engine.snapshot(ctx.clone());
        let region_cache_snap = if let Some(ctx) = ctx {
            self.region_cache_engine.snapshot(
                ctx.range.unwrap(),
                ctx.read_ts,
                disk_snap.sequence_number(),
            )
        } else {
            None
        };
        HybridEngineSnapshot::new(disk_snap, region_cache_snap)
    }

    fn sync(&self) -> engine_traits::Result<()> {
        self.disk_engine.sync()
    }

    fn bad_downcast<T: 'static>(&self) -> &T {
        self.disk_engine.bad_downcast()
    }

    #[cfg(feature = "testexport")]
    fn inner_refcount(&self) -> usize {
        self.disk_engine.inner_refcount()
    }
}

impl<EK, EC> Peekable for HybridEngine<EK, EC>
where
    EK: KvEngine,
    EC: RangeCacheEngine,
{
    type DbVector = EK::DbVector;

    // region cache engine only supports peekable trait in the snapshot of it
    fn get_value_opt(&self, opts: &ReadOptions, key: &[u8]) -> Result<Option<Self::DbVector>> {
        self.disk_engine.get_value_opt(opts, key)
    }

    // region cache engine only supports peekable trait in the snapshot of it
    fn get_value_cf_opt(
        &self,
        opts: &ReadOptions,
        cf: &str,
        key: &[u8],
    ) -> Result<Option<Self::DbVector>> {
        self.disk_engine.get_value_cf_opt(opts, cf, key)
    }
}

impl<EK, EC> SyncMutable for HybridEngine<EK, EC>
where
    EK: KvEngine,
    EC: RangeCacheEngine,
{
    fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        unimplemented!()
    }

    fn put_cf(&self, cf: &str, key: &[u8], value: &[u8]) -> Result<()> {
        unimplemented!()
    }

    fn delete(&self, key: &[u8]) -> Result<()> {
        unimplemented!()
    }

    fn delete_cf(&self, cf: &str, key: &[u8]) -> Result<()> {
        unimplemented!()
    }

    fn delete_range(&self, begin_key: &[u8], end_key: &[u8]) -> Result<()> {
        unimplemented!()
    }

    fn delete_range_cf(&self, cf: &str, begin_key: &[u8], end_key: &[u8]) -> Result<()> {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use engine_rocks::util::new_engine;
    use engine_traits::{CacheRange, KvEngine, SnapshotContext, CF_DEFAULT, CF_LOCK, CF_WRITE};
    use region_cache_memory_engine::RangeCacheMemoryEngine;
    use tempfile::Builder;

    use crate::HybridEngine;

    #[test]
    fn test_engine() {
        let path = Builder::new().prefix("temp").tempdir().unwrap();
        let disk_engine = new_engine(
            path.path().to_str().unwrap(),
            &[CF_DEFAULT, CF_LOCK, CF_WRITE],
        )
        .unwrap();
        let memory_engine = RangeCacheMemoryEngine::new(Arc::default());
        let range = CacheRange::new(b"k00".to_vec(), b"k10".to_vec());
        memory_engine.new_range(range.clone());
        {
            let mut core = memory_engine.core().lock().unwrap();
            core.mut_range_manager().set_range_readable(&range, true);
            core.mut_range_manager().set_safe_ts(&range, 10);
        }

        let hybrid_engine = HybridEngine::new(disk_engine, memory_engine.clone());
        let s = hybrid_engine.snapshot(None);
        assert!(!s.region_cache_snapshot_available());

        let mut snap_ctx = SnapshotContext {
            read_ts: 15,
            range: Some(range.clone()),
        };
        let s = hybrid_engine.snapshot(Some(snap_ctx.clone()));
        assert!(s.region_cache_snapshot_available());

        {
            let mut core = memory_engine.core().lock().unwrap();
            core.mut_range_manager().set_range_readable(&range, false);
        }
        let s = hybrid_engine.snapshot(Some(snap_ctx.clone()));
        assert!(!s.region_cache_snapshot_available());

        {
            let mut core = memory_engine.core().lock().unwrap();
            core.mut_range_manager().set_range_readable(&range, true);
        }
        snap_ctx.read_ts = 5;
        let s = hybrid_engine.snapshot(Some(snap_ctx));
        assert!(!s.region_cache_snapshot_available());
    }
}
