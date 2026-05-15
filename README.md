# <img src="FreeMouse.png" width="28"> Freemouse

[![Release](https://img.shields.io/github/v/release/arnavpadwal/freemouse?style=flat&color=blue)](https://github.com/arnavpadwal/freemouse/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
![Platform: Windows | macOS | Linux](https://img.shields.io/badge/platform-windows%20%7C%20macos%20%7C%20linux-lightgrey)

**Encrypted, self-hosted software KVM switch** — share a single mouse, keyboard, and clipboard across multiple computers over your local network. Move your cursor to the edge of the screen and it seamlessly jumps to the next machine. All traffic is end-to-end encrypted. No cloud, no accounts.

---

## 📦 Download

Grab the latest binary from the [releases page](https://github.com/arnavpadwal/freemouse/releases).

| Platform | File |
|----------|------|
| 🐧 Linux (x86-64) | `FreeMouse-v{VERSION}-x86_64-linux` |
| 🪟 Windows | *(coming soon)* |
| 🍎 macOS | *(coming soon)* |

Or build from source (3-5 minutes):

```bash
git clone https://github.com/arnavpadwal/freemouse
cd freemouse
cargo run --release
```

---

## ✨ Features

- **🔀 Seamless cursor switching** — Move your mouse off the right screen edge to take control of a remote machine; move left to return
- **⌨️ Full keyboard forwarding** — All keys, modifiers (Ctrl/Alt/Shift/Meta), function keys, numpad — mapped and forwarded
- **🖱️ Mouse support** — Left, right, middle clicks, movement, scrolling over the wire
- **📋 Clipboard sync** — Copy on one machine, paste on the other (echo-prevention built in)
- **🔒 End-to-end encrypted** — ChaCha20Poly1305 AEAD with Argon2id key derivation from a 6-digit PIN
- **🔍 Auto-discovery** — No IP wrangling; machines find each other via UDP broadcast
- **🌐 Cross-platform** — Windows, macOS, Linux
- **🎨 Dark-mode GUI** — Built with egui, minimal and responsive
- **💓 Keep-alive pings** — Automatic disconnect detection every 2 seconds
- **📦 No cloud, no accounts** — Pure peer-to-peer over your LAN

---

## 🚀 Quick Start

### Prerequisites

| Platform | Dependency |
|----------|-----------|
| All | [Rust 1.75+](https://rustup.rs/) |
| 🪟 Windows | Visual Studio Build Tools or MSVC |
| 🍎 macOS | `xcode-select --install` |
| 🐧 Linux | `build-essential` (Debian) / `base-devel` (Arch) |

### Build & Run

```bash
cargo run --release
```

First launch on macOS/Linux may prompt for accessibility permissions — grant them for input capture to work.

### Linux (Wayland) Setup

```bash
# Input capture (/dev/input/event*)
sudo usermod -a -G input $USER

# Input simulation (uinput device)
sudo usermod -a -G uinput $USER

# Log out & back in for group changes to take effect
```

> **Note:** No X11 libraries are needed — Freemouse uses `evdev` and `uinput` natively on Wayland.

---

## 🎮 Usage

### Share Mode (Server)

1. Click **📤 Share**
2. Note the displayed **IP** and **6-digit PIN**
3. Share these with the receiving machine
4. Move your cursor **off the right screen edge** to take control of the remote machine
5. Move **back to the left edge** to return control locally

### Receive Mode (Client)

1. Click **📥 Receive**
2. Select a machine from the auto-discovered list, **or** enter IP + PIN manually
3. Click **🔗 Connect**
4. The remote machine can now control your mouse, keyboard, and clipboard

---

## 🔒 Security

| Component | Algorithm |
|-----------|-----------|
| **Key derivation** | Argon2id (memory-hard, PIN → 256-bit key) |
| **Encryption** | ChaCha20Poly1305 (authenticated AEAD) |
| **Per-message nonce** | Random 96-bit nonce per frame |
| **PIN space** | 6 digits (~1M combinations) |

The salt is exchanged in plaintext during the handshake. All subsequent traffic is encrypted and authenticated.

---

## 🏗️ Architecture

```
┌─────────────────────┐     TCP/4444     ┌─────────────────────┐
│   SHARE (Server)    │◄────────────────►│  RECEIVE (Client)   │
│                     │  Encrypted tunnel│                     │
│  Input Capture ─────┤  Mouse, Keys,    ├─── Input Simulation │
│  Clipboard Monitor ─┤  Clipboard, Ping │─── Clipboard Monitor│
└─────────────────────┘                  └─────────────────────┘

        UDP/4446 — Discovery broadcasts (b"FMDS" + IP + hostname)
```

### Modules

| File | Role |
|------|------|
| `src/main.rs` | GUI, app state, mode management (egui/eframe) |
| `src/network.rs` | TCP server/client, encryption, protocol, discovery (tokio) |
| `src/capture.rs` | Input capture + simulation (evdev on Linux, rdev/enigo on Win/macOS) |
| `src/clipboard.rs` | Clipboard polling with echo prevention (arboard) |

### Wire Protocol

1. **Handshake** — Server sends 16-byte salt → Client derives key via Argon2id
2. **Framing** — `[4-byte length][12-byte nonce][encrypted bincode payload]`
3. **Events** — Typed enum: `MouseMoved`, `KeyDown`, `ClipboardText`, `KeepAlive`, etc.
4. **Keep-alive** — Both sides send every 2s; connection drops on timeout

---

## 🛠️ Development

```bash
cargo build              # Debug build
cargo build --release    # Optimized build
cargo clippy             # Lint
cargo fmt                # Format
RUST_LOG=debug cargo run # Debug logging
```

### Project Layout

```
freemouse/
├── src/
│   ├── main.rs
│   ├── network.rs
│   ├── capture.rs
│   └── clipboard.rs
├── build.rs              # Windows .ico embedding
├── FreeMouse.ico         # Windows icon
├── FreeMouse.png         # Project logo
├── Cargo.toml
├── Cargo.lock            # Pinned for reproducible builds
└── CHANGELOG.md
```

---

## 📄 License

MIT — see [LICENSE](LICENSE) for details.
