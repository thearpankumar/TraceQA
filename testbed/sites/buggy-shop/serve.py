"""Serve the Buggy Shop.

    python testbed/sites/buggy-shop/serve.py [--port 4173]

The shop is on PORT. The "third party" payment frame is on PORT + 1, which
makes it a different origin. With --port 0 any free pair of ports is used; the
first line printed names the shop's address either way.

The home page (/) is the page the extraction bench and the id tests read: leave it alone. The explorer is walked on
the demo shop under /shop/ (see README.md): several pages, tabs and dialogs that need a replay to come back to, a
cart, controls that a read-only walk must skip, forms, a single-page app (/shop/app) whose routes change the address
without a page load, one of them slowly, a chain of pages and pages that never answer for the limits of a walk, and a
promo link that can be made to change on every load. Every request is recorded in `server.requests` as
(method, path), and the demo's actions (/shop/api/...) answer for any method, so a test can say what the browser
really sent.
"""

from __future__ import annotations

import argparse
import contextlib
import functools
import json
import socket
import sys
import threading
import time
from http.server import SimpleHTTPRequestHandler
from pathlib import Path
from urllib.parse import urlsplit

HERE = Path(__file__).resolve().parent
# This file runs as a script and is also loaded by path, so the shared module is found by path too.
sys.path.insert(0, str(HERE.parent))

from _server import FixtureServer, serve_in_thread  # noqa: E402

OFFER = "Ten per cent off your first order."
HANG_SECONDS = 25


class QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass

    def _record(self) -> str:
        path = urlsplit(self.path).path
        requests = getattr(self.server, "requests", None)
        if requests is not None:
            requests.append((self.command, path))
        return path

    def _send(self, status: int, body: bytes, content_type: str) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _app_route(self, path: str) -> None:
        """Every address of the demo app is the app's page: its router draws the route in the browser."""
        if path == "/shop/app" or path.startswith("/shop/app/"):
            self.path = "/shop/app.html"

    def do_GET(self) -> None:
        path = self._record()
        self._app_route(path)
        if path == "/shop/api/offer":
            # Made once for as long as the server runs: the first who asks is given it, the next is given nothing.
            # A screen that only this answer opens cannot be reached a second time.
            with self.server.offer_lock:  # type: ignore[attr-defined]
                offer = OFFER if not self.server.offer_made else None  # type: ignore[attr-defined]
                self.server.offer_made = True  # type: ignore[attr-defined]
            self._send(200, json.dumps({"offer": offer}).encode(), "application/json")
        elif path == "/shop/api/promo":
            # The text of the promo link. It changes on every load when the server rotates it, as a banner does.
            with self.server.offer_lock:  # type: ignore[attr-defined]
                self.server.promo += 1  # type: ignore[attr-defined]
                text = f"Promo number {self.server.promo}" if self.server.rotate else "Promo of the day"  # type: ignore[attr-defined]
            self._send(200, text.encode(), "text/plain")
        elif path.startswith("/shop/hang/"):
            # A page that never answers, for the time limit of an action.
            time.sleep(self.server.hang_seconds)  # type: ignore[attr-defined]
            self._send(200, b"<!doctype html><title>Hung</title><p>Late.</p>", "text/html; charset=utf-8")
        elif path.startswith("/shop/api/") or path == "/shop/pay":
            self._send(200, b"done", "text/plain")
        else:
            super().do_GET()

    def do_HEAD(self) -> None:
        self._app_route(self._record())
        super().do_HEAD()

    def _write(self) -> None:
        """What the demo's actions post to. Anything else stays unsupported, as it always was."""
        path = self._record()
        if path.startswith("/shop/api/") or path in {"/shop/pay", "/shop/checkout"}:
            self._send(200, b"done", "text/plain")
        else:
            self.send_error(501, f"Unsupported method ({self.command!r})")

    do_POST = do_PUT = do_DELETE = _write


def start(port: int) -> list[FixtureServer]:
    """Start both servers on background threads and return them."""
    servers: list[FixtureServer] = []
    try:
        for offset, folder in enumerate(["site", "third-party"]):
            handler = functools.partial(QuietHandler, directory=str(HERE / folder))
            servers.append(FixtureServer(("127.0.0.1", port + offset), handler))
    except OSError:
        for server in servers:
            server.server_close()
        raise
    for server in servers:
        server.requests = []  # type: ignore[attr-defined]
        server.offer_made = False  # type: ignore[attr-defined]
        # Rotating the promo link's text is off unless a test asks for it: a walk that must write the same bytes twice
        # needs a site that answers the same way.
        server.rotate = False  # type: ignore[attr-defined]
        server.promo = 0  # type: ignore[attr-defined]
        server.hang_seconds = HANG_SECONDS  # type: ignore[attr-defined]
        server.offer_lock = threading.Lock()  # type: ignore[attr-defined]
        serve_in_thread(server)
    return servers


def start_on_free_ports() -> tuple[int, list[FixtureServer]]:
    """Start the shop on any free pair of neighbouring ports. Used by tests."""
    for _ in range(50):
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]
        try:
            return port, start(port)
        except (OSError, OverflowError):  # the next port is taken, or there is no next port
            continue
    raise RuntimeError("could not find two free neighbouring ports")


def main() -> None:
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    parser.add_argument("--port", type=int, default=4173)
    port = parser.parse_args().port
    if port == 0:
        port, _servers = start_on_free_ports()
    else:
        start(port)
    # Flushed, because tests in other languages start this script and wait for the line.
    print(f"Buggy Shop: http://127.0.0.1:{port}/   (payment frame on port {port + 1})", flush=True)
    with contextlib.suppress(KeyboardInterrupt):
        while True:  # a plain wait() cannot be interrupted with Ctrl-C on Windows
            time.sleep(1)


if __name__ == "__main__":
    main()
