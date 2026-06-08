use freemouse::network::{self, KEEPALIVE_INTERVAL_SECS};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

const TEST_PORT: u16 = 19443;

#[tokio::test]
async fn keepalive_roundtrip() {
    let pin = "424242";
    let shutdown = Arc::new(AtomicBool::new(false));

    let server = tokio::spawn(async move {
        network::start_server(TEST_PORT, pin).await
    });

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let conn = network::start_client("127.0.0.1", TEST_PORT, pin)
        .await
        .expect("connect");

    let (tx, rx) = mpsc::channel(10);
    let sd = shutdown.clone();
    let loop_handle = tokio::spawn(async move {
        network::run_receive_loop(conn, tx, sd).await;
    });

    drop(rx);
    tokio::time::sleep(std::time::Duration::from_secs(KEEPALIVE_INTERVAL_SECS + 1)).await;

    shutdown.store(true, Ordering::SeqCst);
    server.abort();
    loop_handle.abort();
}
