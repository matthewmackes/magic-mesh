#!/usr/bin/env python3
"""
Simple HTTP server for serving WASM SPICE client test page.
Includes proper MIME types for WASM and handles CORS.
"""

import argparse
import http.server
import os
import socketserver
import sys
from pathlib import Path


class WAsmHTTPRequestHandler(http.server.SimpleHTTPRequestHandler):
    """HTTP request handler with proper WASM MIME types and CORS headers."""

    def end_headers(self):
        # Enable CORS for local testing
        self.send_header('Access-Control-Allow-Origin', '*')
        self.send_header('Access-Control-Allow-Methods', 'GET, POST, OPTIONS')
        self.send_header('Access-Control-Allow-Headers', '*')

        # Enable SharedArrayBuffer for WASM threads (if needed)
        self.send_header('Cross-Origin-Opener-Policy', 'same-origin')
        self.send_header('Cross-Origin-Embedder-Policy', 'require-corp')

        super().end_headers()

    def guess_type(self, path):
        """Override to add WASM MIME type."""
        mimetype = super().guess_type(path)

        if path.endswith('.wasm'):
            return 'application/wasm'

        return mimetype

    def log_message(self, format, *args):
        """Customize log output."""
        sys.stdout.write("%s - - [%s] %s\n" %
                        (self.address_string(),
                         self.log_date_time_string(),
                         format % args))


def main():
    parser = argparse.ArgumentParser(
        description='HTTP server for WASM SPICE client testing',
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  %(prog)s                          # Start on default port 8000
  %(prog)s -p 3000                  # Start on port 3000
  %(prog)s -p 8000 --dir web-test   # Serve from specific directory
        """
    )

    parser.add_argument(
        '-p', '--port',
        type=int,
        default=8000,
        help='Port to serve on (default: 8000)'
    )

    parser.add_argument(
        '--host',
        default='localhost',
        help='Host to bind to (default: localhost)'
    )

    parser.add_argument(
        '--dir',
        type=Path,
        help='Directory to serve (default: current directory)'
    )

    args = parser.parse_args()

    # Change to specified directory if provided
    if args.dir:
        if not args.dir.exists():
            print(f"Error: Directory '{args.dir}' does not exist", file=sys.stderr)
            return 1
        os.chdir(args.dir)
    else:
        # Default: serve from parent directory (spice-client root)
        # This allows access to both web-test/ and pkg/
        script_dir = Path(__file__).parent
        parent_dir = script_dir.parent
        if parent_dir.exists():
            os.chdir(parent_dir)

    print(f"Starting HTTP server at http://{args.host}:{args.port}")
    print(f"Serving files from: {os.getcwd()}")
    print(f"\nOpen in your browser:")
    print(f"  http://{args.host}:{args.port}/web-test/")
    print(f"\nPress Ctrl+C to stop the server\n")

    try:
        with socketserver.TCPServer((args.host, args.port), WAsmHTTPRequestHandler) as httpd:
            httpd.allow_reuse_address = True
            httpd.serve_forever()
    except KeyboardInterrupt:
        print("\n\nShutting down server...")
        return 0
    except OSError as e:
        if e.errno == 48:  # Address already in use
            print(f"\nError: Port {args.port} is already in use", file=sys.stderr)
            print(f"Try a different port with: {sys.argv[0]} -p <port>", file=sys.stderr)
        else:
            print(f"\nError: {e}", file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())