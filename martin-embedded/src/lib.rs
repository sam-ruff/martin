//! Embed the [martin](https://github.com/maplibre/martin) tile server inside another application.
//!
//! Build a [`Config`] in code or load one with [`load_config`], then run the
//! server with [`serve`], or use [`start`] to also get the bound address and
//! a [`CacheInvalidator`] for tile files that change on disk.

#[cfg(not(any(
    feature = "mbtiles",
    feature = "pmtiles",
    feature = "geojson",
    feature = "postgres"
)))]
compile_error!(
    "martin-embedded needs at least one tile source feature: mbtiles, pmtiles, geojson, postgres"
);

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use martin::TileSourceManager;
use martin::config::primitives::IdResolver;
use martin::config::primitives::env::OsEnv;
use martin::srv::{RESERVED_KEYWORDS, new_server};
#[cfg(feature = "pmtiles")]
use martin_core::tiles::pmtiles::PmtCache;

pub use martin::config::file::{Config, FileConfigEnum, read_config};
pub use martin::{MartinError, MartinResult, config, logging};

/// Boxed server future returned by [`start`]. It is not `Send`: await it on
/// the task that created it, or run [`serve`] on a dedicated runtime/thread.
pub type ServerFuture = Pin<Box<dyn Future<Output = MartinResult<()>>>>;

/// Refreshes a source whose backing tile file was replaced on disk: the
/// source is reloaded and its cached tiles invalidated, exactly like martin's
/// own per-request modified detection, while other sources stay cached.
/// Cheap to clone and safe to call from any thread.
#[derive(Clone)]
pub struct CacheInvalidator {
    manager: TileSourceManager,
    #[cfg(feature = "pmtiles")]
    pmtiles_directories: PmtCache,
}

impl CacheInvalidator {
    /// Reload `source_id` from disk and drop its cached tiles. The `PMTiles`
    /// directory cache is cleared as well (directories are re-read lazily
    /// and cheaply) because its entries are not addressable per source.
    pub async fn invalidate_source(&self, source_id: &str) {
        #[cfg(feature = "pmtiles")]
        {
            self.pmtiles_directories.invalidate_all();
            self.pmtiles_directories.run_pending_tasks().await;
        }
        match self.manager.reload_source(source_id).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::warn!("Source '{source_id}' was not reloaded (unknown id or reload failed)");
            }
            Err(e) => tracing::warn!("Failed to apply reload for source '{source_id}': {e}"),
        }
    }
}

/// Load a YAML config file, substituting `${ENV_VAR}` references.
pub fn load_config(path: &Path) -> MartinResult<Config> {
    Ok(read_config(path, &OsEnv)?)
}

/// Resolve the configured sources and bind the server.
///
/// Returns the server future, the address it is listening on, and a
/// [`CacheInvalidator`] for handling tile files that change on disk.
/// The caller owns tracing/logging setup.
pub async fn start(mut config: Config) -> MartinResult<(ServerFuture, String, CacheInvalidator)> {
    config.finalize()?;
    let resolver = IdResolver::new(RESERVED_KEYWORDS);
    let state = config.resolve(&resolver).await?;
    let invalidator = CacheInvalidator {
        manager: state.tile_manager.clone(),
        #[cfg(feature = "pmtiles")]
        pmtiles_directories: state.pmtiles_cache.clone(),
    };
    let (server, addr) = new_server(config.srv, state)?;
    Ok((server, addr, invalidator))
}

/// Start the server and run it until it exits.
pub async fn serve(config: Config) -> MartinResult<()> {
    let (server, _, _) = start(config).await?;
    server.await
}
