use martin_tile_utils::{Format, TileCoord};

use crate::cache::{CacheKey, Cacheable, ResourceCache};
use crate::tiles::Tile;

/// Tile cache for storing rendered tile data, keyed by source ID and epoch,
/// tile coordinate, query string, and `Accept`-driven output format.
pub type TileCache = ResourceCache<TileCacheKey, Tile>;

/// Optional wrapper for [`TileCache`].
pub type OptTileCache = Option<TileCache>;

/// Constant representing no tile cache configuration.
pub const NO_TILE_CACHE: OptTileCache = None;

/// Cache key for a rendered tile.
///
/// Source-based invalidation matches exactly on `source_id` (each tile
/// belongs to one source) and so covers every `source_epoch`. Metric
/// recording adds a `zoom` dimension on top of the standard cache/hit labels.
#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub struct TileCacheKey {
    source_id: String,
    /// [`Source::cache_epoch`](crate::tiles::Source::cache_epoch) of the
    /// instance that produced the tile.
    source_epoch: u64,
    xyz: TileCoord,
    query: Option<String>,
    /// Format requested via the `Accept` header; `None` if absent.
    format: Option<Format>,
}

impl TileCacheKey {
    /// Build a key from the source instance and the request fields.
    #[must_use]
    pub fn new(
        source_id: String,
        source_epoch: u64,
        xyz: TileCoord,
        query: Option<String>,
        format: Option<Format>,
    ) -> Self {
        Self {
            source_id,
            source_epoch,
            xyz,
            query,
            format,
        }
    }
}

impl CacheKey for TileCacheKey {
    const CACHE_NAME: &'static str = "tile";

    fn matches_source(&self, source_id: &str) -> bool {
        self.source_id == source_id
    }

    fn record_outcome(&self, hit: bool) {
        #[cfg(feature = "metrics")]
        crate::metrics::TILE_CACHE_REQUESTS_TOTAL
            .with_label_values(&[
                Self::CACHE_NAME,
                crate::cache::hit_miss_label(hit),
                crate::metrics::ZOOM_LABELS[self.xyz.z as usize],
            ])
            .inc();
        #[expect(
            clippy::if_same_then_else,
            reason = "hotpath::gauge! requires a literal name argument"
        )]
        if hit {
            hotpath::gauge!("tile_cache_hits").inc(1.0);
        } else {
            hotpath::gauge!("tile_cache_misses").inc(1.0);
        }
    }
}

impl Cacheable for Tile {
    fn weight(&self) -> u32 {
        self.data.len().try_into().unwrap_or(u32::MAX)
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use martin_tile_utils::{Encoding, TileInfo};

    use super::*;

    fn key(source_id: &str, source_epoch: u64) -> TileCacheKey {
        let xyz = TileCoord { z: 0, x: 0, y: 0 };
        TileCacheKey::new(source_id.to_string(), source_epoch, xyz, None, None)
    }

    /// Returns what the cache holds for `key` after offering it `data`.
    async fn get_or_insert(cache: &TileCache, key: TileCacheKey, data: &[u8]) -> Vec<u8> {
        let info = TileInfo::new(Format::Mvt, Encoding::Uncompressed);
        let tile = Tile::new_hash_etag(data.to_vec(), info);
        cache
            .get_or_insert(key, || async { Ok::<_, Infallible>(tile) })
            .await
            .expect("an infallible compute succeeds")
            .data
    }

    #[tokio::test]
    async fn source_epochs_do_not_share_entries() {
        let cache = TileCache::new(1_000_000, None, None);

        assert_eq!(get_or_insert(&cache, key("src", 1), b"old").await, b"old");
        assert_eq!(get_or_insert(&cache, key("src", 2), b"new").await, b"new");
        assert_eq!(get_or_insert(&cache, key("src", 1), b"other").await, b"old");
    }

    #[tokio::test]
    async fn invalidating_a_source_purges_every_epoch() {
        let cache = TileCache::new(1_000_000, None, None);
        get_or_insert(&cache, key("src", 1), b"a").await;
        get_or_insert(&cache, key("src", 2), b"b").await;
        get_or_insert(&cache, key("other", 1), b"c").await;

        cache.invalidate_source("src");
        cache.run_pending_tasks().await;

        assert!(!cache.contains_key(&key("src", 1)));
        assert!(!cache.contains_key(&key("src", 2)));
        assert!(cache.contains_key(&key("other", 1)));
    }
}
