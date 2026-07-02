//! Serve a `PMTiles` file with an embedded martin server:
//!
//! ```sh
//! cargo run -p martin-embedded --example serve -- [FILE.pmtiles]
//! ```

use std::path::PathBuf;

use martin_embedded::logging::{LogFormat, init_tracing};
use martin_embedded::{Config, FileConfigEnum, MartinResult};

#[tokio::main]
async fn main() -> MartinResult<()> {
    init_tracing("info", LogFormat::from_env(), false);

    let path = std::env::args().nth(1).map_or_else(
        || PathBuf::from("tests/fixtures/pmtiles/png.pmtiles"),
        PathBuf::from,
    );

    let mut config = Config::default();
    config.srv.listen_addresses = Some("127.0.0.1:3111".to_string());
    config.pmtiles = FileConfigEnum::new(vec![path]);

    let (server, addr, _invalidator) = martin_embedded::start(config).await?;
    tracing::info!("catalog at http://{addr}/catalog");
    server.await
}
