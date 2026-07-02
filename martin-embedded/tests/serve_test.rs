use std::path::PathBuf;
use std::time::Duration;

use martin_embedded::{Config, FileConfigEnum};

const ADDR: &str = "127.0.0.1:3199";

fn fixture() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/fixtures/pmtiles/png.pmtiles"
    ))
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_catalog_and_tiles() {
    let mut config = Config::default();
    config.srv.listen_addresses = Some(ADDR.to_string());
    config.pmtiles = FileConfigEnum::new(vec![fixture()]);

    let (server, addr, _invalidator) = martin_embedded::start(config).await.unwrap();
    assert_eq!(addr, ADDR);

    // The server future is not Send, so run it and the assertions on the same task.
    tokio::select! {
        res = server => panic!("server exited early: {res:?}"),
        () = assert_endpoints() => {}
    }
}

async fn assert_endpoints() {
    let client = reqwest::Client::new();
    let catalog_url = format!("http://{ADDR}/catalog");

    let mut catalog = None;
    for _ in 0..50 {
        if let Ok(resp) = client.get(&catalog_url).send().await {
            catalog = Some(resp);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let catalog = catalog.expect("server did not become ready");
    assert!(catalog.status().is_success());
    let body = catalog.text().await.unwrap();
    assert!(body.contains("\"png\""), "catalog missing png source: {body}");

    let tile = client
        .get(format!("http://{ADDR}/png/0/0/0"))
        .send()
        .await
        .unwrap();
    assert!(tile.status().is_success());
    assert_eq!(tile.headers()["content-type"], "image/png");
    assert!(!tile.bytes().await.unwrap().is_empty());
}
