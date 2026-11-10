use std::path::Path;
use std::time::{Duration, Instant};

use martin_embedded::{CacheInvalidator, Config, FileConfigEnum};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{AssertSqlSafe, Connection as _, SqliteConnection, raw_sql};

const ADDR: &str = "127.0.0.1:3198";
const SOURCE_ID: &str = "layer";
const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// Write an `MBTiles` file holding one JSON tile at 0/0/0.
async fn write_mbtiles(path: &Path, tile: &str) {
    let script = format!(
        "CREATE TABLE metadata (name text, value text);
         INSERT INTO metadata VALUES
           ('name','reload test'), ('minzoom','0'), ('maxzoom','0'), ('format','json');
         CREATE TABLE tiles (zoom_level integer, tile_column integer, tile_row integer, tile_data blob);
         INSERT INTO tiles VALUES (0, 0, 0, CAST('{tile}' AS BLOB));
         CREATE UNIQUE INDEX name ON metadata (name);
         CREATE UNIQUE INDEX tile_index ON tiles (zoom_level, tile_column, tile_row);"
    );
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);
    let mut conn = SqliteConnection::connect_with(&options)
        .await
        .expect("the tiles file can be created");
    raw_sql(AssertSqlSafe(script))
        .execute(&mut conn)
        .await
        .expect("the tiles file can be filled");
    conn.close().await.expect("the tiles file can be closed");
}

async fn wait_until_ready(client: &reqwest::Client) {
    let deadline = Instant::now() + READY_TIMEOUT;
    while client
        .get(format!("http://{ADDR}/catalog"))
        .send()
        .await
        .is_err()
    {
        assert!(Instant::now() < deadline, "server did not become ready");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn fetch_tile(client: &reqwest::Client) -> String {
    let response = client
        .get(format!("http://{ADDR}/{SOURCE_ID}/0/0/0"))
        .send()
        .await
        .expect("the tile request is answered");
    assert!(response.status().is_success());
    response.text().await.expect("the tile has a text body")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replaced_mbtiles_file_is_served_after_invalidation() {
    let dir = tempfile::tempdir().expect("a temporary directory can be created");
    let served = dir.path().join(format!("{SOURCE_ID}.mbtiles"));
    write_mbtiles(&served, r#"{"version":"first"}"#).await;

    let mut config = Config::default();
    config.srv.listen_addresses = Some(ADDR.to_string());
    config.mbtiles = FileConfigEnum::new(vec![served.clone()]);
    let (server, _addr, invalidator) = martin_embedded::start(config)
        .await
        .expect("the server starts");

    // The server future is not Send, so run it and the assertions on the same task.
    tokio::select! {
        res = server => panic!("server exited early: {res:?}"),
        () = replace_and_invalidate(dir.path(), &served, &invalidator) => {}
    }
}

async fn replace_and_invalidate(dir: &Path, served: &Path, invalidator: &CacheInvalidator) {
    let client = reqwest::Client::new();
    wait_until_ready(&client).await;
    assert_eq!(fetch_tile(&client).await, r#"{"version":"first"}"#);

    let replacement = dir.join("replacement.mbtiles");
    write_mbtiles(&replacement, r#"{"version":"second"}"#).await;
    std::fs::rename(&replacement, served).expect("the served file can be replaced");
    assert_eq!(
        fetch_tile(&client).await,
        r#"{"version":"first"}"#,
        "the open source keeps serving the file it opened"
    );

    invalidator.invalidate_source(SOURCE_ID).await;
    assert_eq!(fetch_tile(&client).await, r#"{"version":"second"}"#);
}
