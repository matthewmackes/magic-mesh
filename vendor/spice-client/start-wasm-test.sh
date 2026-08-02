#!/usr/bin/env bash
# Integrated script to start QEMU VM and web server for WASM testing

set -euo pipefail

# Color codes
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
MAGENTA='\033[0;35m'
NC='\033[0m'

# Default configuration
WEB_PORT="${WEB_PORT:-8000}"
SPICE_PORT="${SPICE_PORT:-5900}"
WS_PROXY_PORT="${WS_PROXY_PORT:-8080}"
BUILD_MODE="${BUILD_MODE:-dev}"
QEMU_TYPE="${QEMU_TYPE:-docker-debug}"  # docker-debug, docker-qemu, native, or none
CLEANUP_ON_EXIT="${CLEANUP_ON_EXIT:-true}"

# Tracking PIDs for cleanup
WEB_SERVER_PID=""
WS_PROXY_PID=""
QEMU_PID=""

show_help() {
    cat << EOF
${BLUE}WASM SPICE Client Test Environment Launcher${NC}

This script sets up a complete testing environment for the WASM SPICE client:
1. Builds WASM package
2. Starts QEMU VM with SPICE server (optional)
3. Starts WebSocket proxy for WASM-to-SPICE communication
4. Starts HTTP server for web interface

Usage: $(basename "$0") [OPTIONS]

OPTIONS:
  --web-port PORT           HTTP server port (default: 8000)
  --spice-port PORT         SPICE server port (default: 5900)
  --ws-port PORT            WebSocket proxy port (default: 8080)
  --build-mode MODE         WASM build mode: dev, release, test (default: dev)
  --qemu TYPE               QEMU type: docker, native, none (default: docker)
  --no-cleanup              Don't cleanup on exit
  --skip-build              Skip WASM build step
  -h, --help                Show this help

QEMU TYPES:
  docker-debug  Use Docker debug SPICE server (lightweight, fast, default)
  docker-qemu   Use Docker QEMU Ubuntu VM (realistic, with display)
  native        Use local QEMU installation (requires QEMU installed)
  none          Don't start QEMU, connect to existing SPICE server

EXAMPLES:
  # Start with debug server (fast, for protocol testing)
  $(basename "$0")

  # Start with QEMU Ubuntu VM (realistic, with display)
  $(basename "$0") --qemu docker-qemu

  # Use existing SPICE server
  $(basename "$0") --qemu none

  # Custom ports
  $(basename "$0") --web-port 3000 --spice-port 5901

  # Production test with release build and QEMU
  $(basename "$0") --build-mode release --qemu docker-qemu

ENVIRONMENT VARIABLES:
  WEB_PORT          Override default web server port
  SPICE_PORT        Override default SPICE port
  WS_PROXY_PORT     Override default WebSocket proxy port
  BUILD_MODE        Override default build mode
  QEMU_TYPE         Override default QEMU type

USAGE:
  After starting, open your browser to:
    http://localhost:${WEB_PORT}

  Press Ctrl+C to stop all services.

EOF
}

# Parse arguments
SKIP_BUILD=""
while [[ $# -gt 0 ]]; do
    case $1 in
        --web-port)
            WEB_PORT="$2"
            shift 2
            ;;
        --spice-port)
            SPICE_PORT="$2"
            shift 2
            ;;
        --ws-port)
            WS_PROXY_PORT="$2"
            shift 2
            ;;
        --build-mode)
            BUILD_MODE="$2"
            shift 2
            ;;
        --qemu)
            QEMU_TYPE="$2"
            shift 2
            ;;
        --no-cleanup)
            CLEANUP_ON_EXIT="false"
            shift
            ;;
        --skip-build)
            SKIP_BUILD="true"
            shift
            ;;
        -h|--help)
            show_help
            exit 0
            ;;
        *)
            echo -e "${RED}Unknown option: $1${NC}"
            show_help
            exit 1
            ;;
    esac
done

# Cleanup function
cleanup() {
    echo -e "\n${YELLOW}Shutting down services...${NC}"

    # Kill web server and its children
    if [[ -n "$WEB_SERVER_PID" ]]; then
        echo "Stopping web server (PID: $WEB_SERVER_PID)..."
        # Kill process group to get all children
        kill -TERM -$WEB_SERVER_PID 2>/dev/null || kill -TERM $WEB_SERVER_PID 2>/dev/null || true
        sleep 0.5
        # Force kill if still running
        kill -9 -$WEB_SERVER_PID 2>/dev/null || kill -9 $WEB_SERVER_PID 2>/dev/null || true
    fi

    # Kill WebSocket proxy and its children
    if [[ -n "$WS_PROXY_PID" ]]; then
        echo "Stopping WebSocket proxy (PID: $WS_PROXY_PID)..."
        kill -TERM -$WS_PROXY_PID 2>/dev/null || kill -TERM $WS_PROXY_PID 2>/dev/null || true
        sleep 0.5
        kill -9 -$WS_PROXY_PID 2>/dev/null || kill -9 $WS_PROXY_PID 2>/dev/null || true
    fi

    # Kill QEMU if started natively
    if [[ -n "$QEMU_PID" ]]; then
        echo "Stopping native QEMU (PID: $QEMU_PID)..."
        kill -TERM $QEMU_PID 2>/dev/null || true
        sleep 1
        kill -9 $QEMU_PID 2>/dev/null || true
        rm -f /tmp/spice-wasm-test.pid 2>/dev/null || true
    fi

    # Stop Docker containers quickly
    if [[ "$QEMU_TYPE" == "docker-debug" || "$QEMU_TYPE" == "docker" ]]; then
        echo "Stopping Docker containers..."
        # Use stop with timeout instead of down for faster cleanup
        cd docker 2>/dev/null || true
        docker compose --profile server-debug stop -t 2 2>/dev/null || true
        docker compose --profile server-debug rm -f 2>/dev/null || true
        cd - >/dev/null 2>&1 || true
    elif [[ "$QEMU_TYPE" == "docker-qemu" ]]; then
        echo "Stopping Docker QEMU containers..."
        cd docker 2>/dev/null || true
        docker compose --profile server-qemu stop -t 5 2>/dev/null || true
        docker compose --profile server-qemu rm -f 2>/dev/null || true
        cd - >/dev/null 2>&1 || true
    fi

    # Final cleanup: kill any lingering processes on our ports
    for port in $WEB_PORT $WS_PROXY_PORT; do
        lsof -ti:$port 2>/dev/null | xargs -r kill -9 2>/dev/null || true
    done

    echo -e "${GREEN}Cleanup complete${NC}"
}

# Register cleanup on exit
if [[ "$CLEANUP_ON_EXIT" == "true" ]]; then
    trap cleanup EXIT INT TERM
fi

echo -e "${BLUE}=== WASM SPICE Client Test Environment ===${NC}"
echo "Configuration:"
echo "  Web server port: $WEB_PORT"
echo "  SPICE port: $SPICE_PORT"
echo "  WebSocket proxy port: $WS_PROXY_PORT"
echo "  Build mode: $BUILD_MODE"
echo "  QEMU type: $QEMU_TYPE"
echo ""

# Step 1: Build WASM package
if [[ -z "$SKIP_BUILD" ]]; then
    echo -e "${BLUE}[1/4] Building WASM package...${NC}"
    ./build-wasm.sh "$BUILD_MODE" || {
        echo -e "${RED}WASM build failed${NC}"
        exit 1
    }
else
    echo -e "${YELLOW}[1/4] Skipping WASM build${NC}"
fi

# Step 2: Start QEMU/SPICE server
echo -e "${BLUE}[2/4] Starting SPICE server...${NC}"
case "$QEMU_TYPE" in
    docker-debug)
        echo "Starting Docker debug SPICE server on port $SPICE_PORT..."
        cd docker
        docker compose --profile server-debug up -d
        cd ..

        # Wait for server to be ready
        echo "Waiting for SPICE server to start..."
        for i in {1..30}; do
            if nc -z localhost "$SPICE_PORT" 2>/dev/null; then
                echo -e "${GREEN}✓ SPICE debug server ready${NC}"
                break
            fi
            sleep 1
            if [[ $i -eq 30 ]]; then
                echo -e "${RED}SPICE server failed to start${NC}"
                exit 1
            fi
        done
        ;;
    docker-qemu)
        echo "Starting Docker QEMU Ubuntu VM with SPICE on port $SPICE_PORT..."
        echo "Note: QEMU VM takes longer to start (~30-60 seconds)"
        cd docker
        docker compose --profile server-qemu up -d
        cd ..

        # Wait for server to be ready (QEMU takes longer)
        echo "Waiting for QEMU VM to start..."
        for i in {1..60}; do
            if nc -z localhost "$SPICE_PORT" 2>/dev/null; then
                echo -e "${GREEN}✓ QEMU SPICE server ready${NC}"
                break
            fi
            sleep 2
            if [[ $i -eq 60 ]]; then
                echo -e "${RED}QEMU SPICE server failed to start${NC}"
                echo "Check Docker logs: docker compose -f docker/docker-compose.yml logs spice-qemu-server"
                exit 1
            fi
        done
        ;;
    docker)
        # Backward compatibility - treat "docker" as "docker-debug"
        echo -e "${YELLOW}Note: 'docker' is deprecated, use 'docker-debug' or 'docker-qemu'${NC}"
        echo "Using docker-debug by default..."
        cd docker
        docker compose --profile server-debug up -d
        cd ..

        echo "Waiting for SPICE server to start..."
        for i in {1..30}; do
            if nc -z localhost "$SPICE_PORT" 2>/dev/null; then
                echo -e "${GREEN}✓ SPICE debug server ready${NC}"
                break
            fi
            sleep 1
            if [[ $i -eq 30 ]]; then
                echo -e "${RED}SPICE server failed to start${NC}"
                exit 1
            fi
        done
        ;;
    native)
        echo "Starting native QEMU with SPICE on port $SPICE_PORT..."

        # Check if QEMU is available
        if ! command -v qemu-system-x86_64 &> /dev/null; then
            echo -e "${RED}qemu-system-x86_64 not found${NC}"
            echo "Install QEMU first, then try again"
            exit 1
        fi

        # Create temporary disk image if it doesn't exist
        TEMP_DISK="/tmp/spice-test-vm.qcow2"
        if [[ ! -f "$TEMP_DISK" ]]; then
            echo "Creating temporary VM disk image..."
            qemu-img create -f qcow2 "$TEMP_DISK" 1G
        fi

        echo "Starting QEMU VM in background..."
        # Start QEMU with SPICE support
        qemu-system-x86_64 \
            -name "spice-wasm-test" \
            -machine pc,accel=kvm:tcg \
            -cpu host \
            -m 512 \
            -drive file="$TEMP_DISK",format=qcow2,if=virtio \
            -device qxl-vga,ram_size=67108864,vram_size=67108864 \
            -spice port="$SPICE_PORT",addr=127.0.0.1,disable-ticketing=on,image-compression=off \
            -device virtio-serial-pci \
            -device virtserialport,chardev=spicechannel0,name=com.redhat.spice.0 \
            -chardev spicevmc,id=spicechannel0,name=vdagent \
            -boot menu=on \
            -display none \
            -daemonize \
            -pidfile /tmp/spice-wasm-test.pid

        QEMU_PID=$(cat /tmp/spice-wasm-test.pid 2>/dev/null || echo "")

        # Wait for SPICE server to be ready
        echo "Waiting for QEMU SPICE server to start..."
        for i in {1..30}; do
            if nc -z localhost "$SPICE_PORT" 2>/dev/null; then
                echo -e "${GREEN}✓ Native QEMU SPICE server ready (PID: $QEMU_PID)${NC}"
                break
            fi
            sleep 1
            if [[ $i -eq 30 ]]; then
                echo -e "${RED}QEMU SPICE server failed to start${NC}"
                exit 1
            fi
        done
        ;;
    none)
        echo -e "${YELLOW}Skipping QEMU startup - expecting existing SPICE server on port $SPICE_PORT${NC}"
        # Verify server is available
        if ! nc -z localhost "$SPICE_PORT" 2>/dev/null; then
            echo -e "${RED}No SPICE server found on port $SPICE_PORT${NC}"
            echo "Start a SPICE server first or use --qemu docker-debug or --qemu docker-qemu"
            exit 1
        fi
        echo -e "${GREEN}✓ SPICE server detected${NC}"
        ;;
    *)
        echo -e "${RED}Unknown QEMU type: $QEMU_TYPE${NC}"
        echo "Valid types: docker-debug, docker-qemu, native, none"
        exit 1
        ;;
esac

# Step 3: Start WebSocket proxy
echo -e "${BLUE}[3/4] Starting WebSocket proxy (multiplexed)...${NC}"
# Use the new multiplexed proxy that supports multiple channels per session
if [[ -f "docker/websocket-proxy-multiplexed.py" ]]; then
    # Start in new process group for better cleanup
    # Note: multiplexed proxy uses environment variables, not command-line args
    WS_PORT="$WS_PROXY_PORT" SPICE_HOST="localhost" SPICE_PORT="$SPICE_PORT" \
        setsid python3 docker/websocket-proxy-multiplexed.py &
    WS_PROXY_PID=$!

    # Wait for proxy to start
    sleep 2
    if ! kill -0 "$WS_PROXY_PID" 2>/dev/null; then
        echo -e "${RED}WebSocket proxy failed to start${NC}"
        exit 1
    fi
    echo -e "${GREEN}✓ WebSocket proxy (multiplexed) running (PID: $WS_PROXY_PID)${NC}"
elif [[ -f "docker/websocket-proxy.py" ]]; then
    echo -e "${YELLOW}⚠ Using legacy proxy (single-channel mode)${NC}"
    WS_PORT="$WS_PROXY_PORT" SPICE_HOST="localhost" SPICE_PORT="$SPICE_PORT" \
        setsid python3 docker/websocket-proxy.py &
    WS_PROXY_PID=$!
    sleep 2
    if ! kill -0 "$WS_PROXY_PID" 2>/dev/null; then
        echo -e "${RED}WebSocket proxy failed to start${NC}"
        exit 1
    fi
    echo -e "${GREEN}✓ WebSocket proxy (legacy) running (PID: $WS_PROXY_PID)${NC}"
else
    echo -e "${RED}WebSocket proxy script not found${NC}"
    exit 1
fi

# Step 4: Start web server
echo -e "${BLUE}[4/4] Starting web server...${NC}"
# Start in new process group for better cleanup
setsid python3 web-test/serve.py --port "$WEB_PORT" --host 0.0.0.0 &
WEB_SERVER_PID=$!

# Wait for web server to start
sleep 2
if ! kill -0 "$WEB_SERVER_PID" 2>/dev/null; then
    echo -e "${RED}Web server failed to start${NC}"
    exit 1
fi
echo -e "${GREEN}✓ Web server running (PID: $WEB_SERVER_PID)${NC}"

# All services started
echo ""
echo -e "${GREEN}=== All Services Started! ===${NC}"
echo ""
echo -e "${MAGENTA}Open your browser to:${NC}"
echo -e "  ${BLUE}http://localhost:${WEB_PORT}${NC}"
echo ""
echo "Service URLs:"
echo "  Web interface:    http://localhost:${WEB_PORT}"
echo "  WebSocket proxy:  ws://localhost:${WS_PROXY_PORT}"
echo "  SPICE server:     localhost:${SPICE_PORT}"
echo ""
echo -e "${YELLOW}Press Ctrl+C to stop all services${NC}"
echo ""

# Wait indefinitely
wait