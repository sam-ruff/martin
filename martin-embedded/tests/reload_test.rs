#![cfg(feature = "mbtiles")]

use std::path::Path;
use std::time::{Duration, Instant};

use martin_embedded::{CacheInvalidator, Config, FileConfigEnum};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{AssertSqlSafe, Connection as _, SqliteConnection, raw_sql};

const SOURCE_ID: &str = "layer";
const FIRST: &str = r#"{"version":"first"}"#;
const SECOND: &str = r#"{"version":"second"}"#;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Write an `MBTiles` file holding the same JSON tile at 0/0/0 and 1/0/0.
async fn write_mbtiles(path: &Path, tile: &str) {
    let script = format!(
        "CREATE TABLE metadata (name text, value text);
         INSERT INTO metadata VALUES
           ('name','reload test'), ('minzoom','0'), ('maxzoom','1'), ('format','json');
         CREATE TABLE tiles (zoom_level integer, tile_column integer, tile_row integer, tile_data blob);
         INSERT INTO tiles VALUES
           (0, 0, 0, CAST('{tile}' AS BLOB)), (1, 0, 1, CAST('{tile}' AS BLOB));
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

/// Swap a finished file into place, the way callers of `invalidate_source` are told to.
fn rename_over(staged: &Path, tile_file: &Path) {
    std::fs::rename(staged, tile_file).expect("the served file can be replaced");
}

async fn replace_with_mbtiles(tile_file: &Path, tile: &str) {
    let staged = tile_file.with_extension("staged");
    write_mbtiles(&staged, tile).await;
    rename_over(&staged, tile_file);
}

fn replace_with_garbage(tile_file: &Path) {
    let staged = tile_file.with_extension("staged");
    std::fs::write(&staged, "this is not an SQLite database").expect("garbage can be written");
    rename_over(&staged, tile_file);
}

/// HTTP client for the server under test.
struct TileClient {
    http: reqwest::Client,
    addr: &'static str,
}

impl TileClient {
    fn new(addr: &'static str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("the HTTP client can be built");
        Self { http, addr }
    }

    async fn wait_until_ready(&self) {
        let catalog = format!("http://{}/catalog", self.addr);
        let deadline = Instant::now() + WAIT_TIMEOUT;
        while self.http.get(&catalog).send().await.is_err() {
            assert!(Instant::now() < deadline, "server did not become ready");
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    async fn request_tile(&self, zxy: &str) -> reqwest::Response {
        self.http
            .get(format!("http://{}/{SOURCE_ID}/{zxy}", self.addr))
            .send()
            .await
            .expect("the tile request is answered")
    }

    async fn tile(&self, zxy: &str) -> String {
        let response = self.request_tile(zxy).await;
        let status = response.status();
        assert!(status.is_success(), "{zxy} was answered with {status}");
        response.text().await.expect("the tile has a text body")
    }

    /// Polls while `zxy` is answered with a server error, then returns it.
    async fn tile_once_served(&self, zxy: &str) -> String {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            let response = self.request_tile(zxy).await;
            let status = response.status();
            if status.is_success() {
                return response.text().await.expect("the tile has a text body");
            }
            assert!(status.is_server_error(), "{zxy} was answered with {status}");
            assert!(
                Instant::now() < deadline,
                "{zxy} is still answered with {status}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// Serves `tile_file` on `addr` and runs `scenario` against it. The server future is not
/// `Send`, so both run on this task.
async fn serve_and_run(
    addr: &'static str,
    tile_file: &Path,
    scenario: impl AsyncFnOnce(TileClient, CacheInvalidator),
) {
    let mut config = Config::default();
    config.srv.listen_addresses = Some(addr.to_string());
    config.mbtiles = FileConfigEnum::new(vec![tile_file.to_path_buf()]);
    let (server, _addr, invalidator) = martin_embedded::start(config)
        .await
        .expect("the server starts");

    let client = TileClient::new(addr);
    let scenario = async {
        client.wait_until_ready().await;
        scenario(client, invalidator).await;
    };
    tokio::select! {
        res = server => panic!("server exited early: {res:?}"),
        finished = tokio::time::timeout(SCENARIO_TIMEOUT, scenario) => {
            finished.expect("the scenario finishes in time");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replaced_mbtiles_file_is_served_after_invalidation() {
    let dir = tempfile::tempdir().expect("a temporary directory can be created");
    let tile_file = dir.path().join(format!("{SOURCE_ID}.mbtiles"));
    write_mbtiles(&tile_file, FIRST).await;

    serve_and_run("127.0.0.1:3198", &tile_file, async |client, invalidator| {
        assert_eq!(client.tile("0/0/0").await, FIRST);

        replace_with_mbtiles(&tile_file, SECOND).await;
        assert_eq!(
            client.tile("0/0/0").await,
            FIRST,
            "a cached tile stays stale until its source is invalidated"
        );

        invalidator.invalidate_source(SOURCE_ID).await;
        assert_eq!(client.tile("0/0/0").await, SECOND);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bad_replacement_leaves_the_previous_source_serving() {
    let dir = tempfile::tempdir().expect("a temporary directory can be created");
    let tile_file = dir.path().join(format!("{SOURCE_ID}.mbtiles"));
    write_mbtiles(&tile_file, FIRST).await;

    serve_and_run("127.0.0.1:3197", &tile_file, async |client, invalidator| {
        assert_eq!(client.tile("0/0/0").await, FIRST);

        replace_with_garbage(&tile_file);
        invalidator.invalidate_source(SOURCE_ID).await;

        assert_eq!(
            client.tile("0/0/0").await,
            FIRST,
            "a failed reload keeps the cached tiles"
        );
        // 1/0/0 was never requested, so the previous source has to read it rather than the
        // tile cache. That read fails whenever the source has no idle connection left from
        // before the replacement and opens one against the garbage, hence the polling.
        assert_eq!(client.tile_once_served("1/0/0").await, FIRST);

        replace_with_mbtiles(&tile_file, SECOND).await;
        invalidator.invalidate_source(SOURCE_ID).await;
        assert_eq!(client.tile("0/0/0").await, SECOND);
        assert_eq!(client.tile("1/0/0").await, SECOND);
    })
    .await;
}
