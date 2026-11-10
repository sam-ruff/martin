use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use martin_tile_utils::{TileCoord, TileData, TileInfo};
use tilejson::TileJSON;

use crate::CacheZoomRange;
use crate::tiles::catalog::CatalogSourceEntry;
use crate::tiles::{MartinCoreError, MartinCoreResult, Tile};

/// URL query parameters for dynamic tile generation.
pub type UrlQuery = HashMap<String, String>;

/// Returns a process-wide unique, non-zero value for [`Source::cache_epoch`].
#[must_use]
pub fn next_cache_epoch() -> u64 {
    static NEXT_CACHE_EPOCH: AtomicU64 = AtomicU64::new(1);
    NEXT_CACHE_EPOCH.fetch_add(1, Ordering::Relaxed)
}

/// Core trait for tile sources providing data to Martin
///
/// Implementors can serve tiles from databases, files, or other backends.
#[async_trait]
pub trait Source: Send + Sync + Debug {
    /// Unique source identifier used in URLs.
    fn get_id(&self) -> &str;

    /// `TileJSON` specification served to clients.
    fn get_tilejson(&self) -> &TileJSON;

    /// Technical tile information (format, encoding, etc.).
    fn get_tile_info(&self) -> TileInfo;

    /// Creates a boxed clone for trait object storage.
    fn clone_source(&self) -> BoxedSource;

    /// A version string for this source, if available. Default: None.
    /// If available, this string is appended to tile URLs as a query parameter,
    /// invalidating caches.
    fn get_version(&self) -> Option<String> {
        None
    }

    /// Whether this source accepts URL query parameters. Default: false.
    fn support_url_query(&self) -> bool {
        false
    }

    /// Whether martin-cp should use concurrent scraping. Default: false.
    fn benefits_from_concurrent_scraping(&self) -> bool {
        false
    }

    /// Zoom-level bounds for tile caching.
    fn cache_zoom(&self) -> CacheZoomRange;

    /// Identifies this source instance in tile cache keys, so that a reloaded source never
    /// reads a tile cached by the instance it replaced. Default: 0.
    ///
    /// Sources implementing [`try_reload()`](Self::try_reload) should return a
    /// [`next_cache_epoch`] value that is taken at construction and shared by clones.
    fn cache_epoch(&self) -> u64 {
        0
    }

    /// Retrieves tile data for the given coordinates.
    ///
    /// # Arguments
    /// * `xyz` - Tile coordinates (x, y, zoom)
    /// * `url_query` - Optional query parameters for dynamic tiles
    async fn get_tile(
        &self,
        xyz: TileCoord,
        url_query: Option<&UrlQuery>,
    ) -> MartinCoreResult<TileData>;

    /// Retrieves tile with etag for the given coordinates.
    ///
    /// Default implementation calls [`get_tile()`](Self::get_tile) and computes etag using `xxh3_128`.
    /// Sources can override this for more performance.
    ///
    /// # Arguments
    /// * `xyz` - Tile coordinates (x, y, zoom)
    /// * `url_query` - Optional query parameters for dynamic tiles
    async fn get_tile_with_etag(
        &self,
        xyz: TileCoord,
        url_query: Option<&UrlQuery>,
    ) -> MartinCoreResult<Tile> {
        let data = self.get_tile(xyz, url_query).await?;
        Ok(Tile::new_hash_etag(data, self.get_tile_info()))
    }

    /// Validates zoom level against `TileJSON` min/max zoom constraints.
    fn is_valid_zoom(&self, zoom: u8) -> bool {
        let tj = self.get_tilejson();
        tj.minzoom.is_none_or(|minzoom| zoom >= minzoom)
            && tj.maxzoom.is_none_or(|maxzoom| zoom <= maxzoom)
    }

    /// Attempts to create a fresh instance of this source.
    ///
    /// Sources that return a `MartinCoreError::SourceNeedReload` from `get_tile()` must also
    /// implement this method.
    ///
    /// The default implementation returns an error, for sources that cannot be reloaded.
    async fn try_reload(&self) -> MartinCoreResult<BoxedSource> {
        Err(MartinCoreError::OtherError(
            format!("Source {} does not support reloading", self.get_id()).into(),
        ))
    }

    /// Generates catalog entry for this source.
    fn get_catalog_entry(&self) -> CatalogSourceEntry {
        let id = self.get_id();
        let tilejson = self.get_tilejson();
        let info = self.get_tile_info();
        CatalogSourceEntry {
            content_type: info.format.content_type().to_string(),
            content_encoding: info.encoding.compression().map(ToString::to_string),
            name: tilejson.name.as_ref().filter(|v| *v != id).cloned(),
            description: tilejson.description.clone(),
            attribution: tilejson.attribution.clone(),
            // FIXME: surface `tilejson.vector_layers.len()` once we always have it.
            layer_count: None,
            // FIXME: surface the source's mtime (mbtiles/pmtiles modtime, etc.).
            last_modified_at: None,
        }
    }
}

/// Boxed tile source trait object for storage in collections.
pub type BoxedSource = Box<dyn Source>;

impl Clone for BoxedSource {
    fn clone(&self) -> Self {
        self.clone_source()
    }
}

#[cfg(test)]
mod tests {
    use martin_tile_utils::{Encoding, Format};
    use tilejson::tilejson;

    use super::*;

    /// A source that relies on every default of the trait.
    #[derive(Debug, Clone)]
    struct DefaultsSource {
        id: &'static str,
        tilejson: TileJSON,
    }

    #[async_trait]
    impl Source for DefaultsSource {
        fn get_id(&self) -> &str {
            self.id
        }

        fn get_tilejson(&self) -> &TileJSON {
            &self.tilejson
        }

        fn get_tile_info(&self) -> TileInfo {
            TileInfo::new(Format::Mvt, Encoding::Uncompressed)
        }

        fn clone_source(&self) -> BoxedSource {
            Box::new(self.clone())
        }

        fn cache_zoom(&self) -> CacheZoomRange {
            CacheZoomRange::default()
        }

        async fn get_tile(
            &self,
            _xyz: TileCoord,
            _url_query: Option<&UrlQuery>,
        ) -> MartinCoreResult<TileData> {
            Ok(Vec::new())
        }
    }

    fn defaults_source() -> DefaultsSource {
        DefaultsSource {
            id: "defaults",
            tilejson: tilejson! { tiles: vec![] },
        }
    }

    #[tokio::test]
    async fn try_reload_fails_by_default() {
        let err = defaults_source()
            .try_reload()
            .await
            .expect_err("a source without try_reload cannot be reloaded");
        assert_eq!(
            err.to_string(),
            "Source defaults does not support reloading"
        );
    }

    #[test]
    fn cache_epoch_is_zero_by_default() {
        assert_eq!(defaults_source().cache_epoch(), 0);
    }

    #[test]
    fn next_cache_epoch_never_repeats_or_returns_the_default() {
        let first = next_cache_epoch();
        let second = next_cache_epoch();
        assert_ne!(first, 0);
        assert_ne!(second, 0);
        assert_ne!(first, second);
    }
}
