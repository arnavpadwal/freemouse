# Changelog

## v0.1.0 (2026-05-12)

Initial release of FreeMouse - a cross-platform, encrypted, self-hosted software KVM switch.

### Features
- Seamless cursor switching across machines (move mouse to screen edge)
- Full keyboard forwarding (keys, modifiers, function keys, numpad)
- Mouse support (left, right, middle click, movement, scrolling)
- Clipboard sync with echo-prevention
- ChaCha20Poly1305 AEAD encryption with Argon2 key derivation
- Auto-discovery via UDP broadcast (no IP configuration needed)
- Cross-platform: Windows, macOS, Linux (Wayland & X11)
- Clean dark-mode GUI built with egui
- 10-second connection timeout with clear error messages
- Keep-alive pings for automatic disconnect detection
- No cloud, no accounts - pure peer-to-peer

### Known Limitations
- Linux build only in this release (Windows/macOS builds coming soon)
- Requires Rust runtime dependencies on target machines
- UDP broadcast discovery limited to local subnet
