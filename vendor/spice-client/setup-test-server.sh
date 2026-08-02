#!/usr/bin/env bash
# Setup script for SPICE test server (native, no Docker)
# This script builds the SPICE server test utilities locally

set -euo pipefail

# Color codes
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Configuration
BUILD_DIR="${BUILD_DIR:-build/spice-server}"
INSTALL_PREFIX="${INSTALL_PREFIX:-$PWD/build/install}"
SPICE_PROTOCOL_VERSION="${SPICE_PROTOCOL_VERSION:-master}"
SPICE_VERSION="${SPICE_VERSION:-master}"

# Help function
show_help() {
    cat << EOF
SPICE Test Server Setup Script

This script downloads and builds the SPICE server test utilities locally.
No root access or system-wide installation required.

Usage: $(basename "$0") [options]

OPTIONS:
  --build-dir DIR       Build directory (default: build/spice-server)
  --prefix DIR          Installation prefix (default: build/install)
  --clean               Clean previous builds before building
  -h, --help           Show this help message

ENVIRONMENT VARIABLES:
  BUILD_DIR             Override default build directory
  INSTALL_PREFIX        Override default installation prefix

PREREQUISITES:
  - git
  - meson (>= 0.50)
  - ninja-build
  - pkg-config
  - libglib2.0-dev
  - libpixman-1-dev
  - libssl-dev
  - libjpeg-dev

EXAMPLES:
  # Basic build
  $(basename "$0")

  # Clean build
  $(basename "$0") --clean

  # Custom installation directory
  $(basename "$0") --prefix ~/.local

After building, the test server will be available at:
  \$PREFIX/bin/test-display-no-ssl

EOF
}

# Parse arguments
CLEAN_BUILD=""
while [[ $# -gt 0 ]]; do
    case $1 in
        --build-dir)
            BUILD_DIR="$2"
            shift 2
            ;;
        --prefix)
            INSTALL_PREFIX="$2"
            shift 2
            ;;
        --clean)
            CLEAN_BUILD="true"
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

echo -e "${BLUE}=== SPICE Test Server Setup ===${NC}"
echo "Build directory: $BUILD_DIR"
echo "Install prefix: $INSTALL_PREFIX"
echo ""

# Check prerequisites
echo -e "${BLUE}Checking prerequisites...${NC}"

MISSING_DEPS=()

check_command() {
    if ! command -v "$1" &> /dev/null; then
        MISSING_DEPS+=("$1")
        echo -e "${RED}✗ $1 not found${NC}"
        return 1
    else
        echo -e "${GREEN}✓ $1 found${NC}"
        return 0
    fi
}

check_pkg_config() {
    if ! pkg-config --exists "$1"; then
        MISSING_DEPS+=("$1")
        echo -e "${RED}✗ $1 not found${NC}"
        return 1
    else
        echo -e "${GREEN}✓ $1 found${NC}"
        return 0
    fi
}

check_command git
check_command meson
check_command ninja
check_command pkg-config
check_pkg_config glib-2.0
check_pkg_config pixman-1

# Optional dependencies
pkg-config --exists openssl && echo -e "${GREEN}✓ openssl found${NC}" || echo -e "${YELLOW}⚠ openssl not found (optional)${NC}"
pkg-config --exists libjpeg && echo -e "${GREEN}✓ libjpeg found${NC}" || echo -e "${YELLOW}⚠ libjpeg not found (optional)${NC}"

if [[ ${#MISSING_DEPS[@]} -gt 0 ]]; then
    echo ""
    echo -e "${RED}Missing required dependencies:${NC}"
    for dep in "${MISSING_DEPS[@]}"; do
        echo "  - $dep"
    done
    echo ""
    echo "On Debian/Ubuntu, install with:"
    echo "  sudo apt-get install git meson ninja-build pkg-config libglib2.0-dev libpixman-1-dev libssl-dev libjpeg-dev"
    echo ""
    echo "On Fedora/RHEL, install with:"
    echo "  sudo dnf install git meson ninja-build pkgconfig glib2-devel pixman-devel openssl-devel libjpeg-turbo-devel"
    echo ""
    echo "On macOS, install with:"
    echo "  brew install git meson ninja pkg-config glib pixman openssl jpeg"
    exit 1
fi

echo -e "${GREEN}All prerequisites satisfied${NC}"
echo ""

# Clean if requested
if [[ -n "$CLEAN_BUILD" ]]; then
    echo -e "${YELLOW}Cleaning previous builds...${NC}"
    rm -rf "$BUILD_DIR"
    rm -rf "$INSTALL_PREFIX"
    echo -e "${GREEN}✓ Clean complete${NC}"
fi

# Create directories
mkdir -p "$BUILD_DIR"
mkdir -p "$INSTALL_PREFIX"

# Build spice-protocol
echo -e "${BLUE}Building spice-protocol...${NC}"
PROTOCOL_DIR="$BUILD_DIR/spice-protocol"

if [[ ! -d "$PROTOCOL_DIR" ]]; then
    git clone https://gitlab.freedesktop.org/spice/spice-protocol.git "$PROTOCOL_DIR"
fi

cd "$PROTOCOL_DIR"
git fetch origin
git checkout "$SPICE_PROTOCOL_VERSION"

if [[ ! -d "builddir" ]] || [[ -n "$CLEAN_BUILD" ]]; then
    rm -rf builddir
    meson setup builddir --prefix="$INSTALL_PREFIX"
fi

meson compile -C builddir
meson install -C builddir

echo -e "${GREEN}✓ spice-protocol installed${NC}"
cd - > /dev/null

# Build SPICE server
echo -e "${BLUE}Building SPICE server...${NC}"
SPICE_DIR="$BUILD_DIR/spice"

if [[ ! -d "$SPICE_DIR" ]]; then
    git clone https://gitlab.freedesktop.org/spice/spice.git "$SPICE_DIR"
fi

cd "$SPICE_DIR"
git fetch origin
git checkout "$SPICE_VERSION"

if [[ ! -d "builddir" ]] || [[ -n "$CLEAN_BUILD" ]]; then
    rm -rf builddir
    # Configure with minimal features needed for test server
    PKG_CONFIG_PATH="$INSTALL_PREFIX/lib/pkgconfig:$PKG_CONFIG_PATH" \
    meson setup builddir \
        --prefix="$INSTALL_PREFIX" \
        -Dgstreamer=no \
        -Dopus=disabled \
        -Dsasl=false \
        -Dmanual=false \
        -Dtests=true
fi

meson compile -C builddir

# Install test binaries
echo -e "${BLUE}Installing test binaries...${NC}"
mkdir -p "$INSTALL_PREFIX/bin"

# Copy test server binary
if [[ -f "builddir/server/tests/test-display-no-ssl" ]]; then
    cp builddir/server/tests/test-display-no-ssl "$INSTALL_PREFIX/bin/"
    chmod +x "$INSTALL_PREFIX/bin/test-display-no-ssl"
    echo -e "${GREEN}✓ test-display-no-ssl installed${NC}"
else
    echo -e "${RED}✗ test-display-no-ssl not found${NC}"
    exit 1
fi

cd - > /dev/null

# Create convenience wrapper script
WRAPPER_SCRIPT="$INSTALL_PREFIX/bin/spice-test-server"
cat > "$WRAPPER_SCRIPT" << 'WRAPPER_EOF'
#!/usr/bin/env bash
# Wrapper script for SPICE test server

# Get the directory where this script is located
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALL_PREFIX="$(dirname "$SCRIPT_DIR")"

# Update library path to find SPICE libraries
export LD_LIBRARY_PATH="$INSTALL_PREFIX/lib:${LD_LIBRARY_PATH:-}"

# Default values
PORT=5912
HOST="0.0.0.0"
VERBOSE=""

show_help() {
    cat << EOF
SPICE Test Server Wrapper

Usage: $(basename "$0") [options]

OPTIONS:
  -p, --port PORT      Server port (default: 5912)
  -h, --host HOST      Bind address (default: 0.0.0.0)
  -v, --verbose        Enable verbose output
  --help              Show this help message

EXAMPLES:
  # Start on default port 5912
  $(basename "$0")

  # Start on custom port
  $(basename "$0") --port 5920

  # Bind to localhost only
  $(basename "$0") --host 127.0.0.1

EOF
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -p|--port)
            PORT="$2"
            shift 2
            ;;
        -h|--host)
            HOST="$2"
            shift 2
            ;;
        -v|--verbose)
            VERBOSE="1"
            shift
            ;;
        --help)
            show_help
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            show_help
            exit 1
            ;;
    esac
done

# Set environment variables
export SPICE_DEBUG="${VERBOSE}"
export G_MESSAGES_DEBUG="${VERBOSE:+all}"

echo "Starting SPICE test server on ${HOST}:${PORT}"
exec "$SCRIPT_DIR/test-display-no-ssl"
WRAPPER_EOF

chmod +x "$WRAPPER_SCRIPT"

echo ""
echo -e "${GREEN}=== Build Complete ===${NC}"
echo ""
echo "SPICE test server installed to: $INSTALL_PREFIX"
echo ""
echo "To use the test server:"
echo "  1. Add to PATH: export PATH=\"$INSTALL_PREFIX/bin:\$PATH\""
echo "  2. Run server: spice-test-server"
echo "  3. Or directly: $INSTALL_PREFIX/bin/test-display-no-ssl"
echo ""
echo "Quick test:"
echo "  $INSTALL_PREFIX/bin/spice-test-server &"
echo "  cargo run --bin spice-e2e-test -- --host localhost --port 5912"
echo ""