"""Serve the page.act fixture: one control for every kind of thing an action can cause.

    python testbed/sites/act/serve.py [--port 4177]

With --port 0 any free port is used; the first line printed names the address either way. Every request is
recorded in `server.requests` as (method, path), so a test can say what the browser really sent. Used by
web-drivers/playwright-py/tests/test_act.py.

| Address           | What it does |
|-------------------|--------------|
| /                 | The page with the controls (index.html). |
| /next.html        | A second page. |
| /frame.html       | The page inside the iframe. |
| /files/receipt.txt| A download (Content-Disposition: attachment). |
| /api/fail         | 500. |
| /api/ok           | 200. |
| /api/drop         | Closes the connection without answering. |
| /checkout         | 200 for any method: where the page's forms post to. |
| /api/post, /api/beacon | 200 for any method. |
| /redirect-out?to=X| 302 to X (another origin, in a test). |
| /sw.js            | A service worker script. |
| /ws               | 404: nothing speaks WebSocket here, so any request to it is a connection attempt that got through. |
"""

from __future__ import annotations

import argparse
import contextlib
import socket
import sys
import time
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

HERE = Path(__file__).resolve().parent
# This file runs as a script and is also loaded by path, so the shared module is found by path too.
sys.path.insert(0, str(HERE.parent))

from _server import FixtureServer, serve_in_thread  # noqa: E402

PAGES = {"/": "index.html", "/index.html": "index.html", "/next.html": "next.html", "/frame.html": "frame.html"}


class Server(FixtureServer):
    def __init__(self, *args: object, **kwargs: object) -> None:
        super().__init__(*args, **kwargs)  # type: ignore[arg-type]
        self.requests: list[tuple[str, str]] = []


class Handler(BaseHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass

    def _send(self, status: int, body: bytes, content_type: str = "text/html; charset=utf-8", **headers: str) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        for name, value in headers.items():
            self.send_header(name.replace("_", "-"), value)
        self.end_headers()
        self.wfile.write(body)

    def _handle(self) -> None:
        path = urlsplit(self.path).path
        requests: list[tuple[str, str]] = self.server.requests  # type: ignore[attr-defined]
        requests.append((self.command, path))
        if path in PAGES:
            self._send(200, (HERE / PAGES[path]).read_bytes())
        elif path == "/files/receipt.txt":
            self._send(200, b"receipt", "text/plain", Content_Disposition='attachment; filename="receipt.txt"')
        elif path == "/api/fail":
            self._send(500, b"nope", "text/plain")
        elif path == "/api/drop":
            self.close_connection = True
            with contextlib.suppress(OSError):
                self.connection.shutdown(socket.SHUT_RDWR)
        elif path in ("/checkout", "/api/post", "/api/beacon"):
            self._send(200, b"done", "text/plain")
        elif path == "/redirect-out":
            target = parse_qs(urlsplit(self.path).query).get("to", [""])[0]
            self._send(302, b"", "text/plain", Location=target)
        elif path == "/sw.js":
            self._send(200, b"self.addEventListener('fetch', () => {});", "text/javascript")
        elif path == "/api/ok":
            self._send(200, b"ok", "text/plain")
        elif path == "/favicon.ico":
            self._send(204, b"", "image/x-icon")
        else:
            self._send(404, b"not found", "text/plain")

    do_GET = do_HEAD = do_POST = do_PUT = do_DELETE = _handle


def start(port: int = 0) -> Server:
    """Start the server on a background thread. Port 0 picks a free one."""
    server = Server(("127.0.0.1", port), Handler)
    serve_in_thread(server)
    return server


def main() -> None:
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    parser.add_argument("--port", type=int, default=4177)
    server = start(parser.parse_args().port)
    print(f"Act fixture: http://127.0.0.1:{server.server_address[1]}/", flush=True)
    with contextlib.suppress(KeyboardInterrupt):
        while True:  # a plain wait() cannot be interrupted with Ctrl-C on Windows
            time.sleep(1)


if __name__ == "__main__":
    main()
