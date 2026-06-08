use crate::layout::{MachineLayout, WorkspaceLayout};
use crate::network::{
    self, is_input_event, DiscoveredServer, Edge, NetworkEvent, ScreenInfo, DEFAULT_PORT,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use uuid::Uuid;

pub struct PeerConnection {
    pub machine_id: Uuid,
    pub outbound: mpsc::Sender<NetworkEvent>,
    pub shutdown: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

pub struct ConnectionManager {
    pub local_machine_id: Uuid,
    pub pin: String,
    pub layout: WorkspaceLayout,
    peers: HashMap<Uuid, PeerConnection>,
    peer_outbounds: Arc<RwLock<HashMap<Uuid, mpsc::Sender<NetworkEvent>>>>,
    discovery_shutdown: Arc<AtomicBool>,
    listener_shutdown: Arc<AtomicBool>,
    listener_handle: Option<std::thread::JoinHandle<()>>,
    discovery_handle: Option<std::thread::JoinHandle<()>>,
    inbound_tx: mpsc::Sender<NetworkEvent>,
    inbound_rx: Option<mpsc::Receiver<NetworkEvent>>,
    ui_tx: mpsc::Sender<NetworkEvent>,
    pub ui_rx: mpsc::Receiver<NetworkEvent>,
    bridge_handle: Option<std::thread::JoinHandle<()>>,
}

impl ConnectionManager {
    pub fn new(local_machine_id: Uuid, pin: String, layout: WorkspaceLayout) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(256);
        let (ui_tx, ui_rx) = mpsc::channel(64);
        Self {
            local_machine_id,
            pin,
            layout,
            peers: HashMap::new(),
            peer_outbounds: Arc::new(RwLock::new(HashMap::new())),
            discovery_shutdown: Arc::new(AtomicBool::new(false)),
            listener_shutdown: Arc::new(AtomicBool::new(false)),
            listener_handle: None,
            discovery_handle: None,
            inbound_tx,
            inbound_rx: Some(inbound_rx),
            ui_tx,
            ui_rx,
            bridge_handle: None,
        }
    }

    /// Routes captured events to peers and inbound events to local simulation.
    pub fn start_event_bridge(
        &mut self,
        mut capture_rx: mpsc::Receiver<NetworkEvent>,
        simulation_tx: mpsc::Sender<NetworkEvent>,
        shutdown: Arc<AtomicBool>,
    ) {
        let Some(mut inbound_rx) = self.inbound_rx.take() else {
            tracing::warn!("Event bridge already started");
            return;
        };

        let peer_outbounds = self.peer_outbounds.clone();
        let ui_tx = self.ui_tx.clone();

        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            rt.block_on(async {
                while !shutdown.load(Ordering::SeqCst) {
                    tokio::select! {
                        Some(event) = capture_rx.recv() => {
                            match &event {
                                NetworkEvent::ClipboardText(_) => {
                                    let targets: Vec<_> = peer_outbounds
                                        .read()
                                        .unwrap()
                                        .values()
                                        .cloned()
                                        .collect();
                                    for tx in targets {
                                        let _ = tx.send(event.clone()).await;
                                    }
                                }
                                _ if crate::capture::os::is_remote() => {
                                    if let Some(peer_id) = crate::capture::get_active_peer() {
                                        let target = peer_outbounds
                                            .read()
                                            .unwrap()
                                            .get(&peer_id)
                                            .cloned();
                                        if let Some(tx) = target {
                                            let _ = tx.send(event).await;
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        Some(event) = inbound_rx.recv() => {
                            match &event {
                                NetworkEvent::Hello { .. }
                                | NetworkEvent::LayoutUpdate { .. } => {
                                    let _ = ui_tx.send(event).await;
                                }
                                NetworkEvent::ClipboardText(txt) => {
                                    crate::clipboard::set_clipboard_text(txt.clone());
                                }
                                NetworkEvent::FileOffer { .. }
                                | NetworkEvent::FileAccept { .. }
                                | NetworkEvent::FileChunk { .. }
                                | NetworkEvent::FileComplete { .. }
                                | NetworkEvent::FileReject { .. } => {
                                    crate::file_transfer::handle_file_event(event);
                                }
                                _ if is_input_event(&event) => {
                                    let _ = simulation_tx.send(event).await;
                                }
                                _ => {}
                            }
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
                    }
                }
            });
        });
        self.bridge_handle = Some(handle);
    }

    pub fn start_discovery(&mut self, screens: Vec<ScreenInfo>) {
        self.stop_discovery();
        let shutdown = self.discovery_shutdown.clone();
        shutdown.store(false, Ordering::SeqCst);
        let machine_id = self.local_machine_id;
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(network::start_discovery_broadcast(
                DEFAULT_PORT,
                machine_id,
                screens,
                shutdown,
            ));
        });
        self.discovery_handle = Some(handle);
    }

    pub fn stop_discovery(&mut self) {
        self.discovery_shutdown.store(true, Ordering::SeqCst);
        if let Some(h) = self.discovery_handle.take() {
            let _ = h.join();
        }
    }

    pub fn start_listener(&mut self) {
        self.stop_listener();
        let shutdown = self.listener_shutdown.clone();
        shutdown.store(false, Ordering::SeqCst);
        let pin = self.pin.clone();
        let inbound_tx = self.inbound_tx.clone();
        let local_id = self.local_machine_id;
        let peer_outbounds = self.peer_outbounds.clone();

        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                loop {
                    if shutdown.load(Ordering::SeqCst) {
                        break;
                    }
                    match network::start_server(DEFAULT_PORT, &pin).await {
                        Ok(conn) => {
                            let peer_shutdown = Arc::new(AtomicBool::new(false));
                            let (out_tx, out_rx) = mpsc::channel(100);
                            let in_tx = inbound_tx.clone();
                            let ps = peer_shutdown.clone();
                            let outs = peer_outbounds.clone();
                            tokio::spawn(async move {
                                network::run_peer_loop(conn, out_rx, in_tx, ps, false).await;
                            });
                            // Peer id unknown until Hello; store outbound by connection order later
                            let _ = out_tx
                                .send(NetworkEvent::Hello {
                                    machine_id: local_id,
                                    hostname: hostname::get()
                                        .map(|h| h.to_string_lossy().to_string())
                                        .unwrap_or_else(|_| "Unknown".into()),
                                    screens: crate::capture::get_screens(),
                                    protocol_version: crate::layout::PROTOCOL_VERSION,
                                })
                                .await;
                            let _ = outs; // out_tx registered when Hello received from peer
                        }
                        Err(e) => {
                            tracing::warn!("Listener error: {}", e);
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        }
                    }
                }
            });
        });
        self.listener_handle = Some(handle);
    }

    pub fn stop_listener(&mut self) {
        self.listener_shutdown.store(true, Ordering::SeqCst);
        if let Some(h) = self.listener_handle.take() {
            let _ = h.join();
        }
    }

    pub fn connect_to_peer(&mut self, server: &DiscoveredServer) -> Result<(), String> {
        if self.peers.contains_key(&server.machine_id) {
            return Ok(());
        }
        let pin = self.pin.clone();
        let ip = server.ip.clone();
        let port = server.port;
        let machine_id = server.machine_id;
        let local_id = self.local_machine_id;
        let inbound_tx = self.inbound_tx.clone();
        let peer_outbounds = self.peer_outbounds.clone();

        let (out_tx, out_rx) = mpsc::channel(100);
        peer_outbounds
            .write()
            .unwrap()
            .insert(machine_id, out_tx.clone());

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();

        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                match network::start_client(&ip, port, &pin).await {
                    Ok(conn) => {
                        let in_tx = inbound_tx;
                        network::run_peer_loop(conn, out_rx, in_tx, shutdown_clone, true).await;
                    }
                    Err(e) => tracing::warn!("Failed to connect to {}: {}", ip, e),
                }
            });
        });

        let _ = out_tx.try_send(NetworkEvent::Hello {
            machine_id: local_id,
            hostname: hostname::get()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "Unknown".into()),
            screens: crate::capture::get_screens(),
            protocol_version: crate::layout::PROTOCOL_VERSION,
        });

        self.peers.insert(
            machine_id,
            PeerConnection {
                machine_id,
                outbound: out_tx,
                shutdown,
                handle: Some(handle),
            },
        );
        Ok(())
    }

    pub fn connect_neighbors(&mut self, discovered: &[DiscoveredServer]) {
        for server in discovered {
            if server.machine_id == self.local_machine_id {
                continue;
            }
            if self.local_machine_id >= server.machine_id {
                continue;
            }
            let is_neighbor = [Edge::Right, Edge::Left, Edge::Top, Edge::Bottom]
                .iter()
                .any(|edge| {
                    self.layout
                        .neighbor_at_edge(self.local_machine_id, *edge)
                        .is_some_and(|n| n.machine_id == server.machine_id)
                });
            if is_neighbor {
                let _ = self.connect_to_peer(server);
            }
        }
    }

    pub fn send_to_peer(&self, machine_id: Uuid, event: NetworkEvent) {
        if let Some(peer) = self.peers.get(&machine_id) {
            let _ = peer.outbound.try_send(event);
        }
    }

    pub fn send_to_active(&self, active_peer: Option<Uuid>, event: NetworkEvent) {
        if let Some(id) = active_peer {
            self.send_to_peer(id, event);
        }
    }

    pub fn update_layout(&mut self, layout: WorkspaceLayout) {
        self.layout = layout;
        let update = NetworkEvent::LayoutUpdate {
            machines: self.layout.machines.clone(),
        };
        for peer in self.peers.values() {
            let _ = peer.outbound.try_send(update.clone());
        }
    }

    pub fn add_discovered_to_layout(&mut self, server: &DiscoveredServer) {
        if self
            .layout
            .machines
            .iter()
            .any(|m| m.machine_id == server.machine_id)
        {
            return;
        }
        if let Some(pos) = self.layout.next_free_grid_pos() {
            self.layout.add_or_update_machine(MachineLayout {
                machine_id: server.machine_id,
                hostname: server.hostname.clone(),
                ip: server.ip.clone(),
                grid_pos: pos,
                screens: server.screens.clone(),
            });
            let _ = self.layout.save();
        }
    }

    pub fn shutdown_all(&mut self) {
        self.stop_discovery();
        self.stop_listener();
        if let Some(h) = self.bridge_handle.take() {
            let _ = h.join();
        }
        for (_, mut peer) in self.peers.drain() {
            peer.shutdown.store(true, Ordering::SeqCst);
            if let Some(h) = peer.handle.take() {
                let _ = h.join();
            }
        }
        self.peer_outbounds.write().unwrap().clear();
        crate::capture::os::stop_capture();
    }

    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    pub fn is_connected(&self, machine_id: Uuid) -> bool {
        self.peers.contains_key(&machine_id)
    }

    pub fn peers_first(&self) -> Option<&PeerConnection> {
        self.peers.values().next()
    }
}
