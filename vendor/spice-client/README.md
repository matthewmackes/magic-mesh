# 🚀 spice-client

[![Crates.io](https://img.shields.io/crates/v/spice-client.svg)](https://crates.io/crates/spice-client)
[![Documentation](https://docs.rs/spice-client/badge.svg)](https://docs.rs/spice-client)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)

A modern SPICE (Simple Protocol for Independent Computing Environments) client implementation in pure Rust with WebAssembly support.

> ⚠️ **Experimental**: This library is under active development and APIs are subject to change. We're working on stabilizing the interface and will have a detailed roadmap available soon.

## ✨ Features

- **Pure Rust** - No C dependencies, memory safe implementation
- **Cross-platform** - Native support for Linux, macOS, Windows*, and WebAssembly
- **Async/Await** - Modern async API using Tokio
- **WebAssembly Ready** - Run SPICE clients directly in web browsers
- **Multiple Channels** - Display, input, cursor, and main channel support

*\* Windows support is included but currently untested. Contributions welcome!*

## 🏗️ Architecture

**Modern Design with Platform Flexibility**

```
🖥️  Native App          🌐  Web Browser
       │                        │
  ┌────▼────┐             ┌────▼────┐
  │  Tokio  │             │  WASM   │
  │   TCP   │             │WebSocket│
  └────┬────┘             └────┬────┘
       │                        │
       └────────┬───────────────┘
                │
          ┌─────▼─────┐
          │   SPICE   │
          │  Server   │
          └───────────┘
```

## 🚀 Getting Started

### Prerequisites
- 🦀 Rust 1.75+
- 🖥️ SPICE-enabled VM (QEMU/libvirt)

### Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
spice-client = "0.1.0"
```

### Quick Example

```rust
use spice_client::{SpiceClient, SpiceError};

#[tokio::main]
async fn main() -> Result<(), SpiceError> {
    // Connect to SPICE server
    let mut client = SpiceClient::new("localhost".to_string(), 5900);
    client.connect().await?;

    // Start event processing
    client.start_event_loop().await?;

    Ok(())
}
```

## 📦 Supported Channels

| Channel | Status | Description |
|---------|--------|-------------|
| Main | ✅ | Connection setup and control |
| Display | ✅ | Screen rendering and updates |
| Inputs | ✅ | Keyboard and mouse input |
| Cursor | ✅ | Hardware cursor support |
| Audio | 🚧 | Coming soon |
| USB | 🚧 | Planned |

## 🛠️ Building

### Native Build

```bash
# Standard build
cargo build --release

# Run tests
cargo test
```

### WebAssembly Build

```bash
# Install wasm-pack
curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh

# Build for web
wasm-pack build --target web
```

## 🌐 WebSocket Proxy

For browser deployments, a WebSocket-to-TCP proxy is required:

```bash
# Example proxy setup
python examples/websocket-proxy.py --spice-host localhost --spice-port 5900
```

## 📋 Current Status

This is an experimental implementation focusing on core functionality:

**Working**:
- Basic SPICE protocol handshake
- Display channel with drawing operations
- Keyboard and mouse input
- Cursor updates
- WebAssembly compilation

**In Progress**:
- Audio channels
- Clipboard integration
- Performance optimizations
- Comprehensive testing

**Planned**:
- USB redirection
- File transfer
- Enhanced compression (LZ4)
- TLS encryption

## 🤝 Contributing

We welcome contributions! Please:
- 🐛 Report bugs via GitHub issues
- 💡 Discuss major changes before implementing
- 🧪 Add tests for new functionality
- 📚 Update documentation as needed

## 📜 License

GPL v3 License 🎉

---

**Part of the ⚡ Quickemu Manager project**