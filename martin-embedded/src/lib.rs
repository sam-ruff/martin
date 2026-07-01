//! Embed the [martin](https://github.com/maplibre/martin) tile server inside another application.
//!
//! Build a [`Config`] in code or load one with [`load_config`], then run the
//! server with [`serve`], or use [`start`] to also get the bound address.

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

use martin::config::primitives::IdResolver;
use martin::config::primitives::env::OsEnv;
use martin::srv::{RESERVED_KEYWORDS, new_server};

pub use martin::config::file::{Config, FileConfigEnum, read_config};
pub use martin::{MartinError, MartinResult, config, logging};

/// Boxed server future returned by [`start`]. It is not `Send`: await it on
/// the task that created it, or run [`serve`] on a dedicated runtime/thread.
pub type ServerFuture = Pin<Box<dyn Future<Output = MartinResult<()>>>>;

/// Load a YAML config file, substituting `${ENV_VAR}` references.
pub fn load_config(path: &Path) -> MartinResult<Config> {
    Ok(read_config(path, &OsEnv)?)
}

/// Resolve the configured sources and bind the server.
///
/// Returns the server future together with the address it is listening on.
/// The caller owns tracing/logging setup.
pub async fn start(mut config: Config) -> MartinResult<(ServerFuture, String)> {
    config.finalize()?;
    let resolver = IdResolver::new(RESERVED_KEYWORDS);
    let state = config.resolve(&resolver).await?;
    new_server(config.srv, state)
}

/// Start the server and run it until it exits.
pub async fn serve(config: Config) -> MartinResult<()> {
    let (server, _) = start(config).await?;
    server.await
}
