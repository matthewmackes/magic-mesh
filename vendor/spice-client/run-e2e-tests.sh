#!/usr/bin/env bash
# Native E2E test runner for SPICE client (no Docker)
# This script builds and runs tests directly on the host system

set -euo pipefail

# Color codes for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Default values
IMPLEMENTATION="basic"
SERVER="docker"  # Default to docker for AIO experience
TEST_DURATION="30"
CLEAN_AFTER="true"
VERBOSE=""
DRY_RUN=""
MIN_DISPLAY_UPDATES="1"
FAIL_FAST=""
TRACE_ON_FAILURE="true"
PROGRESS_INTERVAL="5"
CONNECT_TIMEOUT="10"
SPICE_HOST="localhost"
SPICE_PORT="5912"
SPICE_SERVER_PID=""
SPICE_CONTAINER_ID=""
USE_SYSTEM_SERVER="false"
USE_DOCKER_SERVER="false"

# Help function
show_help() {
    cat << EOF
SPICE Client Native E2E Test Runner (No Docker)

Usage: $(basename "$0") [implementation] [server] [options]

IMPLEMENTATIONS:
  basic         Basic client without multimedia (default)
  gtk4          GTK4 backend implementation
  all           Run all implementations

SERVERS:
  docker        Use Docker SPICE test server (default, AIO)
  qemu          Use QEMU with Ubuntu VM (realistic test)
  none          Assume server is already running
  system        Use system-installed SPICE test server
  build         Build and use SPICE test server from source

OPTIONS:
  -d, --duration TIME    Test duration in seconds (default: 30)
  -k, --keep            Don't clean up after tests
  -v, --verbose         Enable verbose output
  -n, --dry-run         Show what would be run without executing
  --min-updates N       Minimum display updates required (default: 1)
  --fail-fast           Exit immediately on first error
  --no-trace            Don't save protocol traces on failure
  --progress N          Progress report interval in seconds (default: 5)
  --timeout N           Connection timeout in seconds (default: 10)
  --host HOST           SPICE server host (default: localhost)
  --port PORT           SPICE server port (default: 5912)
  -h, --help            Show this help message

EXAMPLES:
  # Quick test (auto-starts Docker server - AIO)
  $(basename "$0")

  # Test with existing server
  $(basename "$0") basic none --host 192.168.1.100 --port 5900

  # Test with system SPICE server
  $(basename "$0") basic system

  # Test with QEMU Ubuntu VM
  $(basename "$0") basic qemu

  # Test GTK4 implementation
  $(basename "$0") gtk4

  # Verbose test with longer duration
  $(basename "$0") basic docker -v -d 60

PREREQUISITES:
  - Rust toolchain (cargo)
  - Docker (for default docker server mode)
  - For GTK4 tests: libgtk-4-dev, libgstreamer1.0-dev
  - For system server: spice-protocol, libspice-server (optional)

EOF
}

# Parse command line arguments
parse_args() {
    while [[ $# -gt 0 ]]; do
        case $1 in
            # Implementations
            basic|gtk4|all)
                IMPLEMENTATION="$1"
                shift
                ;;
            # Servers
            docker|qemu|none|system|build)
                SERVER="$1"
                shift
                ;;
            # Options
            -d|--duration)
                TEST_DURATION="$2"
                shift 2
                ;;
            -k|--keep)
                CLEAN_AFTER="false"
                shift
                ;;
            -v|--verbose)
                VERBOSE="-v"
                shift
                ;;
            -n|--dry-run)
                DRY_RUN="true"
                shift
                ;;
            --min-updates)
                MIN_DISPLAY_UPDATES="$2"
                shift 2
                ;;
            --fail-fast)
                FAIL_FAST="true"
                shift
                ;;
            --no-trace)
                TRACE_ON_FAILURE="false"
                shift
                ;;
            --progress)
                PROGRESS_INTERVAL="$2"
                shift 2
                ;;
            --timeout)
                CONNECT_TIMEOUT="$2"
                shift 2
                ;;
            --host)
                SPICE_HOST="$2"
                shift 2
                ;;
            --port)
                SPICE_PORT="$2"
                shift 2
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
}

# Print test configuration
print_config() {
    echo -e "${BLUE}=== SPICE Native E2E Test Configuration ===${NC}"
    echo "Implementation: $IMPLEMENTATION"
    echo "Server: $SERVER"
    echo "Server address: ${SPICE_HOST}:${SPICE_PORT}"
    echo "Duration: ${TEST_DURATION}s"
    echo "Clean after: $CLEAN_AFTER"
    echo "Verbose: ${VERBOSE:-false}"
    echo "Min display updates: $MIN_DISPLAY_UPDATES"
    echo "Fail fast: ${FAIL_FAST:-false}"
    echo "Trace on failure: $TRACE_ON_FAILURE"
    echo "Progress interval: ${PROGRESS_INTERVAL}s"
    echo "Connect timeout: ${CONNECT_TIMEOUT}s"
    echo ""
}

# Check prerequisites
check_prerequisites() {
    echo -e "${BLUE}Checking prerequisites...${NC}"

    if ! command -v cargo &> /dev/null; then
        echo -e "${RED}Error: cargo not found. Please install Rust toolchain.${NC}"
        exit 1
    fi

    if [[ "$IMPLEMENTATION" == "gtk4" || "$IMPLEMENTATION" == "all" ]]; then
        if ! pkg-config --exists gtk4; then
            echo -e "${YELLOW}Warning: GTK4 not found. GTK4 tests will be skipped.${NC}"
        fi
    fi

    if [[ "$SERVER" == "system" ]]; then
        if ! command -v test-display-no-ssl &> /dev/null; then
            echo -e "${RED}Error: test-display-no-ssl not found in PATH.${NC}"
            echo "Please install SPICE server or use 'none' server option."
            exit 1
        fi
    fi

    echo -e "${GREEN}✓ Prerequisites check passed${NC}"
}

# Build test binaries
build_tests() {
    local impl=$1
    echo -e "${YELLOW}Building test binary for: $impl${NC}"

    if [[ -n "$DRY_RUN" ]]; then
        echo -e "${BLUE}Would run: cargo build --release --bin spice-e2e-test${NC}"
        return 0
    fi

    local features=""
    case $impl in
        gtk4)
            features="--features backend-gtk4"
            ;;
    esac

    if cargo build --release --bin spice-e2e-test --no-default-features $features; then
        echo -e "${GREEN}✓ Build succeeded${NC}"
        return 0
    else
        echo -e "${RED}✗ Build failed${NC}"
        return 1
    fi
}

# Start SPICE test server
start_server() {
    if [[ "$SERVER" == "none" ]]; then
        echo -e "${BLUE}Assuming SPICE server is already running on ${SPICE_HOST}:${SPICE_PORT}${NC}"
        return 0
    fi

    if [[ "$SERVER" == "docker" ]]; then
        echo -e "${YELLOW}Starting SPICE test server with Docker...${NC}"

        if ! command -v docker &> /dev/null; then
            echo -e "${RED}Error: docker not found. Please install Docker or use 'system' server option.${NC}"
            return 1
        fi

        if [[ -n "$DRY_RUN" ]]; then
            echo -e "${BLUE}Would run: docker run -d --rm -p ${SPICE_PORT}:5912 spice-debug-server${NC}"
            return 0
        fi

        # Check if image exists, build if not
        if ! docker image inspect quickemu-manager-spice-debug-server &> /dev/null; then
            echo -e "${BLUE}Building SPICE debug server image...${NC}"
            if ! docker build -f docker/Dockerfile.spice-debug -t quickemu-manager-spice-debug-server . ; then
                echo -e "${RED}✗ Failed to build Docker image${NC}"
                return 1
            fi
        fi

        # Start container
        SPICE_CONTAINER_ID=$(docker run -d --rm -p "${SPICE_PORT}:5912" quickemu-manager-spice-debug-server)
        if [[ -z "$SPICE_CONTAINER_ID" ]]; then
            echo -e "${RED}✗ Failed to start Docker container${NC}"
            return 1
        fi

        USE_DOCKER_SERVER="true"

        # Wait for server to start
        echo -e "${BLUE}Waiting for server to start...${NC}"
        local retries=15
        while [[ $retries -gt 0 ]]; do
            if nc -z "$SPICE_HOST" "$SPICE_PORT" 2>/dev/null; then
                echo -e "${GREEN}✓ SPICE server started (container: ${SPICE_CONTAINER_ID:0:12})${NC}"
                return 0
            fi
            retries=$((retries - 1))
            sleep 1
        done

        echo -e "${RED}✗ SPICE server failed to start within timeout${NC}"
        docker logs "$SPICE_CONTAINER_ID"
        docker stop "$SPICE_CONTAINER_ID" &> /dev/null
        return 1
    fi

    if [[ "$SERVER" == "qemu" ]]; then
        echo -e "${YELLOW}Starting QEMU with Ubuntu VM (SPICE server)...${NC}"

        if ! command -v docker &> /dev/null; then
            echo -e "${RED}Error: docker not found. Please install Docker.${NC}"
            return 1
        fi

        if [[ -n "$DRY_RUN" ]]; then
            echo -e "${BLUE}Would run: docker run -d --rm -p ${SPICE_PORT}:5900 spice-qemu-server${NC}"
            return 0
        fi

        # Override port for QEMU (uses 5900)
        SPICE_PORT="5900"

        # Check if image exists, build if not
        if ! docker image inspect quickemu-manager-spice-qemu-server &> /dev/null; then
            echo -e "${BLUE}Building QEMU SPICE server image...${NC}"
            if ! docker build -f tests/docker/Dockerfile.spice-server -t quickemu-manager-spice-qemu-server tests/docker/ ; then
                echo -e "${RED}✗ Failed to build Docker image${NC}"
                return 1
            fi
        fi

        # Start container (QEMU needs more resources)
        SPICE_CONTAINER_ID=$(docker run -d --rm -p "${SPICE_PORT}:5900" quickemu-manager-spice-qemu-server)
        if [[ -z "$SPICE_CONTAINER_ID" ]]; then
            echo -e "${RED}✗ Failed to start Docker container${NC}"
            return 1
        fi

        USE_DOCKER_SERVER="true"

        # Wait for server to start (QEMU takes longer)
        echo -e "${BLUE}Waiting for QEMU VM to boot (this may take 30-60 seconds)...${NC}"
        local retries=60
        while [[ $retries -gt 0 ]]; do
            if nc -z "$SPICE_HOST" "$SPICE_PORT" 2>/dev/null; then
                echo -e "${GREEN}✓ QEMU SPICE server started (container: ${SPICE_CONTAINER_ID:0:12})${NC}"
                # Give QEMU a bit more time to fully initialize
                sleep 3
                return 0
            fi
            retries=$((retries - 1))
            sleep 1
        done

        echo -e "${RED}✗ QEMU SPICE server failed to start within timeout${NC}"
        docker logs "$SPICE_CONTAINER_ID"
        docker stop "$SPICE_CONTAINER_ID" &> /dev/null
        return 1
    fi

    if [[ "$SERVER" == "system" ]]; then
        echo -e "${YELLOW}Starting system SPICE test server on port ${SPICE_PORT}...${NC}"

        if [[ -n "$DRY_RUN" ]]; then
            echo -e "${BLUE}Would run: test-display-no-ssl --port ${SPICE_PORT}${NC}"
            return 0
        fi

        # Start the test server in background
        test-display-no-ssl --port "$SPICE_PORT" > test-server.log 2>&1 &
        SPICE_SERVER_PID=$!
        USE_SYSTEM_SERVER="true"

        # Wait for server to start
        echo -e "${BLUE}Waiting for server to start...${NC}"
        local retries=10
        while [[ $retries -gt 0 ]]; do
            if nc -z "$SPICE_HOST" "$SPICE_PORT" 2>/dev/null; then
                echo -e "${GREEN}✓ SPICE server started (PID: $SPICE_SERVER_PID)${NC}"
                return 0
            fi
            retries=$((retries - 1))
            sleep 1
        done

        echo -e "${RED}✗ SPICE server failed to start${NC}"
        cat test-server.log
        return 1
    fi

    if [[ "$SERVER" == "build" ]]; then
        echo -e "${RED}Error: Building SPICE server from source is not yet implemented.${NC}"
        echo "Please use 'none' or 'system' server option."
        return 1
    fi
}

# Stop SPICE test server
stop_server() {
    if [[ "$USE_DOCKER_SERVER" == "true" ]] && [[ -n "$SPICE_CONTAINER_ID" ]]; then
        echo -e "${YELLOW}Stopping SPICE Docker container (${SPICE_CONTAINER_ID:0:12})...${NC}"
        if [[ -n "$DRY_RUN" ]]; then
            echo -e "${BLUE}Would run: docker stop $SPICE_CONTAINER_ID${NC}"
            return 0
        fi

        docker stop "$SPICE_CONTAINER_ID" &> /dev/null || true
        echo -e "${GREEN}✓ SPICE server stopped${NC}"
    fi

    if [[ "$USE_SYSTEM_SERVER" == "true" ]] && [[ -n "$SPICE_SERVER_PID" ]]; then
        echo -e "${YELLOW}Stopping SPICE server (PID: $SPICE_SERVER_PID)...${NC}"
        if [[ -n "$DRY_RUN" ]]; then
            echo -e "${BLUE}Would run: kill $SPICE_SERVER_PID${NC}"
            return 0
        fi

        kill "$SPICE_SERVER_PID" 2>/dev/null || true
        wait "$SPICE_SERVER_PID" 2>/dev/null || true
        echo -e "${GREEN}✓ SPICE server stopped${NC}"
    fi
}

# Run a single test
run_test() {
    local impl=$1
    local test_name="${impl}_native"

    echo -e "${YELLOW}Running test: $test_name${NC}"

    # Build test binary
    if ! build_tests "$impl"; then
        return 1
    fi

    # Construct test command (check both locations)
    local test_cmd=""
    if [[ -f "./target/release/spice-e2e-test" ]]; then
        test_cmd="./target/release/spice-e2e-test"
    elif [[ -f "../target/release/spice-e2e-test" ]]; then
        test_cmd="../target/release/spice-e2e-test"
    else
        echo -e "${RED}✗ Binary not found at ./target/release/spice-e2e-test or ../target/release/spice-e2e-test${NC}"
        return 1
    fi
    test_cmd="$test_cmd --host $SPICE_HOST"
    test_cmd="$test_cmd --port $SPICE_PORT"
    test_cmd="$test_cmd --duration $TEST_DURATION"
    test_cmd="$test_cmd --min-display-updates $MIN_DISPLAY_UPDATES"
    test_cmd="$test_cmd --progress-interval $PROGRESS_INTERVAL"
    test_cmd="$test_cmd --connect-timeout $CONNECT_TIMEOUT"
    test_cmd="$test_cmd --require-display-channel --require-display-updates"

    if [[ -n "$FAIL_FAST" ]]; then
        test_cmd="$test_cmd --fail-fast"
    fi

    if [[ "$TRACE_ON_FAILURE" == "true" ]]; then
        test_cmd="$test_cmd --trace-on-failure"
    fi

    if [[ -n "$VERBOSE" ]]; then
        test_cmd="$test_cmd -vv"
    fi

    if [[ -n "$DRY_RUN" ]]; then
        echo -e "${BLUE}Would run: $test_cmd${NC}"
        return 0
    fi

    # Run the test with timeout
    local test_timeout=$((TEST_DURATION + CONNECT_TIMEOUT + 10))
    echo -e "${BLUE}Running test with ${test_timeout}s timeout${NC}"

    if timeout "${test_timeout}" $test_cmd; then
        echo -e "${GREEN}✓ Test $test_name passed${NC}"
        return 0
    else
        local exit_code=$?
        if [[ $exit_code -eq 124 ]]; then
            echo -e "${RED}✗ Test $test_name timed out after ${test_timeout}s${NC}"
        else
            echo -e "${RED}✗ Test $test_name failed with exit code $exit_code${NC}"
        fi

        # Check for trace files
        if [[ "$TRACE_ON_FAILURE" == "true" ]] && ls e2e_failure_*.trace 1> /dev/null 2>&1; then
            mkdir -p test-results
            mv e2e_failure_*.trace test-results/
            echo -e "${YELLOW}Protocol traces saved to test-results/${NC}"
        fi

        return 1
    fi
}

# Main test runner
main() {
    parse_args "$@"
    print_config
    check_prerequisites

    # Change to spice-client directory
    cd "$(dirname "$0")"

    # Create results directory
    mkdir -p test-results

    # Determine which tests to run
    local implementations=()
    case $IMPLEMENTATION in
        all)
            implementations=("basic" "gtk4")
            ;;
        *)
            implementations=("$IMPLEMENTATION")
            ;;
    esac

    # Start server if needed
    if ! start_server; then
        echo -e "${RED}Failed to start SPICE server${NC}"
        exit 1
    fi

    # Setup cleanup trap
    trap 'stop_server' EXIT INT TERM

    # Track test results
    local total_tests=0
    local passed_tests=0
    local failed_tests=()

    # Run tests
    for impl in "${implementations[@]}"; do
        total_tests=$((total_tests + 1))

        if run_test "$impl"; then
            passed_tests=$((passed_tests + 1))
        else
            failed_tests+=("$impl")

            # Early exit if fail-fast is enabled
            if [[ "$FAIL_FAST" == "true" ]]; then
                echo -e "${YELLOW}Fail-fast enabled, stopping after first failure${NC}"
                break
            fi
        fi

        # Small delay between tests
        if [[ ${#implementations[@]} -gt 1 ]]; then
            sleep 2
        fi
    done

    # Cleanup
    if [[ "$CLEAN_AFTER" == "true" ]]; then
        stop_server
    fi

    # Print summary
    echo ""
    echo -e "${BLUE}=== Test Summary ===${NC}"
    echo "Total tests: $total_tests"
    echo -e "${GREEN}Passed: $passed_tests${NC}"
    echo -e "${RED}Failed: ${#failed_tests[@]}${NC}"

    if [[ ${#failed_tests[@]} -gt 0 ]]; then
        echo ""
        echo -e "${RED}Failed tests:${NC}"
        for test in "${failed_tests[@]}"; do
            echo "  - $test"
        done
        exit 1
    else
        echo -e "${GREEN}All tests passed!${NC}"
        exit 0
    fi
}

# Run main function
main "$@"