---
id: task-18
title: Create WASM web testing environment with QEMU
status: Done
assignee:
  - '@claude'
created_date: '2025-09-29 18:39'
updated_date: '2025-09-29 18:44'
labels:
  - spice-client
  - wasm
  - testing
  - web
dependencies: []
priority: medium
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Build a complete web testing environment for the WASM SPICE client that includes a proper web server and can automatically start both QEMU Ubuntu VM and the web interface for browser-based testing.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Create web server setup for serving WASM build
- [x] #2 Add script to build WASM with test configuration
- [x] #3 Implement option to start QEMU and web server together
- [x] #4 Add browser testing instructions to documentation
- [x] #5 Create justfile commands for easy WASM testing
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Research existing WASM infrastructure and web server options
2. Create a simple HTTP server for serving WASM files
3. Update build-wasm.sh to support test configuration
4. Create integrated script to start QEMU VM and web server together
5. Add justfile commands for WASM testing workflow
6. Document WASM browser testing setup
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Created comprehensive WASM browser testing environment for spice-client:

## What Was Implemented

### 1. Web Test Infrastructure (spice-client/web-test/)
- **index.html**: Full-featured browser testing interface with:
  - Connection controls (host/port configuration)
  - Real-time status display
  - HTML5 canvas for display rendering
  - Event logging console
  - Mouse and keyboard event handling

- **serve.py**: Python HTTP server with:
  - Proper WASM MIME types (application/wasm)
  - CORS headers for local testing
  - SharedArrayBuffer support headers
  - Configurable host/port
  - Clean command-line interface

### 2. Enhanced Build Script (build-wasm.sh)
Upgraded with:
- Three build modes: release, dev, test
- Configurable output directory
- Dependency checking (wasm-pack)
- Comprehensive help documentation
- Color-coded output
- Usage examples and next steps

### 3. Integrated Test Launcher (start-wasm-test.sh)
All-in-one script that:
- Builds WASM package
- Starts SPICE server (Docker/native/existing)
- Starts WebSocket proxy for WASM-to-TCP bridge
- Starts HTTP server for web interface
- Handles graceful cleanup on exit
- Supports multiple QEMU configurations
- Extensive configuration options

### 4. Justfile Commands
Added convenience commands:
- `just wasm-build`: Release build
- `just wasm-build-dev`: Development build
- `just wasm-build-test`: Test build with extra features
- `just wasm-test`: Start complete test environment
- `just wasm-test-port`: Custom web port
- `just wasm-test-existing`: Use existing SPICE server
- `just wasm-serve`: Web server only
- `just wasm-test-release`: Release build test

### 5. Comprehensive Documentation (WASM_BROWSER_TESTING.md)
38KB guide covering:
- Quick start instructions
- Architecture diagram and component overview
- Build modes and configuration
- Multiple testing scenarios
- Troubleshooting guide
- Performance testing tips
- Development workflow
- Testing checklist
- Advanced configuration
- Common use cases (CI/CD, multi-VM, production testing)

## Key Features

- **Zero-configuration testing**: Single command starts everything
- **Flexible deployment**: Docker, native, or existing server support
- **Developer-friendly**: Hot reload support, debug modes, verbose logging
- **Production-ready**: Release builds, performance testing, protocol tracing
- **Well-documented**: Complete guide with examples and troubleshooting

## Files Modified/Created

Created:
- spice-client/web-test/index.html
- spice-client/web-test/serve.py
- spice-client/start-wasm-test.sh
- spice-client/WASM_BROWSER_TESTING.md

Modified:
- spice-client/build-wasm.sh (enhanced with modes and help)
- spice-client/justfile (added WASM testing commands)

## Testing

The environment is ready for testing with:
```bash
cd spice-client
just wasm-test
# Then open http://localhost:8000 in browser
```
<!-- SECTION:NOTES:END -->
