use crate::network::{KeyCode, MouseButton, NetworkEvent};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

// ============================================================
// Screen size detection (cross-platform)
// ============================================================

/// Returns the main display dimensions (width, height) in pixels.
/// Falls back to 1920x1080 on failure.
pub fn get_screen_size() -> (f64, f64) {
    #[cfg(any(windows, target_os = "macos"))]
    {
        // Use rdev for screen size on Windows/macOS
        match rdev::display_size() {
            Ok((w, h)) => return (w as f64, h as f64),
            Err(_) => {}
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Try using env vars or xrandr on Linux
        if let Ok(output) = std::process::Command::new("xrandr")
            .args(["--current", "--query"])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.contains(" connected") {
                    // Parse "2560x1440+0+0" or similar
                    if let Some(dim_str) = line
                        .split(' ')
                        .find(|s| s.contains('x') && s.contains('+'))
                        .or_else(|| {
                            // Alternative: "1920x1080"
                            line.split(' ')
                                .find(|s| s.contains('x') && !s.contains('+'))
                        })
                    {
                        // Extract just the resolution part before any '+'
                        let res = dim_str.split('+').next().unwrap_or(dim_str);
                        if let Some((w_str, h_str)) = res.split_once('x') {
                            if let (Ok(w), Ok(h)) = (w_str.parse::<f64>(), h_str.parse::<f64>()) {
                                return (w, h);
                            }
                        }
                    }
                }
            }
        }

        // Fallback: check WAYLAND_DISPLAY and DISPLAY env vars
        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            // Try `wlr-randr` or `wl-info`
            if let Ok(output) = std::process::Command::new("wlr-randr")
                .arg("--json")
                .output()
            {
                // Simple JSON parse for resolution
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(mode) = stdout.split("\"mode\"").nth(1) {
                    if let Some(w) = mode.split("\"width\":").nth(1) {
                        if let Some(w_val) = w.split(',').next() {
                            if let Some(h) = mode.split("\"height\":").nth(1) {
                                if let Some(h_val) = h.split(',').next() {
                                    if let (Ok(w), Ok(h)) =
                                        (w_val.trim().parse::<f64>(), h_val.trim().parse::<f64>())
                                    {
                                        return (w, h);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    (1920.0, 1080.0)
}

// ============================================================
// Capture (Share side): Global input grab
// ============================================================

#[cfg(any(windows, target_os = "macos"))]
pub mod os {
    use super::*;
    use enigo::{
        Axis, Button as EnigoButton, Coordinate, Direction, Enigo, Key as EnigoKey, Keyboard,
        Mouse, Settings,
    };
    use rdev::{grab, Button as RdevButton, Event, EventType, Key as RdevKey};

    lazy_static::lazy_static! {
        static ref IS_REMOTE: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        static ref STOP_FLAG: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    }

    /// Stop the capture thread (sets the stop flag).
    pub fn stop_capture() {
        STOP_FLAG.store(true, Ordering::SeqCst);
    }

    /// Manual toggle remote mode on/off.
    /// Returns the new state.
    pub fn toggle_remote() -> bool {
        let new = !IS_REMOTE.load(Ordering::SeqCst);
        IS_REMOTE.store(new, Ordering::SeqCst);
        tracing::info!("Remote mode manually toggled to {}", if new { "ON" } else { "OFF" });
        new
    }

    /// Returns current remote mode state.
    pub fn is_remote() -> bool {
        IS_REMOTE.load(Ordering::SeqCst)
    }

    /// Start global input capture. Spawns a thread that uses rdev::grab
    /// to intercept input events when in "remote" mode.
    pub fn start_capture(tx: mpsc::Sender<NetworkEvent>, screen_width: f64) {
        // Reset state for a fresh capture session
        IS_REMOTE.store(false, Ordering::SeqCst);
        STOP_FLAG.store(false, Ordering::SeqCst);

        let is_remote = IS_REMOTE.clone();
        let stop_flag = STOP_FLAG.clone();

        std::thread::spawn(move || {
            let callback = move |event: Event| -> Option<Event> {
                // When stopped, pass all events through normally
                if stop_flag.load(Ordering::SeqCst) {
                    return Some(event);
                }

                let currently_remote = is_remote.load(Ordering::SeqCst);
                let mut event_handled = false;

                match event.event_type {
                    EventType::MouseMove { x, y } => {
                        if !currently_remote && x >= screen_width - 2.0 {
                            is_remote.store(true, Ordering::SeqCst);
                            event_handled = true;
                        } else if currently_remote && x <= 2.0 {
                            is_remote.store(false, Ordering::SeqCst);
                            event_handled = true;
                        } else if currently_remote {
                            if tx.blocking_send(NetworkEvent::MouseMoved(x, y)).is_err() {
                                // Channel closed, stop capture
                                stop_flag.store(true, Ordering::SeqCst);
                                return Some(event);
                            }
                            event_handled = true;
                        }
                    }
                    EventType::KeyPress(key) => {
                        if currently_remote {
                            if let Some(kc) = rdev_key_to_keycode(key) {
                                if tx.blocking_send(NetworkEvent::KeyDown(kc)).is_err() {
                                    stop_flag.store(true, Ordering::SeqCst);
                                    return Some(event);
                                }
                            }
                            event_handled = true;
                        }
                    }
                    EventType::KeyRelease(key) => {
                        if currently_remote {
                            if let Some(kc) = rdev_key_to_keycode(key) {
                                if tx.blocking_send(NetworkEvent::KeyUp(kc)).is_err() {
                                    stop_flag.store(true, Ordering::SeqCst);
                                    return Some(event);
                                }
                            }
                            event_handled = true;
                        }
                    }
                    EventType::ButtonPress(btn) => {
                        if currently_remote {
                            let mb = rdev_button_to_mousebutton(btn);
                            if tx.blocking_send(NetworkEvent::MouseButtonDown(mb)).is_err() {
                                stop_flag.store(true, Ordering::SeqCst);
                                return Some(event);
                            }
                            event_handled = true;
                        }
                    }
                    EventType::ButtonRelease(btn) => {
                        if currently_remote {
                            let mb = rdev_button_to_mousebutton(btn);
                            if tx.blocking_send(NetworkEvent::MouseButtonUp(mb)).is_err() {
                                stop_flag.store(true, Ordering::SeqCst);
                                return Some(event);
                            }
                            event_handled = true;
                        }
                    }
                    EventType::Wheel { delta_x, delta_y } => {
                        if currently_remote {
                            if tx
                                .blocking_send(NetworkEvent::MouseScroll(
                                    delta_x as i32,
                                    delta_y as i32,
                                ))
                                .is_err()
                            {
                                stop_flag.store(true, Ordering::SeqCst);
                                return Some(event);
                            }
                            event_handled = true;
                        }
                    }
                }
                if event_handled {
                    None
                } else {
                    Some(event)
                }
            };

            if let Err(e) = grab(callback) {
                eprintln!("Error grabbing input: {:?}", e);
            }
        });
    }

    /// Start receiving and simulating input events on the receiver side.
    pub async fn start_simulation(mut rx: mpsc::Receiver<NetworkEvent>) {
        let mut enigo = match Enigo::new(&Settings::default()) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("Failed to create Enigo instance: {:?}", e);
                return;
            }
        };

        while let Some(event) = rx.recv().await {
            match event {
                NetworkEvent::MouseMoved(x, y) => {
                    let _ = enigo.move_mouse(x as i32, y as i32, Coordinate::Abs);
                }
                NetworkEvent::MouseMoveRelative(dx, dy) => {
                    let _ = enigo.move_mouse(dx as i32, dy as i32, Coordinate::Rel);
                }
                NetworkEvent::MouseButtonDown(btn) => {
                    let eb = mousebutton_to_enigo_button(&btn);
                    let _ = enigo.button(eb, Direction::Press);
                }
                NetworkEvent::MouseButtonUp(btn) => {
                    let eb = mousebutton_to_enigo_button(&btn);
                    let _ = enigo.button(eb, Direction::Release);
                }
                NetworkEvent::MouseScroll(_dx, dy) => {
                    let _ = enigo.scroll(dy, Axis::Vertical);
                }
                NetworkEvent::KeyDown(kc) => {
                    if let Some(ek) = keycode_to_enigo_key(&kc) {
                        let _ = enigo.key(ek, Direction::Press);
                    }
                }
                NetworkEvent::KeyUp(kc) => {
                    if let Some(ek) = keycode_to_enigo_key(&kc) {
                        let _ = enigo.key(ek, Direction::Release);
                    }
                }
                _ => {} // Clipboard/keepalive handled by network loop
            }
        }
    }

    // ============================================================
    // Key mapping: rdev::Key -> KeyCode
    // ============================================================

    fn rdev_key_to_keycode(key: RdevKey) -> Option<KeyCode> {
        // rdev 0.5 Key enum variants
        Some(match key {
            // Letters (QWERTY physical positions: KeyA-KeyZ)
            RdevKey::KeyA => KeyCode::A,
            RdevKey::KeyB => KeyCode::B,
            RdevKey::KeyC => KeyCode::C,
            RdevKey::KeyD => KeyCode::D,
            RdevKey::KeyE => KeyCode::E,
            RdevKey::KeyF => KeyCode::F,
            RdevKey::KeyG => KeyCode::G,
            RdevKey::KeyH => KeyCode::H,
            RdevKey::KeyI => KeyCode::I,
            RdevKey::KeyJ => KeyCode::J,
            RdevKey::KeyK => KeyCode::K,
            RdevKey::KeyL => KeyCode::L,
            RdevKey::KeyM => KeyCode::M,
            RdevKey::KeyN => KeyCode::N,
            RdevKey::KeyO => KeyCode::O,
            RdevKey::KeyP => KeyCode::P,
            RdevKey::KeyQ => KeyCode::Q,
            RdevKey::KeyR => KeyCode::R,
            RdevKey::KeyS => KeyCode::S,
            RdevKey::KeyT => KeyCode::T,
            RdevKey::KeyU => KeyCode::U,
            RdevKey::KeyV => KeyCode::V,
            RdevKey::KeyW => KeyCode::W,
            RdevKey::KeyX => KeyCode::X,
            RdevKey::KeyY => KeyCode::Y,
            RdevKey::KeyZ => KeyCode::Z,
            // Numbers top row
            RdevKey::Num0 => KeyCode::Num0,
            RdevKey::Num1 => KeyCode::Num1,
            RdevKey::Num2 => KeyCode::Num2,
            RdevKey::Num3 => KeyCode::Num3,
            RdevKey::Num4 => KeyCode::Num4,
            RdevKey::Num5 => KeyCode::Num5,
            RdevKey::Num6 => KeyCode::Num6,
            RdevKey::Num7 => KeyCode::Num7,
            RdevKey::Num8 => KeyCode::Num8,
            RdevKey::Num9 => KeyCode::Num9,
            // Modifiers
            RdevKey::Alt => KeyCode::Alt,
            RdevKey::AltGr => KeyCode::AltGr,
            RdevKey::ControlLeft => KeyCode::ControlLeft,
            RdevKey::ControlRight => KeyCode::ControlRight,
            RdevKey::ShiftLeft => KeyCode::ShiftLeft,
            RdevKey::ShiftRight => KeyCode::ShiftRight,
            RdevKey::MetaLeft => KeyCode::MetaLeft,
            RdevKey::MetaRight => KeyCode::MetaRight,
            // Navigation
            RdevKey::UpArrow => KeyCode::UpArrow,
            RdevKey::DownArrow => KeyCode::DownArrow,
            RdevKey::LeftArrow => KeyCode::LeftArrow,
            RdevKey::RightArrow => KeyCode::RightArrow,
            RdevKey::PageUp => KeyCode::PageUp,
            RdevKey::PageDown => KeyCode::PageDown,
            RdevKey::Home => KeyCode::Home,
            RdevKey::End => KeyCode::End,
            RdevKey::Insert => KeyCode::Insert,
            RdevKey::Delete => KeyCode::Delete,
            RdevKey::Backspace => KeyCode::Backspace,
            RdevKey::Space => KeyCode::Space,
            RdevKey::Tab => KeyCode::Tab,
            RdevKey::Return => KeyCode::Return,
            RdevKey::Escape => KeyCode::Escape,
            RdevKey::CapsLock => KeyCode::CapsLock,
            // Function keys (F1-F12 only in rdev 0.5)
            RdevKey::F1 => KeyCode::F1,
            RdevKey::F2 => KeyCode::F2,
            RdevKey::F3 => KeyCode::F3,
            RdevKey::F4 => KeyCode::F4,
            RdevKey::F5 => KeyCode::F5,
            RdevKey::F6 => KeyCode::F6,
            RdevKey::F7 => KeyCode::F7,
            RdevKey::F8 => KeyCode::F8,
            RdevKey::F9 => KeyCode::F9,
            RdevKey::F10 => KeyCode::F10,
            RdevKey::F11 => KeyCode::F11,
            RdevKey::F12 => KeyCode::F12,
            // Symbols (US layout mapping)
            RdevKey::Minus => KeyCode::Minus,
            RdevKey::Equal => KeyCode::Equals,
            RdevKey::LeftBracket => KeyCode::LeftBracket,
            RdevKey::RightBracket => KeyCode::RightBracket,
            RdevKey::BackSlash => KeyCode::Backslash,
            RdevKey::SemiColon => KeyCode::Semicolon,
            RdevKey::Quote => KeyCode::Quote,
            RdevKey::Comma => KeyCode::Comma,
            RdevKey::Dot => KeyCode::Period,
            RdevKey::Slash => KeyCode::Slash,
            RdevKey::BackQuote => KeyCode::Backtick,
            RdevKey::IntlBackslash => KeyCode::Backslash,
            // Numpad
            RdevKey::Kp0 => KeyCode::Numpad0,
            RdevKey::Kp1 => KeyCode::Numpad1,
            RdevKey::Kp2 => KeyCode::Numpad2,
            RdevKey::Kp3 => KeyCode::Numpad3,
            RdevKey::Kp4 => KeyCode::Numpad4,
            RdevKey::Kp5 => KeyCode::Numpad5,
            RdevKey::Kp6 => KeyCode::Numpad6,
            RdevKey::Kp7 => KeyCode::Numpad7,
            RdevKey::Kp8 => KeyCode::Numpad8,
            RdevKey::Kp9 => KeyCode::Numpad9,
            RdevKey::KpPlus => KeyCode::NumpadAdd,
            RdevKey::KpMinus => KeyCode::NumpadSubtract,
            RdevKey::KpMultiply => KeyCode::NumpadMultiply,
            RdevKey::KpDivide => KeyCode::NumpadDivide,
            RdevKey::KpReturn => KeyCode::NumpadEnter,
            RdevKey::KpDelete => KeyCode::NumpadDecimal,
            // Other standard keys
            RdevKey::PrintScreen => KeyCode::PrintScreen,
            RdevKey::ScrollLock => KeyCode::ScrollLock,
            RdevKey::Pause => KeyCode::Pause,
            RdevKey::NumLock => KeyCode::Other(69), // HID usage code for NumLock
            RdevKey::Function => KeyCode::Other(0x74), // HID usage code
            // Unsupported key
            _ => return None,
        })
    }

    // ============================================================
    // Mouse button mapping: rdev::Button -> MouseButton
    // ============================================================

    fn rdev_button_to_mousebutton(btn: RdevButton) -> MouseButton {
        match btn {
            RdevButton::Left => MouseButton::Left,
            RdevButton::Right => MouseButton::Right,
            RdevButton::Middle => MouseButton::Middle,
            RdevButton::Unknown(_) => MouseButton::Left,
        }
    }

    // ============================================================
    // Key mapping: KeyCode -> enigo::Key (for simulation)
    // ============================================================

    fn keycode_to_enigo_key(kc: &KeyCode) -> Option<EnigoKey> {
        match kc {
            // Letters use Unicode character (works cross-platform)
            KeyCode::A => Some(EnigoKey::Unicode('a')),
            KeyCode::B => Some(EnigoKey::Unicode('b')),
            KeyCode::C => Some(EnigoKey::Unicode('c')),
            KeyCode::D => Some(EnigoKey::Unicode('d')),
            KeyCode::E => Some(EnigoKey::Unicode('e')),
            KeyCode::F => Some(EnigoKey::Unicode('f')),
            KeyCode::G => Some(EnigoKey::Unicode('g')),
            KeyCode::H => Some(EnigoKey::Unicode('h')),
            KeyCode::I => Some(EnigoKey::Unicode('i')),
            KeyCode::J => Some(EnigoKey::Unicode('j')),
            KeyCode::K => Some(EnigoKey::Unicode('k')),
            KeyCode::L => Some(EnigoKey::Unicode('l')),
            KeyCode::M => Some(EnigoKey::Unicode('m')),
            KeyCode::N => Some(EnigoKey::Unicode('n')),
            KeyCode::O => Some(EnigoKey::Unicode('o')),
            KeyCode::P => Some(EnigoKey::Unicode('p')),
            KeyCode::Q => Some(EnigoKey::Unicode('q')),
            KeyCode::R => Some(EnigoKey::Unicode('r')),
            KeyCode::S => Some(EnigoKey::Unicode('s')),
            KeyCode::T => Some(EnigoKey::Unicode('t')),
            KeyCode::U => Some(EnigoKey::Unicode('u')),
            KeyCode::V => Some(EnigoKey::Unicode('v')),
            KeyCode::W => Some(EnigoKey::Unicode('w')),
            KeyCode::X => Some(EnigoKey::Unicode('x')),
            KeyCode::Y => Some(EnigoKey::Unicode('y')),
            KeyCode::Z => Some(EnigoKey::Unicode('z')),

            // Numbers use Unicode (works cross-platform)
            KeyCode::Num0 => Some(EnigoKey::Unicode('0')),
            KeyCode::Num1 => Some(EnigoKey::Unicode('1')),
            KeyCode::Num2 => Some(EnigoKey::Unicode('2')),
            KeyCode::Num3 => Some(EnigoKey::Unicode('3')),
            KeyCode::Num4 => Some(EnigoKey::Unicode('4')),
            KeyCode::Num5 => Some(EnigoKey::Unicode('5')),
            KeyCode::Num6 => Some(EnigoKey::Unicode('6')),
            KeyCode::Num7 => Some(EnigoKey::Unicode('7')),
            KeyCode::Num8 => Some(EnigoKey::Unicode('8')),
            KeyCode::Num9 => Some(EnigoKey::Unicode('9')),

            // Symbols use Unicode
            KeyCode::Minus => Some(EnigoKey::Unicode('-')),
            KeyCode::Equals => Some(EnigoKey::Unicode('=')),
            KeyCode::LeftBracket => Some(EnigoKey::Unicode('[')),
            KeyCode::RightBracket => Some(EnigoKey::Unicode(']')),
            KeyCode::Backslash => Some(EnigoKey::Unicode('\\')),
            KeyCode::Semicolon => Some(EnigoKey::Unicode(';')),
            KeyCode::Quote => Some(EnigoKey::Unicode('\'')),
            KeyCode::Comma => Some(EnigoKey::Unicode(',')),
            KeyCode::Period => Some(EnigoKey::Unicode('.')),
            KeyCode::Slash => Some(EnigoKey::Unicode('/')),
            KeyCode::Backtick => Some(EnigoKey::Unicode('`')),

            // Named cross-platform keys in enigo
            KeyCode::Alt | KeyCode::Option => Some(EnigoKey::Alt),
            KeyCode::AltGr => Some(EnigoKey::Alt),
            KeyCode::ControlLeft | KeyCode::ControlRight => Some(EnigoKey::Control),
            KeyCode::ShiftLeft | KeyCode::ShiftRight => Some(EnigoKey::Shift),
            KeyCode::MetaLeft | KeyCode::MetaRight | KeyCode::Super => Some(EnigoKey::Meta),
            KeyCode::Backspace => Some(EnigoKey::Backspace),
            KeyCode::CapsLock => Some(EnigoKey::CapsLock),
            KeyCode::Delete => Some(EnigoKey::Delete),
            KeyCode::DownArrow => Some(EnigoKey::DownArrow),
            KeyCode::End => Some(EnigoKey::End),
            KeyCode::Escape => Some(EnigoKey::Escape),
            KeyCode::Home => Some(EnigoKey::Home),
            KeyCode::Insert => Some(EnigoKey::Insert),
            KeyCode::LeftArrow => Some(EnigoKey::LeftArrow),
            KeyCode::PageDown => Some(EnigoKey::PageDown),
            KeyCode::PageUp => Some(EnigoKey::PageUp),
            KeyCode::Return => Some(EnigoKey::Return),
            KeyCode::RightArrow => Some(EnigoKey::RightArrow),
            KeyCode::Space => Some(EnigoKey::Space),
            KeyCode::Tab => Some(EnigoKey::Tab),
            KeyCode::UpArrow => Some(EnigoKey::UpArrow),

            // Function keys (F1-F20 are cross-platform in enigo)
            KeyCode::F1 => Some(EnigoKey::F1),
            KeyCode::F2 => Some(EnigoKey::F2),
            KeyCode::F3 => Some(EnigoKey::F3),
            KeyCode::F4 => Some(EnigoKey::F4),
            KeyCode::F5 => Some(EnigoKey::F5),
            KeyCode::F6 => Some(EnigoKey::F6),
            KeyCode::F7 => Some(EnigoKey::F7),
            KeyCode::F8 => Some(EnigoKey::F8),
            KeyCode::F9 => Some(EnigoKey::F9),
            KeyCode::F10 => Some(EnigoKey::F10),
            KeyCode::F11 => Some(EnigoKey::F11),
            KeyCode::F12 => Some(EnigoKey::F12),
            KeyCode::F13 => Some(EnigoKey::F13),
            KeyCode::F14 => Some(EnigoKey::F14),
            KeyCode::F15 => Some(EnigoKey::F15),
            KeyCode::F16 => Some(EnigoKey::F16),
            KeyCode::F17 => Some(EnigoKey::F17),
            KeyCode::F18 => Some(EnigoKey::F18),
            KeyCode::F19 => Some(EnigoKey::F19),
            KeyCode::F20 => Some(EnigoKey::F20),
            // F21-F24 are only on certain platforms
            KeyCode::F21 => Some(EnigoKey::F21),
            KeyCode::F22 => Some(EnigoKey::F22),
            KeyCode::F23 => Some(EnigoKey::F23),
            KeyCode::F24 => Some(EnigoKey::F24),

            // Numpad keys (use unicode for cross-platform compatibility)
            KeyCode::Numpad0 => Some(EnigoKey::Unicode('0')),
            KeyCode::Numpad1 => Some(EnigoKey::Unicode('1')),
            KeyCode::Numpad2 => Some(EnigoKey::Unicode('2')),
            KeyCode::Numpad3 => Some(EnigoKey::Unicode('3')),
            KeyCode::Numpad4 => Some(EnigoKey::Unicode('4')),
            KeyCode::Numpad5 => Some(EnigoKey::Unicode('5')),
            KeyCode::Numpad6 => Some(EnigoKey::Unicode('6')),
            KeyCode::Numpad7 => Some(EnigoKey::Unicode('7')),
            KeyCode::Numpad8 => Some(EnigoKey::Unicode('8')),
            KeyCode::Numpad9 => Some(EnigoKey::Unicode('9')),
            KeyCode::NumpadAdd => Some(EnigoKey::Unicode('+')),
            KeyCode::NumpadSubtract => Some(EnigoKey::Unicode('-')),
            KeyCode::NumpadMultiply => Some(EnigoKey::Unicode('*')),
            KeyCode::NumpadDivide => Some(EnigoKey::Unicode('/')),
            KeyCode::NumpadEnter => Some(EnigoKey::Return),
            KeyCode::NumpadDecimal => Some(EnigoKey::Unicode('.')),

            // Media keys (may not work on all platforms)
            KeyCode::MediaNext => Some(EnigoKey::MediaNextTrack),
            KeyCode::MediaPrev => Some(EnigoKey::MediaPrevTrack),
            KeyCode::MediaPlayPause => Some(EnigoKey::MediaPlayPause),
            KeyCode::MediaStop => Some(EnigoKey::MediaStop),
            KeyCode::VolumeUp => Some(EnigoKey::VolumeUp),
            KeyCode::VolumeDown => Some(EnigoKey::VolumeDown),
            KeyCode::VolumeMute => Some(EnigoKey::VolumeMute),

            // Other named keys
            KeyCode::PrintScreen => Some(EnigoKey::Print),
            KeyCode::ScrollLock => Some(EnigoKey::Other(71)),
            KeyCode::Pause => Some(EnigoKey::Pause),
            KeyCode::Menu => Some(EnigoKey::LMenu),

            // Direct Unicode passthrough
            KeyCode::Unicode(c) => Some(EnigoKey::Unicode(*c)),
            // Raw/unknown
            KeyCode::Other(code) => Some(EnigoKey::Other(*code)),
        }
    }

    // ============================================================
    // Mouse button mapping: MouseButton -> enigo::Button
    // ============================================================

    fn mousebutton_to_enigo_button(btn: &MouseButton) -> EnigoButton {
        match btn {
            MouseButton::Left => EnigoButton::Left,
            MouseButton::Right => EnigoButton::Right,
            MouseButton::Middle => EnigoButton::Middle,
            _ => EnigoButton::Left,
        }
    }
}

// ============================================================
// Linux implementation using evdev
// ============================================================

#[cfg(target_os = "linux")]
pub mod os {
    use super::*;
    use evdev::{Device, EventType, InputEvent, Key as EvdevKey};

    lazy_static::lazy_static! {
        static ref IS_REMOTE: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        static ref STOP_FLAG: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    }

    pub fn stop_capture() {
        STOP_FLAG.store(true, Ordering::SeqCst);
    }

    /// Manual toggle remote mode on/off.
    /// Returns the new state.
    pub fn toggle_remote() -> bool {
        let new = !IS_REMOTE.load(Ordering::SeqCst);
        IS_REMOTE.store(new, Ordering::SeqCst);
        tracing::info!("Remote mode manually toggled to {}", if new { "ON" } else { "OFF" });
        new
    }

    /// Returns current remote mode state.
    pub fn is_remote() -> bool {
        IS_REMOTE.load(Ordering::SeqCst)
    }

    /// Enumerate evdev input devices for keyboards and mice.
    /// Opens them for passive input capture (no exclusive grab).
    fn find_input_devices() -> Vec<std::path::PathBuf> {
        let mut devices = Vec::new();
        let input_dir = std::path::Path::new("/dev/input");
        if !input_dir.exists() {
            tracing::warn!("/dev/input does not exist");
            return devices;
        }

        let Ok(entries) = std::fs::read_dir(input_dir) else {
            tracing::warn!("Cannot read /dev/input (permission denied?)");
            return devices;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            // Look for event devices (keyboards, mice, touchpads)
            if name.starts_with("event") {
                match Device::open(&path) {
                    Ok(device) => {
                        // Check if this is a keyboard, mouse, or composite device
                        let has_keys = device
                            .supported_keys()
                            .is_some_and(|keys| keys.contains(evdev::Key::KEY_A));
                        let has_mouse = device
                            .supported_relative_axes()
                            .is_some_and(|axes| axes.contains(evdev::RelativeAxisType::REL_X));

                        let device_name = device.name().unwrap_or("unknown");
                        let is_relevant = has_keys || has_mouse;

                        tracing::debug!(
                            "evdev device: {} (keys={}, mouse={}, relevant={})",
                            device_name,
                            has_keys,
                            has_mouse,
                            is_relevant
                        );

                        if is_relevant {
                            devices.push(path);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Cannot open evdev device {:?}: {}", path, e);
                    }
                }
            }
        }

        if devices.is_empty() {
            tracing::warn!(
                "No input devices found. Try:\n  \
                 1. Add yourself to the 'input' group: sudo usermod -aG input $USER\n  \
                 2. Log out and back in\n  \
                 3. Or run freemouse with sudo"
            );
        }

        devices
    }

    /// Creates a virtual uinput device for simulating input on Linux.
    fn create_uinput_device() -> Option<evdev::uinput::VirtualDevice> {
        use evdev::uinput::VirtualDeviceBuilder;

        let mut keys = evdev::AttributeSet::<evdev::Key>::new();
        for k in [
            evdev::Key::KEY_A,
            evdev::Key::KEY_B,
            evdev::Key::KEY_C,
            evdev::Key::KEY_D,
            evdev::Key::KEY_E,
            evdev::Key::KEY_F,
            evdev::Key::KEY_G,
            evdev::Key::KEY_H,
            evdev::Key::KEY_I,
            evdev::Key::KEY_J,
            evdev::Key::KEY_K,
            evdev::Key::KEY_L,
            evdev::Key::KEY_M,
            evdev::Key::KEY_N,
            evdev::Key::KEY_O,
            evdev::Key::KEY_P,
            evdev::Key::KEY_Q,
            evdev::Key::KEY_R,
            evdev::Key::KEY_S,
            evdev::Key::KEY_T,
            evdev::Key::KEY_U,
            evdev::Key::KEY_V,
            evdev::Key::KEY_W,
            evdev::Key::KEY_X,
            evdev::Key::KEY_Y,
            evdev::Key::KEY_Z,
            evdev::Key::KEY_1,
            evdev::Key::KEY_2,
            evdev::Key::KEY_3,
            evdev::Key::KEY_4,
            evdev::Key::KEY_5,
            evdev::Key::KEY_6,
            evdev::Key::KEY_7,
            evdev::Key::KEY_8,
            evdev::Key::KEY_9,
            evdev::Key::KEY_0,
            evdev::Key::KEY_LEFTALT,
            evdev::Key::KEY_RIGHTALT,
            evdev::Key::KEY_LEFTCTRL,
            evdev::Key::KEY_RIGHTCTRL,
            evdev::Key::KEY_LEFTSHIFT,
            evdev::Key::KEY_RIGHTSHIFT,
            evdev::Key::KEY_LEFTMETA,
            evdev::Key::KEY_RIGHTMETA,
            evdev::Key::KEY_UP,
            evdev::Key::KEY_DOWN,
            evdev::Key::KEY_LEFT,
            evdev::Key::KEY_RIGHT,
            evdev::Key::KEY_PAGEUP,
            evdev::Key::KEY_PAGEDOWN,
            evdev::Key::KEY_HOME,
            evdev::Key::KEY_END,
            evdev::Key::KEY_INSERT,
            evdev::Key::KEY_DELETE,
            evdev::Key::KEY_BACKSPACE,
            evdev::Key::KEY_SPACE,
            evdev::Key::KEY_TAB,
            evdev::Key::KEY_ENTER,
            evdev::Key::KEY_ESC,
            evdev::Key::KEY_CAPSLOCK,
            evdev::Key::KEY_F1,
            evdev::Key::KEY_F2,
            evdev::Key::KEY_F3,
            evdev::Key::KEY_F4,
            evdev::Key::KEY_F5,
            evdev::Key::KEY_F6,
            evdev::Key::KEY_F7,
            evdev::Key::KEY_F8,
            evdev::Key::KEY_F9,
            evdev::Key::KEY_F10,
            evdev::Key::KEY_F11,
            evdev::Key::KEY_F12,
            evdev::Key::KEY_MINUS,
            evdev::Key::KEY_EQUAL,
            evdev::Key::KEY_LEFTBRACE,
            evdev::Key::KEY_RIGHTBRACE,
            evdev::Key::KEY_BACKSLASH,
            evdev::Key::KEY_SEMICOLON,
            evdev::Key::KEY_APOSTROPHE,
            evdev::Key::KEY_COMMA,
            evdev::Key::KEY_DOT,
            evdev::Key::KEY_SLASH,
            evdev::Key::KEY_GRAVE,
            evdev::Key::KEY_KP0,
            evdev::Key::KEY_KP1,
            evdev::Key::KEY_KP2,
            evdev::Key::KEY_KP3,
            evdev::Key::KEY_KP4,
            evdev::Key::KEY_KP5,
            evdev::Key::KEY_KP6,
            evdev::Key::KEY_KP7,
            evdev::Key::KEY_KP8,
            evdev::Key::KEY_KP9,
            evdev::Key::KEY_KPPLUS,
            evdev::Key::KEY_KPMINUS,
            evdev::Key::KEY_KPASTERISK,
            evdev::Key::KEY_KPSLASH,
            evdev::Key::KEY_KPENTER,
            evdev::Key::KEY_KPDOT,
        ] {
            keys.insert(k);
        }

        let device = VirtualDeviceBuilder::new()
            .ok()?
            .name("Freemouse Virtual Input")
            .with_keys(&keys)
            .ok()?
            .build()
            .ok()?;

        Some(device)
    }

    /// Convert a Linux evdev key code to our cross-platform KeyCode
    fn evdev_key_to_keycode(ev: EvdevKey) -> Option<KeyCode> {
        Some(match ev {
            EvdevKey::KEY_A => KeyCode::A,
            EvdevKey::KEY_B => KeyCode::B,
            EvdevKey::KEY_C => KeyCode::C,
            EvdevKey::KEY_D => KeyCode::D,
            EvdevKey::KEY_E => KeyCode::E,
            EvdevKey::KEY_F => KeyCode::F,
            EvdevKey::KEY_G => KeyCode::G,
            EvdevKey::KEY_H => KeyCode::H,
            EvdevKey::KEY_I => KeyCode::I,
            EvdevKey::KEY_J => KeyCode::J,
            EvdevKey::KEY_K => KeyCode::K,
            EvdevKey::KEY_L => KeyCode::L,
            EvdevKey::KEY_M => KeyCode::M,
            EvdevKey::KEY_N => KeyCode::N,
            EvdevKey::KEY_O => KeyCode::O,
            EvdevKey::KEY_P => KeyCode::P,
            EvdevKey::KEY_Q => KeyCode::Q,
            EvdevKey::KEY_R => KeyCode::R,
            EvdevKey::KEY_S => KeyCode::S,
            EvdevKey::KEY_T => KeyCode::T,
            EvdevKey::KEY_U => KeyCode::U,
            EvdevKey::KEY_V => KeyCode::V,
            EvdevKey::KEY_W => KeyCode::W,
            EvdevKey::KEY_X => KeyCode::X,
            EvdevKey::KEY_Y => KeyCode::Y,
            EvdevKey::KEY_Z => KeyCode::Z,
            EvdevKey::KEY_1 => KeyCode::Num1,
            EvdevKey::KEY_2 => KeyCode::Num2,
            EvdevKey::KEY_3 => KeyCode::Num3,
            EvdevKey::KEY_4 => KeyCode::Num4,
            EvdevKey::KEY_5 => KeyCode::Num5,
            EvdevKey::KEY_6 => KeyCode::Num6,
            EvdevKey::KEY_7 => KeyCode::Num7,
            EvdevKey::KEY_8 => KeyCode::Num8,
            EvdevKey::KEY_9 => KeyCode::Num9,
            EvdevKey::KEY_0 => KeyCode::Num0,
            EvdevKey::KEY_LEFTALT => KeyCode::Alt,
            EvdevKey::KEY_RIGHTALT => KeyCode::AltGr,
            EvdevKey::KEY_LEFTCTRL => KeyCode::ControlLeft,
            EvdevKey::KEY_RIGHTCTRL => KeyCode::ControlRight,
            EvdevKey::KEY_LEFTSHIFT => KeyCode::ShiftLeft,
            EvdevKey::KEY_RIGHTSHIFT => KeyCode::ShiftRight,
            EvdevKey::KEY_LEFTMETA => KeyCode::MetaLeft,
            EvdevKey::KEY_RIGHTMETA => KeyCode::MetaRight,
            EvdevKey::KEY_UP => KeyCode::UpArrow,
            EvdevKey::KEY_DOWN => KeyCode::DownArrow,
            EvdevKey::KEY_LEFT => KeyCode::LeftArrow,
            EvdevKey::KEY_RIGHT => KeyCode::RightArrow,
            EvdevKey::KEY_PAGEUP => KeyCode::PageUp,
            EvdevKey::KEY_PAGEDOWN => KeyCode::PageDown,
            EvdevKey::KEY_HOME => KeyCode::Home,
            EvdevKey::KEY_END => KeyCode::End,
            EvdevKey::KEY_INSERT => KeyCode::Insert,
            EvdevKey::KEY_DELETE => KeyCode::Delete,
            EvdevKey::KEY_BACKSPACE => KeyCode::Backspace,
            EvdevKey::KEY_SPACE => KeyCode::Space,
            EvdevKey::KEY_TAB => KeyCode::Tab,
            EvdevKey::KEY_ENTER => KeyCode::Return,
            EvdevKey::KEY_ESC => KeyCode::Escape,
            EvdevKey::KEY_CAPSLOCK => KeyCode::CapsLock,
            EvdevKey::KEY_F1 => KeyCode::F1,
            EvdevKey::KEY_F2 => KeyCode::F2,
            EvdevKey::KEY_F3 => KeyCode::F3,
            EvdevKey::KEY_F4 => KeyCode::F4,
            EvdevKey::KEY_F5 => KeyCode::F5,
            EvdevKey::KEY_F6 => KeyCode::F6,
            EvdevKey::KEY_F7 => KeyCode::F7,
            EvdevKey::KEY_F8 => KeyCode::F8,
            EvdevKey::KEY_F9 => KeyCode::F9,
            EvdevKey::KEY_F10 => KeyCode::F10,
            EvdevKey::KEY_F11 => KeyCode::F11,
            EvdevKey::KEY_F12 => KeyCode::F12,
            EvdevKey::KEY_MINUS => KeyCode::Minus,
            EvdevKey::KEY_EQUAL => KeyCode::Equals,
            EvdevKey::KEY_LEFTBRACE => KeyCode::LeftBracket,
            EvdevKey::KEY_RIGHTBRACE => KeyCode::RightBracket,
            EvdevKey::KEY_BACKSLASH => KeyCode::Backslash,
            EvdevKey::KEY_SEMICOLON => KeyCode::Semicolon,
            EvdevKey::KEY_APOSTROPHE => KeyCode::Quote,
            EvdevKey::KEY_COMMA => KeyCode::Comma,
            EvdevKey::KEY_DOT => KeyCode::Period,
            EvdevKey::KEY_SLASH => KeyCode::Slash,
            EvdevKey::KEY_GRAVE => KeyCode::Backtick,
            EvdevKey::KEY_KP0 => KeyCode::Numpad0,
            EvdevKey::KEY_KP1 => KeyCode::Numpad1,
            EvdevKey::KEY_KP2 => KeyCode::Numpad2,
            EvdevKey::KEY_KP3 => KeyCode::Numpad3,
            EvdevKey::KEY_KP4 => KeyCode::Numpad4,
            EvdevKey::KEY_KP5 => KeyCode::Numpad5,
            EvdevKey::KEY_KP6 => KeyCode::Numpad6,
            EvdevKey::KEY_KP7 => KeyCode::Numpad7,
            EvdevKey::KEY_KP8 => KeyCode::Numpad8,
            EvdevKey::KEY_KP9 => KeyCode::Numpad9,
            EvdevKey::KEY_KPPLUS => KeyCode::NumpadAdd,
            EvdevKey::KEY_KPMINUS => KeyCode::NumpadSubtract,
            EvdevKey::KEY_KPASTERISK => KeyCode::NumpadMultiply,
            EvdevKey::KEY_KPSLASH => KeyCode::NumpadDivide,
            EvdevKey::KEY_KPENTER => KeyCode::NumpadEnter,
            EvdevKey::KEY_KPDOT => KeyCode::NumpadDecimal,
            EvdevKey::KEY_SYSRQ => KeyCode::PrintScreen,
            EvdevKey::KEY_SCROLLLOCK => KeyCode::ScrollLock,
            EvdevKey::KEY_PAUSE => KeyCode::Pause,
            EvdevKey::KEY_COMPOSE => KeyCode::Menu,
            _ => return None,
        })
    }

    /// Convert our cross-platform KeyCode to evdev key code for simulation
    fn keycode_to_evdev_key(kc: &KeyCode) -> Option<EvdevKey> {
        Some(match kc {
            KeyCode::A => EvdevKey::KEY_A,
            KeyCode::B => EvdevKey::KEY_B,
            KeyCode::C => EvdevKey::KEY_C,
            KeyCode::D => EvdevKey::KEY_D,
            KeyCode::E => EvdevKey::KEY_E,
            KeyCode::F => EvdevKey::KEY_F,
            KeyCode::G => EvdevKey::KEY_G,
            KeyCode::H => EvdevKey::KEY_H,
            KeyCode::I => EvdevKey::KEY_I,
            KeyCode::J => EvdevKey::KEY_J,
            KeyCode::K => EvdevKey::KEY_K,
            KeyCode::L => EvdevKey::KEY_L,
            KeyCode::M => EvdevKey::KEY_M,
            KeyCode::N => EvdevKey::KEY_N,
            KeyCode::O => EvdevKey::KEY_O,
            KeyCode::P => EvdevKey::KEY_P,
            KeyCode::Q => EvdevKey::KEY_Q,
            KeyCode::R => EvdevKey::KEY_R,
            KeyCode::S => EvdevKey::KEY_S,
            KeyCode::T => EvdevKey::KEY_T,
            KeyCode::U => EvdevKey::KEY_U,
            KeyCode::V => EvdevKey::KEY_V,
            KeyCode::W => EvdevKey::KEY_W,
            KeyCode::X => EvdevKey::KEY_X,
            KeyCode::Y => EvdevKey::KEY_Y,
            KeyCode::Z => EvdevKey::KEY_Z,
            KeyCode::Num0 => EvdevKey::KEY_0,
            KeyCode::Num1 => EvdevKey::KEY_1,
            KeyCode::Num2 => EvdevKey::KEY_2,
            KeyCode::Num3 => EvdevKey::KEY_3,
            KeyCode::Num4 => EvdevKey::KEY_4,
            KeyCode::Num5 => EvdevKey::KEY_5,
            KeyCode::Num6 => EvdevKey::KEY_6,
            KeyCode::Num7 => EvdevKey::KEY_7,
            KeyCode::Num8 => EvdevKey::KEY_8,
            KeyCode::Num9 => EvdevKey::KEY_9,
            KeyCode::Alt => EvdevKey::KEY_LEFTALT,
            KeyCode::AltGr => EvdevKey::KEY_RIGHTALT,
            KeyCode::ControlLeft => EvdevKey::KEY_LEFTCTRL,
            KeyCode::ControlRight => EvdevKey::KEY_RIGHTCTRL,
            KeyCode::ShiftLeft => EvdevKey::KEY_LEFTSHIFT,
            KeyCode::ShiftRight => EvdevKey::KEY_RIGHTSHIFT,
            KeyCode::MetaLeft | KeyCode::Super => EvdevKey::KEY_LEFTMETA,
            KeyCode::MetaRight => EvdevKey::KEY_RIGHTMETA,
            KeyCode::UpArrow => EvdevKey::KEY_UP,
            KeyCode::DownArrow => EvdevKey::KEY_DOWN,
            KeyCode::LeftArrow => EvdevKey::KEY_LEFT,
            KeyCode::RightArrow => EvdevKey::KEY_RIGHT,
            KeyCode::PageUp => EvdevKey::KEY_PAGEUP,
            KeyCode::PageDown => EvdevKey::KEY_PAGEDOWN,
            KeyCode::Home => EvdevKey::KEY_HOME,
            KeyCode::End => EvdevKey::KEY_END,
            KeyCode::Insert => EvdevKey::KEY_INSERT,
            KeyCode::Delete => EvdevKey::KEY_DELETE,
            KeyCode::Backspace => EvdevKey::KEY_BACKSPACE,
            KeyCode::Space => EvdevKey::KEY_SPACE,
            KeyCode::Tab => EvdevKey::KEY_TAB,
            KeyCode::Return | KeyCode::NumpadEnter => EvdevKey::KEY_ENTER,
            KeyCode::Escape => EvdevKey::KEY_ESC,
            KeyCode::CapsLock => EvdevKey::KEY_CAPSLOCK,
            KeyCode::F1 => EvdevKey::KEY_F1,
            KeyCode::F2 => EvdevKey::KEY_F2,
            KeyCode::F3 => EvdevKey::KEY_F3,
            KeyCode::F4 => EvdevKey::KEY_F4,
            KeyCode::F5 => EvdevKey::KEY_F5,
            KeyCode::F6 => EvdevKey::KEY_F6,
            KeyCode::F7 => EvdevKey::KEY_F7,
            KeyCode::F8 => EvdevKey::KEY_F8,
            KeyCode::F9 => EvdevKey::KEY_F9,
            KeyCode::F10 => EvdevKey::KEY_F10,
            KeyCode::F11 => EvdevKey::KEY_F11,
            KeyCode::F12 => EvdevKey::KEY_F12,
            KeyCode::Minus => EvdevKey::KEY_MINUS,
            KeyCode::Equals => EvdevKey::KEY_EQUAL,
            KeyCode::LeftBracket => EvdevKey::KEY_LEFTBRACE,
            KeyCode::RightBracket => EvdevKey::KEY_RIGHTBRACE,
            KeyCode::Backslash => EvdevKey::KEY_BACKSLASH,
            KeyCode::Semicolon => EvdevKey::KEY_SEMICOLON,
            KeyCode::Quote => EvdevKey::KEY_APOSTROPHE,
            KeyCode::Comma => EvdevKey::KEY_COMMA,
            KeyCode::Period => EvdevKey::KEY_DOT,
            KeyCode::Slash => EvdevKey::KEY_SLASH,
            KeyCode::Backtick => EvdevKey::KEY_GRAVE,
            KeyCode::Numpad0 => EvdevKey::KEY_KP0,
            KeyCode::Numpad1 => EvdevKey::KEY_KP1,
            KeyCode::Numpad2 => EvdevKey::KEY_KP2,
            KeyCode::Numpad3 => EvdevKey::KEY_KP3,
            KeyCode::Numpad4 => EvdevKey::KEY_KP4,
            KeyCode::Numpad5 => EvdevKey::KEY_KP5,
            KeyCode::Numpad6 => EvdevKey::KEY_KP6,
            KeyCode::Numpad7 => EvdevKey::KEY_KP7,
            KeyCode::Numpad8 => EvdevKey::KEY_KP8,
            KeyCode::Numpad9 => EvdevKey::KEY_KP9,
            KeyCode::NumpadAdd => EvdevKey::KEY_KPPLUS,
            KeyCode::NumpadSubtract => EvdevKey::KEY_KPMINUS,
            KeyCode::NumpadMultiply => EvdevKey::KEY_KPASTERISK,
            KeyCode::NumpadDivide => EvdevKey::KEY_KPSLASH,
            KeyCode::NumpadDecimal => EvdevKey::KEY_KPDOT,
            KeyCode::PrintScreen => EvdevKey::KEY_SYSRQ,
            KeyCode::ScrollLock => EvdevKey::KEY_SCROLLLOCK,
            KeyCode::Pause => EvdevKey::KEY_PAUSE,
            KeyCode::Menu => EvdevKey::KEY_COMPOSE,
            // For Unicode and Other, try to find a best match or return None
            KeyCode::Unicode(c) => match c {
                'a'..='z' | 'A'..='Z' => {
                    let idx = c.to_ascii_lowercase() as u8 - b'a';
                    return Some(match idx {
                        0 => EvdevKey::KEY_A,
                        1 => EvdevKey::KEY_B,
                        2 => EvdevKey::KEY_C,
                        3 => EvdevKey::KEY_D,
                        4 => EvdevKey::KEY_E,
                        5 => EvdevKey::KEY_F,
                        6 => EvdevKey::KEY_G,
                        7 => EvdevKey::KEY_H,
                        8 => EvdevKey::KEY_I,
                        9 => EvdevKey::KEY_J,
                        10 => EvdevKey::KEY_K,
                        11 => EvdevKey::KEY_L,
                        12 => EvdevKey::KEY_M,
                        13 => EvdevKey::KEY_N,
                        14 => EvdevKey::KEY_O,
                        15 => EvdevKey::KEY_P,
                        16 => EvdevKey::KEY_Q,
                        17 => EvdevKey::KEY_R,
                        18 => EvdevKey::KEY_S,
                        19 => EvdevKey::KEY_T,
                        20 => EvdevKey::KEY_U,
                        21 => EvdevKey::KEY_V,
                        22 => EvdevKey::KEY_W,
                        23 => EvdevKey::KEY_X,
                        24 => EvdevKey::KEY_Y,
                        25 => EvdevKey::KEY_Z,
                        _ => return None,
                    });
                }
                '0'..='9' => {
                    return Some(match c {
                        '0' => EvdevKey::KEY_0,
                        '1' => EvdevKey::KEY_1,
                        '2' => EvdevKey::KEY_2,
                        '3' => EvdevKey::KEY_3,
                        '4' => EvdevKey::KEY_4,
                        '5' => EvdevKey::KEY_5,
                        '6' => EvdevKey::KEY_6,
                        '7' => EvdevKey::KEY_7,
                        '8' => EvdevKey::KEY_8,
                        '9' => EvdevKey::KEY_9,
                        _ => return None,
                    });
                }
                _ => return None,
            },
            KeyCode::Option => EvdevKey::KEY_LEFTALT,
            KeyCode::F13
            | KeyCode::F14
            | KeyCode::F15
            | KeyCode::F16
            | KeyCode::F17
            | KeyCode::F18
            | KeyCode::F19
            | KeyCode::F20
            | KeyCode::F21
            | KeyCode::F22
            | KeyCode::F23
            | KeyCode::F24 => return None, // Not all F-keys map cleanly
            KeyCode::MediaNext
            | KeyCode::MediaPrev
            | KeyCode::MediaPlayPause
            | KeyCode::MediaStop
            | KeyCode::VolumeUp
            | KeyCode::VolumeDown
            | KeyCode::VolumeMute => return None,
            KeyCode::Other(_) => return None,
        })
    }

    /// Detect edge hits using evdev RELATIVE event patterns.
    /// Works on both X11 and Wayland without querying the display server.
    ///
    /// When the cursor hits the screen edge, the display server clamps it,
    /// but the physical mouse can keep moving. This produces a sustained
    /// stream of same-direction REL events. We detect this "pushing against
    /// the wall" pattern by counting consecutive same-sign values.
    ///
    /// For ABSOLUTE devices (touchpads), we use direct position comparison.
    struct EdgeDetector {
        /// Consecutive positive (rightward) REL_X events
        right_streak: u32,
        /// Consecutive negative (leftward) REL_X events
        left_streak: u32,
        /// Events of same sign needed to trigger
        threshold: u32,
        /// Whether we're currently in "pushing" state (prevents re-trigger)
        was_right_edge: bool,
        was_left_edge: bool,
    }

    impl EdgeDetector {
        fn new(threshold: u32) -> Self {
            Self {
                right_streak: 0,
                left_streak: 0,
                threshold,
                was_right_edge: false,
                was_left_edge: false,
            }
        }

        /// Feed a REL_X value. Returns `Some(true)` if right edge hit,
        /// `Some(false)` if left edge hit, `None` otherwise.
        fn feed_rel_x(&mut self, value: i32) -> Option<bool> {
            if value > 0 {
                self.right_streak += 1;
                self.left_streak = 0;
                if self.right_streak >= self.threshold && !self.was_right_edge {
                    self.was_right_edge = true;
                    self.was_left_edge = false;
                    return Some(true);
                }
            } else if value < 0 {
                self.left_streak += 1;
                self.right_streak = 0;
                if self.left_streak >= self.threshold && !self.was_left_edge {
                    self.was_left_edge = true;
                    self.was_right_edge = false;
                    return Some(false);
                }
            } else {
                // Zero value — no movement, don't reset streaks (mouse paused at edge)
            }
            None
        }

        fn reset(&mut self) {
            self.right_streak = 0;
            self.left_streak = 0;
            self.was_right_edge = false;
            self.was_left_edge = false;
        }
    }

    /// Start evdev-based input capture on Linux.
    /// No exclusive grab — the local cursor always works.
    /// Mouse Without Borders style: move mouse to right edge to transition
    /// to the remote machine; move to left edge to come back.
    /// Edge detection uses velocity-pattern analysis on REL events
    /// (works on X11 and Wayland alike).
    pub fn start_capture(tx: mpsc::Sender<NetworkEvent>, screen_width: f64) {
        IS_REMOTE.store(false, Ordering::SeqCst);
        STOP_FLAG.store(false, Ordering::SeqCst);

        let is_remote = IS_REMOTE.clone();
        let stop_flag = STOP_FLAG.clone();

        std::thread::spawn(move || {
            let devices = find_input_devices();
            if devices.is_empty() {
                tracing::warn!("No evdev input devices found for capture");
                return;
            }

            let mut opened_devices: Vec<Device> = Vec::new();
            for path in &devices {
                match Device::open(path) {
                    Ok(device) => {
                        tracing::info!(
                            "Opened evdev device: {} (passive, no grab)",
                            device.name().unwrap_or("unknown")
                        );
                        opened_devices.push(device);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to open evdev device {:?}: {}", path, e);
                    }
                }
            }

            if opened_devices.is_empty() {
                return;
            }

            let mut edge_detector = EdgeDetector::new(3);
            let edge_threshold = 5.0;
            let mut last_abs_x = 0.0_f64;

            tracing::info!(
                "Capture started: {} device(s), threshold={}, edge_px={}",
                opened_devices.len(),
                3,
                edge_threshold,
            );

            loop {
                if stop_flag.load(Ordering::SeqCst) {
                    tracing::info!("Capture stopped");
                    break;
                }

                for device in &mut opened_devices {
                    match device.fetch_events() {
                        Ok(events) => {
                            for event in events {
                                handle_evdev_event(
                                    &event,
                                    &tx,
                                    &is_remote,
                                    screen_width,
                                    &mut edge_detector,
                                    edge_threshold,
                                    &mut last_abs_x,
                                );
                            }
                        }
                        Err(e) => {
                            if e.kind() != std::io::ErrorKind::WouldBlock {
                                tracing::warn!("evdev fetch error: {}", e);
                            }
                            break;
                        }
                    }
                }

                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        });
    }

    fn handle_evdev_event(
        event: &InputEvent,
        tx: &mpsc::Sender<NetworkEvent>,
        is_remote: &AtomicBool,
        screen_width: f64,
        edge: &mut EdgeDetector,
        edge_threshold: f64,
        last_abs_x: &mut f64,
    ) {
        let currently_remote = is_remote.load(Ordering::SeqCst);

        match event.event_type() {
            EventType::RELATIVE => {
                let value = event.value();
                match event.code() {
                    0 => {
                        // REL_X — feed into velocity-based edge detector
                        if let Some(is_right) = edge.feed_rel_x(value) {
                            if is_right && !currently_remote {
                                is_remote.store(true, Ordering::SeqCst);
                                tracing::info!("Remote ON (REL right edge)");
                            } else if !is_right && currently_remote {
                                is_remote.store(false, Ordering::SeqCst);
                                tracing::info!("Remote OFF (REL left edge)");
                            }
                        }
                        let remote_now = is_remote.load(Ordering::SeqCst);
                        if remote_now {
                            let _ = tx.blocking_send(NetworkEvent::MouseMoveRelative(
                                value as f64, 0.0,
                            ));
                        }
                    }
                    1 => {
                        // REL_Y
                        let remote_now = is_remote.load(Ordering::SeqCst);
                        if remote_now {
                            let _ = tx.blocking_send(NetworkEvent::MouseMoveRelative(
                                0.0, value as f64,
                            ));
                        }
                    }
                    8 => {
                        let remote_now = is_remote.load(Ordering::SeqCst);
                        if remote_now {
                            let _ = tx.blocking_send(NetworkEvent::MouseScroll(0, value));
                        }
                    }
                    _ => {}
                }
            }
            EventType::ABSOLUTE => {
                let val = event.value() as f64;
                match event.code() {
                    0 => {
                        // ABS_X
                        *last_abs_x = val;
                        if !currently_remote && val >= screen_width - edge_threshold {
                            is_remote.store(true, Ordering::SeqCst);
                            edge.reset();
                            tracing::info!("Remote ON (ABS X {:.0})", val);
                        } else if currently_remote && val <= edge_threshold {
                            is_remote.store(false, Ordering::SeqCst);
                            edge.reset();
                            tracing::info!("Remote OFF (ABS X {:.0})", val);
                        }
                        let remote_now = is_remote.load(Ordering::SeqCst);
                        if remote_now {
                            let _ = tx.blocking_send(NetworkEvent::MouseMoved(val, *last_abs_x));
                        }
                    }
                    1 => {
                        // ABS_Y
                        let remote_now = is_remote.load(Ordering::SeqCst);
                        if remote_now {
                            let _ = tx.blocking_send(NetworkEvent::MouseMoved(*last_abs_x, val));
                        }
                    }
                    _ => {}
                }
            }
            EventType::KEY => {
                let key = EvdevKey::new(event.code());
                let pressed = event.value() != 0;
                let remote_now = is_remote.load(Ordering::SeqCst);

                if remote_now {
                    if let Some(kc) = evdev_key_to_keycode(key) {
                        if pressed {
                            let _ = tx.blocking_send(NetworkEvent::KeyDown(kc));
                        } else {
                            let _ = tx.blocking_send(NetworkEvent::KeyUp(kc));
                        }
                    }
                }
            }
            EventType::SYNCHRONIZATION => {}
            _ => {}
        }
    }

    /// Start input simulation on Linux using uinput virtual devices.
    pub async fn start_simulation(mut rx: mpsc::Receiver<NetworkEvent>) {
        let mut uinput_dev = match create_uinput_device() {
            Some(d) => d,
            None => {
                tracing::error!("Failed to create uinput virtual device");
                return;
            }
        };

        while let Some(event) = rx.recv().await {
            match event {
                NetworkEvent::MouseMoved(x, y) => {
                    let _ = uinput_dev.emit(&[
                        InputEvent::new(EventType::ABSOLUTE, 0, x as i32), // ABS_X
                        InputEvent::new(EventType::ABSOLUTE, 1, y as i32), // ABS_Y
                        InputEvent::new(EventType::SYNCHRONIZATION, 0, 0), // SYN_REPORT
                    ]);
                }
                NetworkEvent::MouseMoveRelative(dx, dy) => {
                    let _ = uinput_dev.emit(&[
                        InputEvent::new(EventType::RELATIVE, 0, dx as i32), // REL_X
                        InputEvent::new(EventType::RELATIVE, 1, dy as i32), // REL_Y
                        InputEvent::new(EventType::SYNCHRONIZATION, 0, 0),
                    ]);
                }
                NetworkEvent::MouseButtonDown(btn) => {
                    let code = mousebutton_to_evdev_code(&btn);
                    let _ = uinput_dev.emit(&[
                        InputEvent::new(EventType::KEY, code, 1),
                        InputEvent::new(EventType::SYNCHRONIZATION, 0, 0),
                    ]);
                }
                NetworkEvent::MouseButtonUp(btn) => {
                    let code = mousebutton_to_evdev_code(&btn);
                    let _ = uinput_dev.emit(&[
                        InputEvent::new(EventType::KEY, code, 0),
                        InputEvent::new(EventType::SYNCHRONIZATION, 0, 0),
                    ]);
                }
                NetworkEvent::MouseScroll(_dx, dy) => {
                    let _ = uinput_dev.emit(&[
                        InputEvent::new(EventType::RELATIVE, 8, dy), // REL_WHEEL
                        InputEvent::new(EventType::SYNCHRONIZATION, 0, 0),
                    ]);
                }
                NetworkEvent::KeyDown(kc) => {
                    if let Some(ev) = keycode_to_evdev_key(&kc) {
                        let _ = uinput_dev.emit(&[
                            InputEvent::new(EventType::KEY, ev.code(), 1),
                            InputEvent::new(EventType::SYNCHRONIZATION, 0, 0),
                        ]);
                    }
                }
                NetworkEvent::KeyUp(kc) => {
                    if let Some(ev) = keycode_to_evdev_key(&kc) {
                        let _ = uinput_dev.emit(&[
                            InputEvent::new(EventType::KEY, ev.code(), 0),
                            InputEvent::new(EventType::SYNCHRONIZATION, 0, 0),
                        ]);
                    }
                }
                _ => {}
            }
        }
    }

    fn mousebutton_to_evdev_code(btn: &MouseButton) -> u16 {
        match btn {
            MouseButton::Left => 0x110,   // BTN_LEFT
            MouseButton::Right => 0x111,  // BTN_RIGHT
            MouseButton::Middle => 0x112, // BTN_MIDDLE
            MouseButton::X1 => 0x113,     // BTN_SIDE
            MouseButton::X2 => 0x114,     // BTN_EXTRA
        }
    }

    #[cfg(test)]
    mod tests {
        use super::EdgeDetector;

        #[test]
        fn edge_detector_noise_does_not_trigger() {
            let mut d = EdgeDetector::new(10);
            // Random noise — alternating directions, should not trigger
            for _ in 0..100 {
                assert!(d.feed_rel_x(1).is_none());
                assert!(d.feed_rel_x(-1).is_none());
            }
        }

        #[test]
        fn edge_detector_right_streak_triggers() {
            let mut d = EdgeDetector::new(5);
            // 4 same-sign events: not enough
            for i in 0..4 {
                assert!(d.feed_rel_x(1).is_none(), "failed at i={}", i);
            }
            // 5th event triggers
            assert_eq!(d.feed_rel_x(1), Some(true));
        }

        #[test]
        fn edge_detector_left_streak_triggers() {
            let mut d = EdgeDetector::new(5);
            for i in 0..4 {
                assert!(d.feed_rel_x(-1).is_none(), "failed at i={}", i);
            }
            assert_eq!(d.feed_rel_x(-1), Some(false));
        }

        #[test]
        fn edge_detector_direction_change_resets() {
            let mut d = EdgeDetector::new(10);
            // Build up right streak
            for _ in 0..9 {
                assert!(d.feed_rel_x(1).is_none());
            }
            // Change direction — resets streak
            assert!(d.feed_rel_x(-1).is_none());
            // Need 10 more right events from scratch
            for _ in 0..9 {
                assert!(d.feed_rel_x(1).is_none());
            }
            assert_eq!(d.feed_rel_x(1), Some(true));
        }

        #[test]
        fn edge_detector_no_double_trigger() {
            let mut d = EdgeDetector::new(3);
            assert_eq!(d.feed_rel_x(1), None);
            assert_eq!(d.feed_rel_x(1), None);
            assert_eq!(d.feed_rel_x(1), Some(true));
            // More same-direction events should not re-trigger
            for _ in 0..20 {
                assert!(d.feed_rel_x(1).is_none());
            }
        }

        #[test]
        fn edge_detector_reset_works() {
            let mut d = EdgeDetector::new(3);
            assert_eq!(d.feed_rel_x(1), None);
            assert_eq!(d.feed_rel_x(1), None);
            assert_eq!(d.feed_rel_x(1), Some(true));
            d.reset();
            // After reset, should trigger again
            assert_eq!(d.feed_rel_x(1), None);
            assert_eq!(d.feed_rel_x(1), None);
            assert_eq!(d.feed_rel_x(1), Some(true));
        }

        #[test]
        fn edge_detector_zero_does_not_reset_streak() {
            let mut d = EdgeDetector::new(4);
            assert!(d.feed_rel_x(1).is_none());
            assert!(d.feed_rel_x(1).is_none());
            // Zero (no movement) should not break the streak
            assert!(d.feed_rel_x(0).is_none());
            assert!(d.feed_rel_x(1).is_none());
            assert_eq!(d.feed_rel_x(1), Some(true));
        }
    }
}
