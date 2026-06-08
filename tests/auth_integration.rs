use freemouse::network::{self, NetworkEvent};

const TEST_PORT_VALID: u16 = 19441;
const TEST_PORT_INVALID: u16 = 19442;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

#[tokio::test]
async fn auth_handshake_valid_pin() {
    let pin = "123456";
    let shutdown = Arc::new(AtomicBool::new(false));

    let server = tokio::spawn(async move {
        network::start_server(TEST_PORT_VALID, pin).await
    });

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let client = network::start_client("127.0.0.1", TEST_PORT_VALID, pin).await;
    assert!(client.is_ok(), "valid PIN should connect: {:?}", client.err());

    if let Ok(conn) = client {
        let (tx, rx) = mpsc::channel(10);
        let sd = shutdown.clone();
        tokio::spawn(async move {
            network::run_receive_loop(conn, tx, sd).await;
        });
        drop(rx);
    }
    shutdown.store(true, Ordering::SeqCst);
    server.abort();
}

#[tokio::test]
async fn auth_handshake_invalid_pin() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let server = tokio::spawn(async move {
        network::start_server(TEST_PORT_INVALID, "111111").await
    });

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let result = network::start_client("127.0.0.1", TEST_PORT_INVALID, "999999").await;
    assert!(result.is_err(), "wrong PIN should fail");

    shutdown.store(true, Ordering::SeqCst);
    server.abort();
}

#[test]
fn auth_token_deterministic() {
    use chacha20poly1305::Key;
    let key = Key::from_slice(&[42u8; 32]);
    let t1 = network::compute_auth_token(key);
    let t2 = network::compute_auth_token(key);
    assert_eq!(t1, t2);
}

#[test]
fn protocol_v2_events_roundtrip() {
    use freemouse::network::{Edge, ScreenInfo};
    use uuid::Uuid;

    let events = vec![
        NetworkEvent::AuthChallenge([1u8; 32]),
        NetworkEvent::AuthOk,
        NetworkEvent::AuthFail("bad".into()),
        NetworkEvent::Hello {
            machine_id: Uuid::new_v4(),
            hostname: "test".into(),
            screens: vec![ScreenInfo {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
                primary: true,
            }],
            protocol_version: 2,
        },
        NetworkEvent::RemoteEnter {
            from_edge: Edge::Right,
            y_ratio: 0.5,
        },
        NetworkEvent::RemoteLeave {
            to_edge: Edge::Left,
        },
        NetworkEvent::CursorWarp(100.0, 200.0),
        NetworkEvent::KeyStateSnapshot(vec![]),
    ];

    for event in events {
        let encoded = bincode::serialize(&event).unwrap();
        let decoded: NetworkEvent = bincode::deserialize(&encoded).unwrap();
        let _ = format!("{:?}", decoded);
    }
}
