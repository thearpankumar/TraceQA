"""The axe fixture: one page with a failure of each kind, used by tests/test_axe.py.

    python testbed/sites/axe/serve.py [--port 0]

| Address     | What it is |
|-------------|------------|
| /           | A tall page: a low-contrast button, an image without alt text, a duplicate id that an aria reference uses, and an iframe. |
| /frame.html | The iframe's page: a text input without a label. |
| /csp        | The same page, sent with `Content-Security-Policy: script-src 'self'`, which blocks scripts the page runs through eval or inline. |
"""

from __future__ import annotations

import argparse
import contextlib
import sys
import time
from http.server import BaseHTTPRequestHandler
from pathlib import Path

# This file runs as a script and is also loaded by path, so the shared module is found by path too.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from _server import FixtureServer, serve_in_thread

HERE = Path(__file__).resolve().parent
FRAME = b"""<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Widget</title></head>
<body><form><input type="text" id="q"></form></body></html>"""
CSP = "script-src 'self'"


class Handler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        path = self.path.split("?", 1)[0]
        if path == "/" or path == "/csp":
            body = (HERE / "index.html").read_bytes()
            headers = {"Content-Security-Policy": CSP} if path == "/csp" else {}
            self.reply(body, headers)
        elif path == "/frame.html":
            self.reply(FRAME, {})
        else:
            self.send_error(404)

    def reply(self, body: bytes, headers: dict[str, str]) -> None:
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        for name, value in headers.items():
            self.send_header(name, value)
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        pass


def start(port: int = 0) -> FixtureServer:
    """Start the server on a background thread. Port 0 picks a free one."""
    server = FixtureServer(("127.0.0.1", port), Handler)
    serve_in_thread(server)
    return server


def main() -> None:
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    parser.add_argument("--port", type=int, default=4177)
    server = start(parser.parse_args().port)
    print(f"Axe fixture: http://127.0.0.1:{server.server_address[1]}/", flush=True)
    with contextlib.suppress(KeyboardInterrupt):
        while True:  # a plain wait() cannot be interrupted with Ctrl-C on Windows
            time.sleep(1)


if __name__ == "__main__":
    main()
