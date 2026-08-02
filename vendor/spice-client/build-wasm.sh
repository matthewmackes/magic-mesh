#!/usr/bin/env bash
# Build script for WASM version of spice-client

set -e

# Configuration
BUILD_MODE="${1:-release}"  # release or dev
OUT_DIR="${2:-pkg}"

# Color output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

show_help() {
    cat << EOF
Build script for WASM version of spice-client

Usage: $(basename "$0") [MODE] [OUTPUT_DIR]

ARGUMENTS:
  MODE          Build mode: release (default) or dev
  OUTPUT_DIR    Output directory (default: pkg)

MODES:
  release       Optimized release build (default)
  dev           Development build with debug symbols
  test          Test build with additional features

EXAMPLES:
  $(basename "$0")                    # Release build to pkg/
  $(basename "$0") dev                # Dev build to pkg/
  $(basename "$0") test               # Test build to pkg/
  $(basename "$0") release web-test   # Release build to web-test/

OUTPUT:
  The build creates the following files in the output directory:
  - spice_client.js         # JavaScript bindings
  - spice_client_bg.wasm    # WebAssembly binary
  - spice_client.d.ts       # TypeScript definitions
  - package.json            # NPM package info

ENVIRONMENT:
  RUST_LOG      Set log level (debug, info, warn, error)

EOF
}

# Parse arguments
if [[ "$1" == "-h" ]] || [[ "$1" == "--help" ]]; then
    show_help
    exit 0
fi

echo -e "${BLUE}=== WASM Build Configuration ===${NC}"
echo "Build mode: $BUILD_MODE"
echo "Output directory: $OUT_DIR"
echo ""

# Ensure wasm-pack is installed
echo -e "${BLUE}Checking dependencies...${NC}"

# Add cargo bin to PATH if not already there
if [[ -d "$HOME/.cargo/bin" ]] && [[ ":$PATH:" != *":$HOME/.cargo/bin:"* ]]; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi

if ! command -v wasm-pack &> /dev/null; then
    echo -e "${YELLOW}wasm-pack not found. Installing...${NC}"
    cargo install wasm-pack
else
    echo -e "${GREEN}✓ wasm-pack found${NC}"
fi

# Build the WASM package
echo ""
echo -e "${BLUE}Building WASM package...${NC}"

case "$BUILD_MODE" in
    release)
        echo "Building optimized release version..."
        wasm-pack build --target web --out-dir "$OUT_DIR" --release
        ;;
    dev)
        echo "Building development version with debug symbols..."
        wasm-pack build --target web --out-dir "$OUT_DIR" --dev
        ;;
    test)
        echo "Building test version with additional features..."
        wasm-pack build --target web --out-dir "$OUT_DIR" --dev -- --features test-utils
        ;;
    *)
        echo "Error: Unknown build mode: $BUILD_MODE"
        echo "Valid modes: release, dev, test"
        exit 1
        ;;
esac

echo ""
echo -e "${GREEN}=== WASM Build Complete! ===${NC}"
echo ""
echo "Output files are in the $OUT_DIR/ directory:"
ls -lh "$OUT_DIR"/*.wasm "$OUT_DIR"/*.js 2>/dev/null || true
echo ""
echo "To test in browser:"
echo "  1. Start web server: cd spice-client && python3 web-test/serve.py"
echo "  2. Open browser: http://localhost:8000"
echo ""
echo "Or use justfile commands:"
echo "  just wasm-test    # Build and serve test environment"
echo ""