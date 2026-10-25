"""Serve the stability fixture: a page whose "Flaky" button exists on every other request.

    python testbed/sites/stability/serve.py [--port 4176]

A server-side counter counts the requests for the page. The first request gets the page without
the button, the second with it, and so on. "Steady" is on every response. So `traceqa extract
--repeat 4` finds Flaky in two of four runs (rate 0.5) when each run loads the page once.
With --port 0 any free port is used; the first line printed names the address either way.
Used by tests/test_repeat.py.
"""

from __future__ import annotations

import argparse
import contextlib
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from typing import cast

HERE = Path(__file__).resolve().parent
# This file runs as a script and is also loaded by path, so the shared module is found by path too.
sys.path.insert(0, str(HERE.parent))

from _server import FixtureServer, serve_in_thread  # noqa: E402

MARKER = "<!--FLAKY-->"
FLAKY = '<button type="button">Flaky</button>'


class Handler(BaseHTTPRequestHandler):
    @property
    def fixture(self) -> Counting:
        # socketserver types `server` as a plain BaseServer; this handler is only ever served by Counting.
        return cast(Counting, self.server)

    def log_message(self, format: str, *args: object) -> None:
        pass

    def do_GET(self) -> None:
        if self.path.split("?", 1)[0] not in ("/", "/index.html"):
            self.send_error(404)
            return
        page = (HERE / "index.html").read_text(encoding="utf-8")
        body = page.replace(MARKER, FLAKY if self.fixture.next_request_has_flaky() else "").encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)


class Counting(FixtureServer):
    """Counts page requests; the even-numbered ones (2nd, 4th, ...) carry the Flaky button."""

    def __init__(self, address: tuple[str, int]) -> None:
        super().__init__(address, Handler)
        self.requests = 0
        self.lock = threading.Lock()

    def next_request_has_flaky(self) -> bool:
        with self.lock:
            self.requests += 1
            return self.requests % 2 == 0


def start(port: int = 0) -> Counting:
    """Start the server on a background thread. Port 0 picks a free one."""
    server = Counting(("127.0.0.1", port))
    serve_in_thread(server)
    return server


def main() -> None:
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    parser.add_argument("--port", type=int, default=4176)
    server = start(parser.parse_args().port)
    # Flushed, because tests in other languages start this script and wait for the line.
    print(f"Stability fixture: http://127.0.0.1:{server.server_address[1]}/", flush=True)
    with contextlib.suppress(KeyboardInterrupt):
        while True:  # a plain wait() cannot be interrupted with Ctrl-C on Windows
            time.sleep(1)


if __name__ == "__main__":
    main()
