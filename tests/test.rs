use freemouse::network::{KeyCode, MouseButton, NetworkEvent};

#[test]
fn network_event_mousemoved_roundtrip() {
    let event = NetworkEvent::MouseMoved(1920.5, 1080.25);
    let encoded = bincode::serialize(&event).unwrap();
    let decoded: NetworkEvent = bincode::deserialize(&encoded).unwrap();
    match decoded {
        NetworkEvent::MouseMoved(x, y) => {
            assert!((x - 1920.5).abs() < 1e-6);
            assert!((y - 1080.25).abs() < 1e-6);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn network_event_mousebuttons_roundtrip() {
    for btn in &[MouseButton::Left, MouseButton::Right, MouseButton::Middle, MouseButton::X1, MouseButton::X2] {
        let event = NetworkEvent::MouseButtonDown(btn.clone());
        let encoded = bincode::serialize(&event).unwrap();
        let decoded: NetworkEvent = bincode::deserialize(&encoded).unwrap();
        match decoded {
            NetworkEvent::MouseButtonDown(b) => assert_eq!(b, *btn),
            _ => panic!("wrong variant"),
        }
    }
}

#[test]
fn network_event_keyboard_roundtrip() {
    let keys = vec![
        KeyCode::A, KeyCode::B, KeyCode::Z,
        KeyCode::Num0, KeyCode::F1, KeyCode::F12,
        KeyCode::ControlLeft, KeyCode::ShiftRight,
        KeyCode::Space, KeyCode::Return, KeyCode::Escape,
        KeyCode::UpArrow, KeyCode::DownArrow,
        KeyCode::Unicode('€'),
        KeyCode::Other(42),
    ];
    for key in keys {
        let event = NetworkEvent::KeyDown(key.clone());
        let encoded = bincode::serialize(&event).unwrap();
        let decoded: NetworkEvent = bincode::deserialize(&encoded).unwrap();
        match decoded {
            NetworkEvent::KeyDown(k) => assert_eq!(k, key),
            _ => panic!("wrong variant"),
        }
    }
}

#[test]
fn network_event_clipboard_roundtrip() {
    let text = "Hello Freemouse! 🖱️";
    let event = NetworkEvent::ClipboardText(text.to_string());
    let encoded = bincode::serialize(&event).unwrap();
    let decoded: NetworkEvent = bincode::deserialize(&encoded).unwrap();
    match decoded {
        NetworkEvent::ClipboardText(t) => assert_eq!(t, text),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn network_event_all_variants_serialize() {
    let events = vec![
        NetworkEvent::MouseMoved(100.0, 200.0),
        NetworkEvent::MouseMoveRelative(10.0, -5.0),
        NetworkEvent::MouseButtonDown(MouseButton::Left),
        NetworkEvent::MouseButtonUp(MouseButton::Right),
        NetworkEvent::MouseScroll(0, -3),
        NetworkEvent::KeyDown(KeyCode::Return),
        NetworkEvent::KeyUp(KeyCode::Escape),
        NetworkEvent::ClipboardText("test".into()),
        NetworkEvent::KeepAlive,
        NetworkEvent::AuthOk,
        NetworkEvent::CursorWarp(1.0, 2.0),
    ];
    for event in events {
        let encoded = bincode::serialize(&event).unwrap();
        let decoded: NetworkEvent = bincode::deserialize(&encoded).unwrap();
        // Just verify it round-trips without panic
        let _ = format!("{:?}", decoded);
    }
}
