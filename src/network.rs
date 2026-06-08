use crate::layout::{MachineLayout, PROTOCOL_VERSION};
use argon2::Argon2;
use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    ChaCha20Poly1305, Key, Nonce,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration, Instant};
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

pub const DEFAULT_PORT: u16 = 4444;
pub const FILE_TRANSFER_PORT: u16 = 4445;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Copy)]
pub enum Edge {
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ScreenInfo {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub primary: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum KeyCode {
    A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9,
    Alt, AltGr, ControlLeft, ControlRight, ShiftLeft, ShiftRight, MetaLeft, MetaRight,
    Super, Option,
    UpArrow, DownArrow, LeftArrow, RightArrow, PageUp, PageDown, Home, End,
    Insert, Delete, Backspace, Space, Tab, Return, Escape, CapsLock,
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    F13, F14, F15, F16, F17, F18, F19, F20, F21, F22, F23, F24,
    Minus, Equals, LeftBracket, RightBracket, Backslash, Semicolon, Quote,
    Comma, Period, Slash, Backtick,
    Numpad0, Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7, Numpad8, Numpad9,
    NumpadAdd, NumpadSubtract, NumpadMultiply, NumpadDivide, NumpadEnter, NumpadDecimal,
    PrintScreen, ScrollLock, Pause, Menu,
    MediaNext, MediaPrev, MediaPlayPause, MediaStop, VolumeUp, VolumeDown, VolumeMute,
    Unicode(char),
    Other(u32),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum MouseButton {
    Left, Right, Middle, X1, X2,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum NetworkEvent {
    MouseMoved(f64, f64),
    MouseMoveRelative(f64, f64),
    MouseButtonDown(MouseButton),
    MouseButtonUp(MouseButton),
    MouseScroll(i32, i32),
    KeyDown(KeyCode),
    KeyUp(KeyCode),
    ClipboardText(String),
    KeepAlive,
    AuthChallenge([u8; 32]),
    AuthOk,
    AuthFail(String),
    Hello {
        machine_id: Uuid,
        hostname: String,
        screens: Vec<ScreenInfo>,
        protocol_version: u32,
    },
    RemoteEnter {
        from_edge: Edge,
        y_ratio: f64,
    },
    RemoteLeave {
        to_edge: Edge,
    },
    CursorWarp(f64, f64),
    LayoutUpdate {
        machines: Vec<MachineLayout>,
    },
    KeyStateSnapshot(Vec<KeyCode>),
    FileOffer {
        transfer_id: Uuid,
        filename: String,
        size: u64,
        mime: String,
    },
    FileAccept {
        transfer_id: Uuid,
    },
    FileChunk {
        transfer_id: Uuid,
        offset: u64,
        data: Vec<u8>,
    },
    FileComplete {
        transfer_id: Uuid,
    },
    FileReject {
        transfer_id: Uuid,
        reason: String,
    },
}

const CONNECTION_TIMEOUT_SECS: u64 = 10;
pub const KEEPALIVE_INTERVAL_SECS: u64 = 2;
const KEEPALIVE_TIMEOUT_SECS: u64 = KEEPALIVE_INTERVAL_SECS * 3;
const MAX_FRAME_SIZE: u32 = 10 * 1024 * 1024;
const AUTH_MESSAGE: &[u8] = b"freemouse-auth-v1";

pub fn compute_auth_token(key: &Key) -> [u8; 32] {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key.as_slice())
        .expect("HMAC accepts any key length");
    mac.update(AUTH_MESSAGE);
    mac.finalize().into_bytes().into()
}

fn derive_key(pin: &str, salt: &[u8; 16]) -> Result<Key, Box<dyn std::error::Error + Send + Sync>> {
    let argon2 = Argon2::default();
    let mut key_bytes = [0u8; 32];
    argon2
        .hash_password_into(pin.as_bytes(), salt, &mut key_bytes)
        .map_err(|e| format!("Argon2 error: {}", e))?;
    Ok(*Key::from_slice(&key_bytes))
}

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
    stream.flush().await?;
    Ok(())
}

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

pub struct Connection {
    pub stream: TcpStream,
    pub cipher: ChaCha20Poly1305,
}

async fn server_handshake(
    mut stream: TcpStream,
    pin: &str,
) -> Result<Connection, Box<dyn std::error::Error + Send + Sync>> {
    let mut salt = [0u8; 16];
    rand::Rng::fill(&mut rand::thread_rng(), &mut salt);
    stream.write_all(&salt).await?;
    let key = derive_key(pin, &salt)?;
    let cipher = ChaCha20Poly1305::new(&key);
    let expected = compute_auth_token(&key);

    let challenge = match timeout(
        Duration::from_secs(CONNECTION_TIMEOUT_SECS),
        recv_encrypted(&mut stream, &cipher),
    )
    .await
    {
        Ok(Ok(NetworkEvent::AuthChallenge(token))) => token,
        Ok(Ok(_)) => return Err("Expected AuthChallenge".into()),
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err("Auth handshake timed out".into()),
    };

    if challenge != expected {
        let _ = send_encrypted(
            &mut stream,
            &cipher,
            &NetworkEvent::AuthFail("Invalid PIN".into()),
        )
        .await;
        return Err("Invalid PIN".into());
    }

    send_encrypted(&mut stream, &cipher, &NetworkEvent::AuthOk).await?;
    Ok(Connection { stream, cipher })
}

async fn client_handshake(
    mut stream: TcpStream,
    pin: &str,
) -> Result<Connection, Box<dyn std::error::Error + Send + Sync>> {
    let mut salt = [0u8; 16];
    stream.read_exact(&mut salt).await?;
    let key = derive_key(pin, &salt)?;
    let cipher = ChaCha20Poly1305::new(&key);
    let token = compute_auth_token(&key);
    send_encrypted(&mut stream, &cipher, &NetworkEvent::AuthChallenge(token)).await?;

    match timeout(
        Duration::from_secs(CONNECTION_TIMEOUT_SECS),
        recv_encrypted(&mut stream, &cipher),
    )
    .await
    {
        Ok(Ok(NetworkEvent::AuthOk)) => Ok(Connection { stream, cipher }),
        Ok(Ok(NetworkEvent::AuthFail(msg))) => Err(format!("Authentication failed: {}", msg).into()),
        Ok(Ok(_)) => Err("Unexpected auth response".into()),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("Auth response timed out".into()),
    }
}

pub async fn start_server(
    port: u16,
    pin: &str,
) -> Result<Connection, Box<dyn std::error::Error + Send + Sync>> {
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    tracing::info!("Server listening on port {}", port);
    loop {
        let (stream, addr) = listener.accept().await?;
        tracing::info!("Connection from {}", addr);
        match server_handshake(stream, pin).await {
            Ok(conn) => return Ok(conn),
            Err(e) => tracing::warn!("Handshake failed from {}: {}", addr, e),
        }
    }
}

pub async fn start_client(
    ip: &str,
    port: u16,
    pin: &str,
) -> Result<Connection, Box<dyn std::error::Error + Send + Sync>> {
    const MAX_ATTEMPTS: usize = 3;
    let addresses = [format!("{}:{}", ip, port), format!("[{}]:{}", ip, port)];

    for address in addresses {
        for retry in 1..=MAX_ATTEMPTS {
            match timeout(
                Duration::from_secs(CONNECTION_TIMEOUT_SECS),
                TcpStream::connect(address.clone()),
            )
            .await
            {
                Ok(Ok(stream)) => match client_handshake(stream, pin).await {
                    Ok(conn) => return Ok(conn),
                    Err(e) => {
                        if e.to_string().contains("Invalid PIN") || e.to_string().contains("Authentication failed") {
                            return Err(e);
                        }
                        tracing::warn!("Handshake failed: {}", e);
                    }
                },
                Ok(Err(e)) => {
                    tracing::warn!("Connect attempt {}/{} failed: {}", retry, MAX_ATTEMPTS, e);
                }
                Err(_) => {
                    tracing::warn!("Connect attempt {}/{} timed out", retry, MAX_ATTEMPTS);
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    Err(format!("Failed to connect to {}:{}", ip, port).into())
}

pub fn is_input_event(event: &NetworkEvent) -> bool {
    matches!(
        event,
        NetworkEvent::MouseMoved(_, _)
            | NetworkEvent::MouseMoveRelative(_, _)
            | NetworkEvent::MouseButtonDown(_)
            | NetworkEvent::MouseButtonUp(_)
            | NetworkEvent::MouseScroll(_, _)
            | NetworkEvent::KeyDown(_)
            | NetworkEvent::KeyUp(_)
            | NetworkEvent::CursorWarp(_, _)
            | NetworkEvent::RemoteEnter { .. }
            | NetworkEvent::RemoteLeave { .. }
            | NetworkEvent::KeyStateSnapshot(_)
    )
}

pub async fn run_share_loop(
    conn: Connection,
    mut rx: mpsc::Receiver<NetworkEvent>,
    shutdown: Arc<AtomicBool>,
) {
    let (mut read_half, mut write_half) = conn.stream.into_split();
    let cipher1 = conn.cipher.clone();
    let cipher2 = conn.cipher.clone();
    let mut last_peer_activity = Instant::now();

    let shutdown_r = shutdown.clone();
    let net_read_handle = tokio::spawn(async move {
        loop {
            if shutdown_r.load(Ordering::SeqCst) {
                break;
            }
            if last_peer_activity.elapsed() > Duration::from_secs(KEEPALIVE_TIMEOUT_SECS) {
                tracing::warn!("Keep-alive timeout");
                break;
            }
            match timeout(
                Duration::from_secs(KEEPALIVE_INTERVAL_SECS),
                recv_encrypted(&mut read_half, &cipher1),
            )
            .await
            {
                Ok(Ok(event)) => {
                    last_peer_activity = Instant::now();
                    match event {
                        NetworkEvent::ClipboardText(txt) => {
                            crate::clipboard::set_clipboard_text(txt);
                        }
                        NetworkEvent::KeepAlive => {}
                        _ => {}
                    }
                }
                Ok(Err(e)) => {
                    tracing::debug!("Share read error: {:?}", e);
                    break;
                }
                Err(_) => continue,
            }
        }
    });

    let shutdown_w = shutdown.clone();
    let net_write_handle = tokio::spawn(async move {
        let mut keepalive_interval =
            tokio::time::interval(Duration::from_secs(KEEPALIVE_INTERVAL_SECS));
        loop {
            if shutdown_w.load(Ordering::SeqCst) {
                break;
            }
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

    tokio::select! {
        _ = net_read_handle => {},
        _ = net_write_handle => {},
    }
    shutdown.store(true, Ordering::SeqCst);
    crate::capture::os::stop_capture();
}

pub async fn run_receive_loop(
    conn: Connection,
    tx: mpsc::Sender<NetworkEvent>,
    shutdown: Arc<AtomicBool>,
) {
    let (mut read_half, mut write_half) = conn.stream.into_split();
    let cipher1 = conn.cipher.clone();
    let cipher2 = conn.cipher.clone();

    let (clip_tx, mut clip_rx) = mpsc::channel::<NetworkEvent>(100);
    crate::clipboard::start_clipboard_monitor(clip_tx);

    let mut last_peer_activity = Instant::now();

    let shutdown_w = shutdown.clone();
    let net_write_handle = tokio::spawn(async move {
        let mut keepalive_interval =
            tokio::time::interval(Duration::from_secs(KEEPALIVE_INTERVAL_SECS));
        loop {
            if shutdown_w.load(Ordering::SeqCst) {
                break;
            }
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

    let shutdown_r = shutdown.clone();
    let net_read_handle = tokio::spawn(async move {
        loop {
            if shutdown_r.load(Ordering::SeqCst) {
                break;
            }
            if last_peer_activity.elapsed() > Duration::from_secs(KEEPALIVE_TIMEOUT_SECS) {
                tracing::warn!("Keep-alive timeout");
                break;
            }
            match timeout(
                Duration::from_secs(KEEPALIVE_INTERVAL_SECS),
                recv_encrypted(&mut read_half, &cipher1),
            )
            .await
            {
                Ok(Ok(event)) => {
                    last_peer_activity = Instant::now();
                    match &event {
                        NetworkEvent::ClipboardText(txt) => {
                            crate::clipboard::set_clipboard_text(txt.clone());
                        }
                        NetworkEvent::KeepAlive => {}
                        NetworkEvent::FileOffer { .. }
                        | NetworkEvent::FileAccept { .. }
                        | NetworkEvent::FileChunk { .. }
                        | NetworkEvent::FileComplete { .. }
                        | NetworkEvent::FileReject { .. } => {
                            crate::file_transfer::handle_file_event(event);
                        }
                        _ if is_input_event(&event) => {
                            let _ = tx.send(event).await;
                        }
                        _ => {}
                    }
                }
                Ok(Err(e)) => {
                    tracing::debug!("Receive read error: {:?}", e);
                    break;
                }
                Err(_) => continue,
            }
        }
    });

    tokio::select! {
        _ = net_read_handle => {},
        _ = net_write_handle => {},
    }
    shutdown.store(true, Ordering::SeqCst);
}

pub async fn run_peer_loop(
    conn: Connection,
    mut outbound: mpsc::Receiver<NetworkEvent>,
    inbound: mpsc::Sender<NetworkEvent>,
    shutdown: Arc<AtomicBool>,
    is_controller: bool,
) {
    let (mut read_half, mut write_half) = conn.stream.into_split();
    let cipher1 = conn.cipher.clone();
    let cipher2 = conn.cipher.clone();
    let mut last_peer_activity = Instant::now();

    let shutdown_r = shutdown.clone();
    let net_read = tokio::spawn(async move {
        loop {
            if shutdown_r.load(Ordering::SeqCst) {
                break;
            }
            if last_peer_activity.elapsed() > Duration::from_secs(KEEPALIVE_TIMEOUT_SECS) {
                break;
            }
            match timeout(
                Duration::from_secs(KEEPALIVE_INTERVAL_SECS),
                recv_encrypted(&mut read_half, &cipher1),
            )
            .await
            {
                Ok(Ok(event)) => {
                    last_peer_activity = Instant::now();
                    match &event {
                        NetworkEvent::ClipboardText(txt) => {
                            crate::clipboard::set_clipboard_text(txt.clone());
                        }
                        NetworkEvent::KeepAlive => {}
                        NetworkEvent::FileOffer { .. }
                        | NetworkEvent::FileAccept { .. }
                        | NetworkEvent::FileChunk { .. }
                        | NetworkEvent::FileComplete { .. }
                        | NetworkEvent::FileReject { .. } => {
                            crate::file_transfer::handle_file_event(event);
                        }
                        _ => {
                            let _ = inbound.send(event).await;
                        }
                    }
                }
                Ok(Err(_)) => break,
                Err(_) => continue,
            }
        }
    });

    let shutdown_w = shutdown.clone();
    let net_write = tokio::spawn(async move {
        let mut keepalive_interval =
            tokio::time::interval(Duration::from_secs(KEEPALIVE_INTERVAL_SECS));
        let (clip_tx, mut clip_rx) = mpsc::channel::<NetworkEvent>(100);
        if !is_controller {
            crate::clipboard::start_clipboard_monitor(clip_tx);
        }
        loop {
            if shutdown_w.load(Ordering::SeqCst) {
                break;
            }
            tokio::select! {
                Some(event) = outbound.recv() => {
                    if send_encrypted(&mut write_half, &cipher2, &event).await.is_err() {
                        break;
                    }
                }
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

    tokio::select! {
        _ = net_read => {},
        _ = net_write => {},
    }
    shutdown.store(true, Ordering::SeqCst);
}

// Discovery
const DISCOVERY_PORT: u16 = 4446;
const DISCOVERY_MAGIC: &[u8; 4] = b"FMDS";

#[derive(Debug, Clone)]
pub struct DiscoveredServer {
    pub ip: String,
    pub hostname: String,
    pub port: u16,
    pub machine_id: Uuid,
    pub screens: Vec<ScreenInfo>,
    pub protocol_version: u32,
}

pub struct DiscoveryHandle {
    pub rx: mpsc::Receiver<DiscoveredServer>,
    shutdown: Arc<AtomicBool>,
}

impl DiscoveryHandle {
    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

pub async fn start_discovery_broadcast(
    port: u16,
    machine_id: Uuid,
    screens: Vec<ScreenInfo>,
    shutdown: Arc<AtomicBool>,
) {
    let local_ip = local_ip_address::local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "0.0.0.0".to_string());
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "Unknown".to_string());

    let socket = match create_broadcast_socket().await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("Failed to create discovery broadcast socket: {}", e);
            return;
        }
    };

    let mut interval = tokio::time::interval(Duration::from_secs(2));
    while !shutdown.load(Ordering::SeqCst) {
        interval.tick().await;
        let packet = DiscoveryPacket {
            magic: *DISCOVERY_MAGIC,
            ip: local_ip.clone(),
            hostname: hostname.clone(),
            port,
            machine_id,
            screens: screens.clone(),
            protocol_version: PROTOCOL_VERSION,
        };
        if let Ok(data) = bincode::serialize(&packet) {
            let broadcast_addr = format!("255.255.255.255:{}", DISCOVERY_PORT);
            if let Ok(addr) = broadcast_addr.parse::<std::net::SocketAddr>() {
                let _ = socket.send_to(&data, addr).await;
            }
            if let Ok(std::net::IpAddr::V4(v4)) = local_ip.parse::<std::net::IpAddr>() {
                let octets = v4.octets();
                let subnet =
                    format!("{}.{}.{}.255:{}", octets[0], octets[1], octets[2], DISCOVERY_PORT);
                if let Ok(addr) = subnet.parse::<std::net::SocketAddr>() {
                    let _ = socket.send_to(&data, addr).await;
                }
            }
        }
    }
}

pub fn start_discovery_listener() -> DiscoveryHandle {
    let (tx, rx) = mpsc::channel::<DiscoveredServer>(100);
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let socket = match create_listen_socket().await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("Failed to create discovery listener socket: {}", e);
                    return;
                }
            };
            let mut buf = vec![0u8; 4096];
            while !shutdown_clone.load(Ordering::SeqCst) {
                match timeout(Duration::from_millis(500), socket.recv_from(&mut buf)).await {
                    Ok(Ok((len, _))) => {
                        let data = &buf[..len];
                        if let Ok(packet) = bincode::deserialize::<DiscoveryPacket>(data) {
                            if packet.magic == *DISCOVERY_MAGIC {
                                let _ = tx
                                    .send(DiscoveredServer {
                                        ip: packet.ip,
                                        hostname: packet.hostname,
                                        port: packet.port,
                                        machine_id: packet.machine_id,
                                        screens: packet.screens,
                                        protocol_version: packet.protocol_version,
                                    })
                                    .await;
                            }
                        }
                    }
                    Ok(Err(e)) => tracing::warn!("Discovery recv error: {}", e),
                    Err(_) => {}
                }
            }
        });
    });

    DiscoveryHandle { rx, shutdown }
}

#[derive(Serialize, Deserialize, Debug)]
struct DiscoveryPacket {
    magic: [u8; 4],
    ip: String,
    hostname: String,
    port: u16,
    machine_id: Uuid,
    screens: Vec<ScreenInfo>,
    protocol_version: u32,
}

async fn create_broadcast_socket(
) -> Result<tokio::net::UdpSocket, Box<dyn std::error::Error + Send + Sync>> {
    let std_socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
    std_socket.set_broadcast(true)?;
    Ok(tokio::net::UdpSocket::from_std(std_socket)?)
}

async fn create_listen_socket(
) -> Result<tokio::net::UdpSocket, Box<dyn std::error::Error + Send + Sync>> {
    let std_socket = std::net::UdpSocket::bind(format!("0.0.0.0:{}", DISCOVERY_PORT))?;
    std_socket.set_broadcast(true)?;
    Ok(tokio::net::UdpSocket::from_std(std_socket)?)
}

