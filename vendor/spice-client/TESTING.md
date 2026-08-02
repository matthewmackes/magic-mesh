# SPICE Client Testing Guide

This document describes how to run tests for the SPICE client, with a focus on native (non-Docker) testing for faster development iteration.

## Quick Start

### For Development (Recommended)

Fast E2E testing with native client and Docker server:

```bash
# Run all tests (unit + integration + e2e)
just test-all

# Or run individually:
just test-unit              # Unit tests only
just test-integration       # Integration tests
just test-e2e               # E2E tests (auto-starts Docker server)
```

### For CI/CD

Same fast approach works great in CI/CD:

```bash
just test-e2e               # E2E tests with Docker server
```

## Test Types

### 1. Unit Tests

Fast tests that verify individual components in isolation.

```bash
# Run all unit tests
cargo test --lib --all-features

# Or via just
just test-unit
```

### 2. Integration Tests

Tests that verify components working together, may use test utilities.

```bash
# Run integration tests
cargo test --test '*' --features test-utils -- --nocapture

# Or via just
just test-integration
```

### 3. End-to-End (E2E) Tests

Full system tests connecting to a real SPICE server using native Rust client with Docker server (best of both worlds):

#### E2E Tests (Fast, Recommended)

**Advantages:**
- ⚡ Fast - native Rust client (no Docker client overhead)
- 🔒 Reproducible - Docker server for consistent environment
- 🔄 Quick iteration cycle
- 🐛 Easy debugging with native tools
- 💻 All-in-one - auto-starts server

**Server Types:**
- **Docker debug server** (default): Lightweight, fast startup, good for protocol testing
- **QEMU Ubuntu VM**: Realistic environment with real QEMU/KVM, slower but more thorough

**Prerequisites:**
- Rust toolchain
- Docker (for server)
- For GTK4 tests: libgtk-4-dev, libgstreamer1.0-dev

**Running E2E Tests:**

```bash
# Option 1: Auto-start Docker debug server (fast, recommended)
just test-e2e

# Option 2: Test with QEMU Ubuntu VM (realistic, slower)
just test-e2e-qemu

# Option 3: Test against existing server
./run-e2e-tests.sh basic none --host 192.168.1.100 --port 5900

# Option 4: Test with local system SPICE server
just test-e2e-system

# Option 5: Test all implementations
just test-e2e-all

# Custom options
./run-e2e-tests.sh basic --duration 60 --verbose --min-updates 10
```

**Available Options:**
- `--duration N`: Test duration in seconds (default: 30)
- `--host HOST`: SPICE server address (default: localhost)
- `--port PORT`: SPICE server port (default: 5912)
- `--min-updates N`: Minimum display updates required (default: 1)
- `--verbose`, `-v`: Enable verbose output
- `--fail-fast`: Exit on first error
- `--trace-on-failure`: Save protocol traces on failure
- `--help`: Show all options

## Setting Up SPICE Test Server

### Option 1: Quick Setup (Automated)

The easiest way to get started:

```bash
# Build and install SPICE test server locally
just setup-test-server

# Or with custom install location
./setup-test-server.sh --prefix ~/.local

# Add to PATH if using custom location
export PATH="$HOME/.local/bin:$PATH"
```

This will:
1. Download SPICE protocol and server source
2. Build test utilities with Meson
3. Install to `build/install/bin/` (or custom prefix)
4. Create a wrapper script `spice-test-server`

### Option 2: System Package (If Available)

Some distributions provide SPICE development packages:

```bash
# Debian/Ubuntu
sudo apt-get install spice-protocol libspice-server-dev

# Fedora/RHEL
sudo dnf install spice-protocol-devel spice-server-devel

# Note: System packages may not include test utilities
```

### Option 3: Use Existing SPICE Server

If you have a VM with SPICE enabled:

```bash
# Test against existing server
./run-e2e-tests-native.sh --host <vm-host> --port 5900
```

## Running the Test Server

Once installed, start the test server:

```bash
# Using wrapper script (recommended)
spice-test-server

# Or directly
test-display-no-ssl

# Custom port
spice-test-server --port 5920

# Verbose mode
spice-test-server --verbose

# In background
spice-test-server &
TEST_SERVER_PID=$!
```

The server listens on port 5912 by default and provides:
- Main channel
- Display channel (with periodic updates)
- Inputs channel (keyboard/mouse)
- Cursor channel

## Test Workflow Examples

### Development Workflow

```bash
# 1. Start test server (one-time setup)
spice-test-server &

# 2. Make code changes
vim src/client.rs

# 3. Run quick unit tests
just test-unit

# 4. Run native E2E test
just test-e2e-native

# 5. If issues found, enable verbose logging
RUST_LOG=debug ./run-e2e-tests-native.sh -v

# 6. Check protocol traces if test failed
ls test-results/e2e_failure_*.trace
```

### Pre-Commit Workflow

```bash
# Run all tests before committing
just test-all

# Format and lint
cargo fmt
cargo clippy
```

### CI/CD Workflow

The GitHub Actions workflow automatically:
1. Sets up dependencies
2. Builds SPICE test server
3. Runs native E2E tests
4. Uploads logs on failure

See `.github/workflows/e2e-tests.yml` for details.

## Troubleshooting

### Test Server Won't Start

**Check if port is in use:**
```bash
netstat -tln | grep 5912
# or
lsof -i :5912
```

**Solution:** Kill existing process or use different port:
```bash
pkill -f test-display-no-ssl
# or
spice-test-server --port 5920
./run-e2e-tests-native.sh --port 5920
```

### Client Can't Connect

**Check server is running:**
```bash
nc -zv localhost 5912
```

**Check firewall:**
```bash
sudo ufw status
sudo ufw allow 5912/tcp
```

**Enable debug logging:**
```bash
RUST_LOG=debug ./run-e2e-tests-native.sh -vv
```

### Test Failures

**Save protocol traces:**
```bash
./run-e2e-tests-native.sh --trace-on-failure
# Check test-results/ for .trace files
```

**Increase timeout:**
```bash
./run-e2e-tests-native.sh --timeout 30 --duration 60
```

**Check server logs:**
```bash
tail -f test-server.log
```

### Build Failures

**Missing dependencies (Ubuntu/Debian):**
```bash
sudo apt-get install -y \
    meson ninja-build pkg-config \
    libglib2.0-dev libpixman-1-dev \
    libssl-dev libjpeg-dev
```

**Missing dependencies (Fedora/RHEL):**
```bash
sudo dnf install -y \
    meson ninja-build pkgconfig \
    glib2-devel pixman-devel \
    openssl-devel libjpeg-turbo-devel
```

**Missing dependencies (macOS):**
```bash
brew install meson ninja pkg-config glib pixman openssl jpeg
```

## Performance Comparison

Typical test execution times on a modern laptop:

| Test Type              | Docker   | Native   | Speedup |
|------------------------|----------|----------|---------|
| Quick E2E (30s test)   | ~90s     | ~35s     | 2.5x    |
| Full matrix            | ~10min   | ~3min    | 3.3x    |
| Iteration cycle        | ~45s     | ~5s      | 9x      |

*Iteration cycle = time from code change to test result*

## Best Practices

### For Development
1. ✅ Use native tests for rapid iteration
2. ✅ Keep test server running in background
3. ✅ Use `--fail-fast` to catch issues early
4. ✅ Enable `--trace-on-failure` for debugging
5. ✅ Run `just test-all` before committing

### For CI/CD
1. ✅ Use native tests for speed
2. ✅ Cache SPICE server build artifacts
3. ✅ Upload test logs on failure
4. ✅ Use matrix testing for comprehensive coverage

### For Debugging
1. ✅ Use native tests with verbose logging
2. ✅ Check protocol traces in test-results/
3. ✅ Use Wireshark for deep protocol analysis
4. ✅ Enable server debug output: `SPICE_DEBUG=1`

## Additional Resources

- SPICE Protocol Specification: https://www.spice-space.org/documentation.html
- SPICE Server Repository: https://gitlab.freedesktop.org/spice/spice
- Client Implementation: See `CLAUDE.md` for architecture details
- Protocol Messages: See `src/protocol.rs` for message definitions

## Contributing

When adding new test scenarios:

1. Add unit tests for new protocol features
2. Update E2E test expectations if needed
3. Document new test options in this guide
4. Ensure both native and Docker tests pass
5. Update CI workflow if new dependencies required