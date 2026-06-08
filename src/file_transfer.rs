use crate::network::NetworkEvent;
use notify_rust::Notification;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use uuid::Uuid;

struct TransferState {
    #[allow(dead_code)]
    filename: String,
    #[allow(dead_code)]
    size: u64,
    file: Option<File>,
    path: PathBuf,
    received: u64,
}

lazy_static::lazy_static! {
    static ref TRANSFERS: Mutex<HashMap<Uuid, TransferState>> = Mutex::new(HashMap::new());
}

fn download_dir() -> PathBuf {
    directories::UserDirs::new()
        .map(|d| d.download_dir().map(|p| p.join("Freemouse")).unwrap_or_else(|| PathBuf::from(".")))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn handle_file_event(event: NetworkEvent) {
    match event {
        NetworkEvent::FileOffer {
            transfer_id,
            filename,
            size,
            ..
        } => {
            let dir = download_dir();
            let _ = fs::create_dir_all(&dir);
            let path = dir.join(&filename);
            if let Ok(file) = File::create(&path) {
                TRANSFERS.lock().unwrap().insert(
                    transfer_id,
                    TransferState {
                        filename: filename.clone(),
                        size,
                        file: Some(file),
                        path: path.clone(),
                        received: 0,
                    },
                );
                let _ = Notification::new()
                    .summary("Freemouse")
                    .body(&format!("Receiving file: {}", filename))
                    .show();
            }
        }
        NetworkEvent::FileChunk {
            transfer_id,
            offset,
            data,
        } => {
            let mut transfers = TRANSFERS.lock().unwrap();
            if let Some(state) = transfers.get_mut(&transfer_id) {
                if let Some(ref mut file) = state.file {
                    let _ = file.seek(SeekFrom::Start(offset));
                    if file.write_all(&data).is_ok() {
                        state.received = state.received.saturating_add(data.len() as u64);
                    }
                }
            }
        }
        NetworkEvent::FileComplete { transfer_id } => {
            let mut transfers = TRANSFERS.lock().unwrap();
            if let Some(state) = transfers.remove(&transfer_id) {
                let _ = Notification::new()
                    .summary("Freemouse")
                    .body(&format!("File saved: {}", state.path.display()))
                    .show();
            }
        }
        NetworkEvent::FileReject { transfer_id, reason } => {
            TRANSFERS.lock().unwrap().remove(&transfer_id);
            let _ = Notification::new()
                .summary("Freemouse")
                .body(&format!("File transfer rejected: {}", reason))
                .show();
        }
        _ => {}
    }
}

/// Send a file to a remote peer via the provided channel sender.
pub async fn send_file(
    tx: &tokio::sync::mpsc::Sender<NetworkEvent>,
    path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::io::Read;

    let transfer_id = Uuid::new_v4();
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    let metadata = fs::metadata(path)?;
    let size = metadata.len();

    tx.send(NetworkEvent::FileOffer {
        transfer_id,
        filename: filename.clone(),
        size,
        mime: "application/octet-stream".into(),
    })
    .await?;

    let mut file = File::open(path)?;
    let mut offset = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        tx.send(NetworkEvent::FileChunk {
            transfer_id,
            offset,
            data: buf[..n].to_vec(),
        })
        .await?;
        offset += n as u64;
    }
    tx.send(NetworkEvent::FileComplete { transfer_id }).await?;
    Ok(())
}

/// Platform file picker for Wayland fallback "send file" UI.
pub fn pick_file_to_send() -> Option<PathBuf> {
    rfd::FileDialog::new().pick_file()
}
