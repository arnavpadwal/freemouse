use argon2::Argon2;
use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    ChaCha20Poly1305, Key, Nonce,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};

/// Virtual key code - cross-platform key representation.
/// Covers all common keys from both rdev and enigo.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum KeyCode {
    // Letters
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    // Numbers (top row)
    Num0,
    Num1,
    Num2,
    Num3,
    Num4,
    Num5,
    Num6,
    Num7,
    Num8,
    Num9,
    // Modifiers
    Alt,
    AltGr,
    ControlLeft,
    ControlRight,
    ShiftLeft,
    ShiftRight,
    MetaLeft,
    MetaRight,
    Super,
    Option,
    // Navigation
    UpArrow,
    DownArrow,
    LeftArrow,
    RightArrow,
    PageUp,
    PageDown,
    Home,
    End,
    Insert,
    Delete,
    Backspace,
    Space,
    Tab,
    Return,
    Escape,
    CapsLock,
    // Function keys
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
    F21,
    F22,
    F23,
    F24,
    // Symbols (US keyboard layout)
    Minus,
    Equals,
    LeftBracket,
    RightBracket,
    Backslash,
    Semicolon,
    Quote,
    Comma,
    Period,
    Slash,
    Backtick,
    // Numpad
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    NumpadAdd,
    NumpadSubtract,
    NumpadMultiply,
    NumpadDivide,
    NumpadEnter,
    NumpadDecimal,
    // Other
    PrintScreen,
    ScrollLock,
    Pause,
    Menu,
    MediaNext,
    MediaPrev,
    MediaPlayPause,
    MediaStop,
    VolumeUp,
    VolumeDown,
    VolumeMute,
    /// Catch-all: send a Unicode character directly
    Unicode(char),
    /// Raw platform keycode fallback (HID usage code or similar)
    Other(u32),
}

/// Mouse button representation
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

/// Events exchanged between share and receive machines
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum NetworkEvent {
    /// Absolute mouse move (x, y) in screen coordinates
    MouseMoved(f64, f64),
    /// Relative mouse move (dx, dy)
    MouseMoveRelative(f64, f64),
    /// Button down with specific button
    MouseButtonDown(MouseButton),
    /// Button up with specific button
    MouseButtonUp(MouseButton),
    /// Scroll (delta_x, delta_y)
    MouseScroll(i32, i32),
    /// Key down with virtual key code
    KeyDown(KeyCode),
    /// Key up with virtual key code
    KeyUp(KeyCode),
    /// Clipboard text sync
    ClipboardText(String),
    /// Keep-alive / ping (empty payload, just to detect disconnection)
    KeepAlive,
}

/// TCP connection timeout
const CONNECTION_TIMEOUT_SECS: u64 = 10;
/// Keep-alive interval
const KEEPALIVE_INTERVAL_SECS: u64 = 2;
/// Maximum frame size (10 MB)
const MAX_FRAME_SIZE: u32 = 10 * 1024 * 1024;

/// Derives an encryption key from a PIN and salt using Argon2.
fn derive_key(pin: &str, salt: &[u8; 16]) -> Result<Key, Box<dyn std::error::Error + Send + Sync>> {
    let argon2 = Argon2::default();
    let mut key_bytes = [0u8; 32];
    argon2
        .hash_password_into(pin.as_bytes(), salt, &mut key_bytes)
        .map_err(|e| format!("Argon2 error: {}", e))?;
    Ok(*Key::from_slice(&key_bytes))
}

/// Sends an encrypted frame over an async writer.
pub async fn send_encrypted<W: AsyncWriteExt + Unpin>(
    stream: &mut W,
    cipher: &ChaCha20Poly1305,
    event: &NetworkEvent,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let serialized = bincode::serialize(event)?;
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let encrypted = cipher
        .encrypt(&nonce, serialized.as_ref())
        .map_err(|e| format!("Encryption error: {:?}", e))?;

    let frame_len = (12 + encrypted.len()) as u32;
    stream.write_u32(frame_len).await?;
    stream.write_all(&nonce).await?;
    stream.write_all(&encrypted).await?;
    Ok(())
}

/// Receives an encrypted frame from an async reader.
pub async fn recv_encrypted<R: AsyncReadExt + Unpin>(
    stream: &mut R,
    cipher: &ChaCha20Poly1305,
) -> Result<NetworkEvent, Box<dyn std::error::Error + Send + Sync>> {
    let frame_len = stream.read_u32().await?;
    if !(12..=MAX_FRAME_SIZE).contains(&frame_len) {
        return Err("Invalid frame length".into());
    }
    let mut nonce_bytes = [0u8; 12];
    stream.read_exact(&mut nonce_bytes).await?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let payload_len = (frame_len - 12) as usize;
    let mut encrypted = vec![0u8; payload_len];
    stream.read_exact(&mut encrypted).await?;
    let decrypted = cipher
        .decrypt(nonce, encrypted.as_ref())
        .map_err(|e| format!("Decryption error: {:?}", e))?;
    let event: NetworkEvent = bincode::deserialize(&decrypted)?;
    Ok(event)
}

/// A connected session (either server or client side).
pub struct Connection {
    pub stream: TcpStream,
    pub cipher: ChaCha20Poly1305,
}

/// Starts Share mode (server). Listens on the given port and accepts one connection.
pub async fn start_server(
    port: u16,
    pin: &str,
) -> Result<Connection, Box<dyn std::error::Error + Send + Sync>> {
    let mut attempts = 0;
    const MAX_ATTEMPTS: usize = 5;
    
    loop {
        attempts += 1;
        match tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await {
            Ok(listener) => {
                // Server is now listening on port
                tracing::info!("Server listening on 0.0.0.0:{}", port);
                
                // Accept with timeout
                let (mut stream, _addr) = timeout(
                    Duration::from_secs(CONNECTION_TIMEOUT_SECS),
                    listener.accept(),
                )
                .await
                .map_err(|_| "Connection timeout: no receiver connected")??;

                // Application-level keep-alive pings handle disconnect detection
                // TCP keepalive not set here to keep cross-platform compatibility simple

                let mut salt = [0u8; 16];
                rand::Rng::fill(&mut rand::thread_rng(), &mut salt);
                stream.write_all(&salt).await?;

                let key = derive_key(pin, &salt)?;
                let cipher = ChaCha20Poly1305::new(&key);

                return Ok(Connection { stream, cipher });
            }
            Err(e) => {
                if attempts >= MAX_ATTEMPTS {
                    return Err(format!("Failed to bind to port {} after {} attempts: {}", port, MAX_ATTEMPTS, e).into());
                }
                tracing::warn!("Port {} bind attempt {}/{} failed: {}", port, attempts, MAX_ATTEMPTS, e);
                tokio::time::sleep(Duration::from_millis(1000 * attempts as u64)).await;
            }
        }
    }
}

/// Starts Receive mode (client). Connects to the given IP:port.
pub async fn start_client(
    ip: &str,
    port: u16,
    pin: &str,
) -> Result<Connection, Box<dyn std::error::Error + Send + Sync>> {
    let mut attempts = 0;
    const MAX_ATTEMPTS: usize = 3;
    
    // Try IPv4 first, then IPv6
    let addresses = [
        format!("[{}]:{}", ip, port), // IPv6 format
        format!("{}:{}", ip, port),   // IPv4 format
    ];
    
    for address in addresses {
        for retry in 1..=MAX_ATTEMPTS {
            attempts += 1;
            match timeout(
                Duration::from_secs(CONNECTION_TIMEOUT_SECS),
                TcpStream::connect(address.clone()),
            )
            .await {
                Ok(Ok(mut stream)) => {
                    let mut salt = [0u8; 16];
                    stream.read_exact(&mut salt).await?;
                    let key = derive_key(pin, &salt)?;
                    return Ok(Connection { stream, cipher: ChaCha20Poly1305::new(&key) });
                }
                Ok(Err(e)) => {
                    if retry < MAX_ATTEMPTS {
                        tracing::warn!("Connection attempt {:}/{} failed: {}", attempts, MAX_ATTEMPTS, e);
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                }
                Err(_) => {
                    if retry < MAX_ATTEMPTS {
                        tracing::warn!("Connection attempt {:}/{} timed out", attempts, MAX_ATTEMPTS);
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                }
            }
        }
    }
    
    Err(format!("Failed to connect to {}:{} after {} attempts", ip, port, attempts).into())
}

/// Runs the share-side event loop: forwards captured events to the receiver
/// and receives clipboard updates from the receiver.
pub async fn run_share_loop(conn: Connection, mut rx: mpsc::Receiver<NetworkEvent>) {
    let (mut read_half, mut write_half) = conn.stream.into_split();
    let cipher1 = conn.cipher.clone();
    let cipher2 = conn.cipher.clone();

    // Channel for graceful shutdown
    let (_shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

    // From network (receiver -> share): handle clipboard sync and keep-alive
    let net_read_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                result = recv_encrypted(&mut read_half, &cipher1) => {
                    match result {
                        Ok(event) => {
                            match event {
                                NetworkEvent::ClipboardText(txt) => {
                                    crate::clipboard::set_clipboard_text(txt);
                                }
                                NetworkEvent::KeepAlive => {
                                    // Peer is alive. No action needed.
                                }
                                _ => {} // Ignore other events from receiver
                            }
                        }
                        Err(_) => break,
                    }
                }
                _ = shutdown_rx.recv() => break,
            }
        }
    });

    // To network (share -> receiver): forward captured events with keep-alive
    let net_write_handle = tokio::spawn(async move {
        let mut keepalive_interval =
            tokio::time::interval(Duration::from_secs(KEEPALIVE_INTERVAL_SECS));
        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    if send_encrypted(&mut write_half, &cipher2, &event).await.is_err() {
                        break;
                    }
                }
                _ = keepalive_interval.tick() => {
                    if send_encrypted(&mut write_half, &cipher2, &NetworkEvent::KeepAlive).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Wait for either task to finish (connection lost)
    tokio::select! {
        _ = net_read_handle => {},
        _ = net_write_handle => {},
    }
}

/// Runs the receive-side event loop: receives input events from the share machine
/// and forwards them to local simulation, while sending clipboard updates back.
pub async fn run_receive_loop(conn: Connection, tx: mpsc::Sender<NetworkEvent>) {
    let (mut read_half, mut write_half) = conn.stream.into_split();
    let cipher1 = conn.cipher.clone();
    let cipher2 = conn.cipher.clone();

    // Clipboard monitor for sending local clipboard to share machine
    let (clip_tx, mut clip_rx) = mpsc::channel::<NetworkEvent>(100);
    crate::clipboard::start_clipboard_monitor(clip_tx);

    let (_shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

    // To network (receiver -> share): send clipboard updates + keep-alive
    let net_write_handle = tokio::spawn(async move {
        let mut keepalive_interval =
            tokio::time::interval(Duration::from_secs(KEEPALIVE_INTERVAL_SECS));
        loop {
            tokio::select! {
                Some(event) = clip_rx.recv() => {
                    if send_encrypted(&mut write_half, &cipher2, &event).await.is_err() {
                        break;
                    }
                }
                _ = keepalive_interval.tick() => {
                    if send_encrypted(&mut write_half, &cipher2, &NetworkEvent::KeepAlive).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // From network (share -> receiver): forward mouse/keyboard events to simulation
    let net_read_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                result = recv_encrypted(&mut read_half, &cipher1) => {
                    match result {
                        Ok(event) => {
                            match &event {
                                NetworkEvent::ClipboardText(txt) => {
                                    crate::clipboard::set_clipboard_text(txt.clone());
                                }
                                NetworkEvent::KeepAlive => {
                                    // No action needed.
                                }
                                _ => {
                                    // Forward mouse/keyboard events to simulation
                                    let _ = tx.send(event).await;
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
                _ = shutdown_rx.recv() => break,
            }
        }
    });

    tokio::select! {
        _ = net_read_handle => {},
        _ = net_write_handle => {},
    }
}

// ============================================================
// Service Discovery using UDP broadcast
// ============================================================

/// Port used for UDP discovery broadcast
const DISCOVERY_PORT: u16 = 4446;
/// Magic bytes to identify Freemouse discovery packets
const DISCOVERY_MAGIC: &[u8; 4] = b"FMDS";

/// A discovered server
#[derive(Debug, Clone)]
pub struct DiscoveredServer {
    pub ip: String,
    pub hostname: String,
    #[allow(dead_code)]
    pub port: u16,
}

/// Starts the server-side discovery broadcaster.
/// Periodically broadcasts this server's existence on the local network.
pub async fn start_discovery_broadcast(port: u16) {
    let local_ip = local_ip_address::local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "0.0.0.0".to_string());
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "Unknown".to_string());

    // Create a UDP socket for broadcasting
    let socket = match create_broadcast_socket().await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("Failed to create discovery broadcast socket: {}", e);
            return;
        }
    };

    let mut interval = tokio::time::interval(Duration::from_secs(2));
    loop {
        interval.tick().await;

        let packet = DiscoveryPacket {
            magic: *DISCOVERY_MAGIC,
            ip: local_ip.clone(),
            hostname: hostname.clone(),
            port,
        };

        if let Ok(data) = bincode::serialize(&packet) {
            // Broadcast to the local network
            let broadcast_addr = format!("255.255.255.255:{}", DISCOVERY_PORT);
            if let Ok(addr) = broadcast_addr.parse::<std::net::SocketAddr>() {
                let _ = socket.send_to(&data, addr).await;
            }
        }
    }
}

/// Starts the client-side discovery listener.
/// Returns a receiver that yields discovered servers as they appear.
pub fn start_discovery_listener() -> mpsc::Receiver<DiscoveredServer> {
    let (tx, rx) = mpsc::channel::<DiscoveredServer>(100);

    tokio::spawn(async move {
        let socket = match create_listen_socket().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Failed to create discovery listener socket: {}", e);
                return;
            }
        };

        let mut buf = vec![0u8; 1024];
        loop {
            match socket.recv_from(&mut buf).await {
                Ok((len, _addr)) => {
                    let data = &buf[..len];
                    if let Ok(packet) = bincode::deserialize::<DiscoveryPacket>(data) {
                        if packet.magic == *DISCOVERY_MAGIC {
                            let server = DiscoveredServer {
                                ip: packet.ip,
                                hostname: packet.hostname,
                                port: packet.port,
                            };
                            let _ = tx.send(server).await;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Discovery recv error: {}", e);
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
    });

    rx
}

#[derive(Serialize, Deserialize, Debug)]
struct DiscoveryPacket {
    magic: [u8; 4],
    ip: String,
    hostname: String,
    port: u16,
}

async fn create_broadcast_socket(
) -> Result<tokio::net::UdpSocket, Box<dyn std::error::Error + Send + Sync>> {
    let std_socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
    std_socket.set_broadcast(true)?;
    let socket = tokio::net::UdpSocket::from_std(std_socket)?;
    Ok(socket)
}

async fn create_listen_socket(
) -> Result<tokio::net::UdpSocket, Box<dyn std::error::Error + Send + Sync>> {
    let std_socket = std::net::UdpSocket::bind(format!("0.0.0.0:{}", DISCOVERY_PORT))?;
    std_socket.set_broadcast(true)?;
    let socket = tokio::net::UdpSocket::from_std(std_socket)?;
    Ok(socket)
}
