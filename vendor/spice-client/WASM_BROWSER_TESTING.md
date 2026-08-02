# WASM Browser Testing Guide

This guide explains how to test the SPICE client WebAssembly (WASM) build in a web browser with a real SPICE server.

## Quick Start

The fastest way to get started is to use the integrated test environment:

```bash
cd spice-client

# Start complete test environment (QEMU + WebSocket + Web Server)
just wasm-test

# Or use the script directly
./start-wasm-test.sh
```

This will:
1. Build the WASM package
2. Start a QEMU VM with SPICE server
3. Start a WebSocket proxy for WASM-to-SPICE communication
4. Start an HTTP server for the web interface
5. Open http://localhost:8000 in your browser

## Architecture

The WASM testing environment consists of several components:

```
┌─────────────────┐       ┌──────────────────┐       ┌─────────────────┐
│   Web Browser   │◄─────►│  HTTP Server     │       │  SPICE Server   │
│  (WASM Client)  │       │  (Python)        │       │  (QEMU/Docker)  │
└────────┬────────┘       └──────────────────┘       └────────┬────────┘
         │                                                      │
         │ WebSocket (8080)                                    │ TCP (5900)
         │                                                      │
         └──────────────────►┌──────────────────┐◄────────────┘
                             │ WebSocket Proxy  │
                             │  (Python)        │
                             └──────────────────┘
```

### Components

1. **Web Browser**: Runs the WASM SPICE client
2. **HTTP Server**: Serves the HTML page and WASM files (Python-based)
3. **WebSocket Proxy**: Bridges WebSocket (browser) to TCP (SPICE protocol)
4. **SPICE Server**: QEMU VM or Docker container providing SPICE protocol

## Building WASM Package

### Build Modes

The `build-wasm.sh` script supports three build modes:

```bash
# Release build (optimized, smaller)
./build-wasm.sh release

# Development build (with debug symbols)
./build-wasm.sh dev

# Test build (with extra features)
./build-wasm.sh test
```

### Output Location

By default, builds output to the `pkg/` directory. You can customize:

```bash
# Build to custom directory
./build-wasm.sh release web-test
```

### Using Justfile

```bash
# Build commands
just wasm-build          # Release build
just wasm-build-dev      # Dev build
just wasm-build-test     # Test build
```

## Starting Test Environment

### Option 1: Complete Environment (Recommended)

Start everything with one command:

```bash
just wasm-test
```

This uses Docker to run a lightweight SPICE test server and automatically configures all ports.

### Option 2: Custom Configuration

Start with custom ports:

```bash
# Custom web server port
just wasm-test-port 3000

# Use different SPICE port
./start-wasm-test.sh --spice-port 5901 --web-port 8080
```

### Option 3: Existing SPICE Server

If you already have a SPICE server running:

```bash
# Connect to existing SPICE server
just wasm-test-existing 5900

# Or with script
./start-wasm-test.sh --qemu none --spice-port 5900
```

### Option 4: Manual Setup

For fine-grained control, start components individually:

```bash
# 1. Build WASM
just wasm-build-dev

# 2. Start SPICE server (Docker)
cd docker
docker compose --profile server-debug up -d
cd ..

# 3. Start WebSocket proxy
python3 docker/websocket-proxy.py --port 8080 --spice-host localhost --spice-port 5900 &

# 4. Start web server
just wasm-serve 8000
```

## Testing in Browser

### 1. Open Web Interface

Navigate to: http://localhost:8000

### 2. Configure Connection

In the web interface:
- **Host**: `localhost` (for local testing)
- **Port**: `8080` (WebSocket proxy port, not SPICE port!)

### 3. Connect

Click "Connect" button. The status should change to "Connected" if successful.

### 4. Test Features

- **Display**: You should see the VM screen rendered on the canvas
- **Mouse**: Click and move mouse on the canvas to control VM pointer
- **Keyboard**: Focus on the page and type to send keystrokes

### 5. Monitor Logs

The web interface includes a connection log at the bottom showing:
- Connection events
- Display updates
- Errors and warnings

## Troubleshooting

### Connection Failed

**Problem**: "Connection failed: WebSocket connection error"

**Solutions**:
1. Verify WebSocket proxy is running:
   ```bash
   curl http://localhost:8080
   ```

2. Check SPICE server is accessible:
   ```bash
   nc -zv localhost 5900
   ```

3. Review WebSocket proxy logs for errors

### WASM Module Not Loading

**Problem**: "Failed to load WASM module"

**Solutions**:
1. Rebuild WASM package:
   ```bash
   just wasm-build-dev
   ```

2. Check HTTP server is serving from correct directory (should include `pkg/` subdirectory)

3. Verify MIME types in browser Network tab (`.wasm` should be `application/wasm`)

### Display Not Updating

**Problem**: Connected but no display visible

**Solutions**:
1. Check browser console for JavaScript errors
2. Verify SPICE server is sending display updates:
   ```bash
   # Test with native client
   cargo run --bin spice-e2e-test -- --host localhost --port 5900 --duration 10 -vv
   ```

3. Ensure display channel is properly initialized in WASM client

### CORS Errors

**Problem**: "Cross-Origin Request Blocked"

**Solutions**:
1. The Python HTTP server includes CORS headers by default
2. For custom servers, ensure these headers:
   ```
   Access-Control-Allow-Origin: *
   Cross-Origin-Opener-Policy: same-origin
   Cross-Origin-Embedder-Policy: require-corp
   ```

## Port Configuration

### Default Ports

| Service          | Port | Purpose                     |
|------------------|------|-----------------------------|
| HTTP Server      | 8000 | Serves web interface        |
| WebSocket Proxy  | 8080 | WASM-to-SPICE bridge       |
| SPICE Server     | 5900 | SPICE protocol endpoint    |

### Changing Ports

All ports can be customized:

```bash
./start-wasm-test.sh \
  --web-port 3000 \
  --ws-port 9000 \
  --spice-port 5901
```

Or via environment variables:

```bash
export WEB_PORT=3000
export WS_PROXY_PORT=9000
export SPICE_PORT=5901
./start-wasm-test.sh
```

## Development Workflow

### Iterative Testing

For rapid iteration during development:

```bash
# Terminal 1: Keep SPICE server running
cd docker
docker compose --profile server-debug up

# Terminal 2: Rebuild and test
just wasm-build-dev
./start-wasm-test.sh --qemu none --skip-build
```

### Hot Reload

For automatic rebuilds on code changes, consider using:

```bash
# Watch for changes and rebuild
cargo watch -x 'build --target wasm32-unknown-unknown'

# Or use wasm-pack watch
wasm-pack build --target web --dev --out-dir pkg
```

## Testing Checklist

Use this checklist to verify WASM implementation:

- [ ] WASM module loads without errors
- [ ] WebSocket connection establishes to proxy
- [ ] SPICE handshake completes successfully
- [ ] Display channel initializes
- [ ] Screen renders on canvas
- [ ] Mouse events send to server
- [ ] Keyboard events send to server
- [ ] Display updates reflect in real-time
- [ ] Reconnection works after disconnect
- [ ] No memory leaks during extended use

## Performance Testing

### Browser DevTools

Use browser developer tools to monitor:

1. **Network Tab**: WebSocket frame rates and sizes
2. **Performance Tab**: Frame rendering performance
3. **Memory Tab**: Heap usage and leaks

### Metrics to Track

- WebSocket message frequency (should match display update rate)
- Canvas rendering FPS (target: 30-60 fps)
- Memory usage (should stabilize after initial connection)
- CPU usage (should be reasonable for canvas rendering)

## Advanced Configuration

### Custom WASM Features

Build with specific features:

```bash
wasm-pack build --target web --dev -- --features "wasm,debug-protocol"
```

### Debug Logging

Enable verbose logging in the WASM client by opening browser console and setting:

```javascript
// Set log level
localStorage.setItem('spice_log_level', 'debug');
```

### Protocol Tracing

To capture SPICE protocol messages:

1. Enable tracing in WebSocket proxy:
   ```bash
   python3 docker/websocket-proxy.py --debug --trace-file /tmp/spice-trace.bin
   ```

2. Analyze with Wireshark or spice-protocol tools

## Cleanup

Stop all services:

```bash
# If started with start-wasm-test.sh, just press Ctrl+C

# Manual cleanup
pkill -f "python3.*serve.py"
pkill -f "python3.*websocket-proxy.py"
docker compose -f docker/docker-compose.yml down
```

## References

- [WASM Architecture](WASM_ARCHITECTURE.md) - Overall WASM implementation design
- [WASM Quick Start](WASM_QUICKSTART.md) - Quick reference for WASM features
- [Testing Guide](TESTING.md) - General testing documentation
- [Docker Setup](docker/README.md) - Docker-based test infrastructure

## Common Use Cases

### Testing Against Production VM

```bash
# Start only web components, connect to remote SPICE
./start-wasm-test.sh --qemu none --spice-port 5900

# In browser, set:
# Host: your-vm-host.example.com
# Port: 8080 (local WebSocket proxy)
```

### CI/CD Integration

```bash
# Non-interactive test run
./start-wasm-test.sh --no-cleanup &
TEST_PID=$!

# Run browser automation tests
npm run test:e2e

# Cleanup
kill $TEST_PID
```

### Multi-VM Testing

Start multiple SPICE servers on different ports and test concurrently:

```bash
# Terminal 1: VM 1
./start-wasm-test.sh --spice-port 5900 --ws-port 8080 --web-port 8000

# Terminal 2: VM 2
./start-wasm-test.sh --spice-port 5901 --ws-port 8081 --web-port 8001
```