use crate::network::NetworkEvent;
use arboard::Clipboard;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Flag set when we programmatically set the clipboard ourselves.
/// The clipboard monitor checks this to avoid echo loops.
static CLIPBOARD_OWN_SET: AtomicBool = AtomicBool::new(false);

/// Start monitoring the local clipboard for changes.
/// Sends detected changes over the provided channel.
/// Uses a simple polling mechanism and respects the CLIPBOARD_OWN_SET flag
/// to avoid re-sending texts that were set by our own set_clipboard_text().
pub fn start_clipboard_monitor(tx: tokio::sync::mpsc::Sender<NetworkEvent>) {
    std::thread::spawn(move || {
        let mut clipboard = match Clipboard::new() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to init clipboard: {:?}", e);
                return;
            }
        };

        let mut last_text = clipboard.get_text().unwrap_or_default();

        loop {
            std::thread::sleep(Duration::from_millis(500));

            // If we just set the clipboard ourselves, skip this poll cycle
            // and clear the flag for next time
            if CLIPBOARD_OWN_SET.swap(false, Ordering::SeqCst) {
                // Update last_text to the current clipboard content so we
                // don't detect the change on the next poll
                last_text = clipboard.get_text().unwrap_or_default();
                continue;
            }

            if let Ok(current_text) = clipboard.get_text() {
                if current_text != last_text && !current_text.is_empty() {
                    last_text = current_text.clone();
                    let _ = tx.blocking_send(NetworkEvent::ClipboardText(current_text));
                }
            }
        }
    });
}

/// Set the local clipboard text.
/// Marks the change as our own so the clipboard monitor doesn't echo it back.
pub fn set_clipboard_text(text: String) {
    if let Ok(mut clipboard) = Clipboard::new() {
        if clipboard.set_text(text).is_ok() {
            CLIPBOARD_OWN_SET.store(true, Ordering::SeqCst);
        }
    }
}
