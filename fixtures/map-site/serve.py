"""Serve the map fixture: a small site to crawl with `traceqa map`.

    python fixtures/map-site/serve.py [--port 4177] [--robots none|disallow|error|empty]

Every page has the same header and footer (the shared chrome). The link graph, from the home page:

    /                  depth 0   links to /about, /products/1..3, /terms, /private/page, /files/guide.pdf,
                                 /logout, /admin/delete-account, mailto:, tel:, an external site, and
                                 an iframe (/frame) that holds the only link to /framed
    /about             depth 1   -> /team            /team          depth 2  -> /team/history
    /team/history      depth 3
    /products/N        depth 1   -> next product, and a button "Add to cart" on each product
    /logout, /admin/delete-account, /files/guide.pdf, /private/page exist as traps: following them
    with a real session would sign it out or delete something, so a test fails if they are asked for.

The server records every request as (method, path, time) in `server.requests`. robots.txt:
none answers 404, disallow forbids /private/, error answers 500, empty answers an empty file.
With --port 0 any free port is used; the first line printed names the address either way.
Used by tests/test_map.py.
"""

from __future__ import annotations

import argparse
import contextlib
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler
from pathlib import Path

HERE = Path(__file__).resolve().parent
# This file runs as a script and is also loaded by path, so the shared module is found by path too.
sys.path.insert(0, str(HERE.parent))

from _server import FixtureServer, serve_in_thread  # noqa: E402

ROBOTS = {
    "none": None,
    "disallow": (200, "User-agent: *\nDisallow: /private/\n"),
    "error": (500, "oops"),
    "empty": (200, ""),
}

HEADER = """<header><nav aria-label="Main">
<a href="/">Home</a> <a href="/about">About us</a> <a href="/products/1">Products</a> <a href="/logout">Log out</a>
</nav></header>"""
FOOTER = """<footer><a href="/terms">Terms of use</a> <button type="button">Back to top</button></footer>"""

HOME = """<h1>Map fixture</h1>
<p><a href="/about?utm_source=news&utm_medium=mail">Read about us</a></p>
<ul>
<li><a href="/products/1#reviews">Product one</a></li>
<li><a href="/products/2">Product two</a></li>
<li><a href="/products/3">Product three</a></li>
</ul>
<p><a href="/private/page">Members area</a> <a href="/files/guide.pdf">User guide (PDF)</a>
<a href="/admin/delete-account">Delete my account</a>
<a href="mailto:hello@example.test">Write to us</a> <a href="tel:+15550100">Call us</a>
<a href="https://example.org/elsewhere">Another site</a></p>
<iframe src="/frame" title="Widget" width="400" height="120"></iframe>"""

PAGES = {
    "/": ("Home", HOME),
    "/about": ("About", '<h1>About us</h1><p><a href="/team">Meet the team</a></p>'),
    "/team": ("Team", '<h1>Team</h1><p><a href="/team/history">Our history</a></p>'),
    "/team/history": ("History", "<h1>History</h1><p>Founded long ago.</p>"),
    "/terms": ("Terms", "<h1>Terms</h1><p>Be nice.</p>"),
    "/private/page": ("Private", "<h1>Members only</h1><button type='button'>Sign the book</button>"),
    "/framed": ("Framed", "<h1>Framed</h1><p>Only reachable from the widget.</p>"),
    "/logout": ("Logout", "<h1>Signed out</h1>"),
    "/admin/delete-account": ("Delete", "<h1>Deleted</h1>"),
}
for number in (1, 2, 3):
    PAGES[f"/products/{number}"] = (
        f"Product {number}",
        f'<h1>Product {number}</h1><button type="button">Add to cart</button>'
        f'<p><a href="/products/{number % 3 + 1}">Next product</a></p>',
    )

FRAME = '<!doctype html><title>Widget</title><body><h2>Widget</h2><a href="/framed">Open the framed page</a></body>'


def render(title: str, body: str) -> bytes:
    page = (
        f"<!doctype html><html lang=en><head><meta charset=utf-8><title>{title}</title></head>"
        f"<body>{HEADER}<main>{body}</main>{FOOTER}</body></html>"
    )
    return page.encode("utf-8")


class Handler(BaseHTTPRequestHandler):
    server: Recording

    def log_message(self, format: str, *args: object) -> None:
        pass

    def _record(self) -> str:
        with self.server.lock:
            self.server.requests.append((self.command, self.path, time.monotonic()))
        return self.path.split("?", 1)[0]

    def _send(self, status: int, body: bytes, content_type: str = "text/html; charset=utf-8") -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        path = self._record()
        if path == "/robots.txt":
            answer = ROBOTS[self.server.robots]
            if answer is None:
                self._send(404, b"no robots")
            else:
                self._send(answer[0], answer[1].encode("utf-8"), "text/plain; charset=utf-8")
        elif path == "/frame":
            self._send(200, FRAME.encode("utf-8"))
        elif path == "/files/guide.pdf":
            self._send(200, b"%PDF-1.4\n", "application/pdf")
        elif path in PAGES:
            title, body = PAGES[path]
            self._send(200, render(title, body))
        else:
            self._send(404, b"not found", "text/plain")

    def do_POST(self) -> None:
        self._record()
        self._send(405, b"no", "text/plain")

    do_HEAD = do_PUT = do_DELETE = do_POST


class Recording(FixtureServer):
    def __init__(self, address: tuple[str, int], robots: str) -> None:
        super().__init__(address, Handler)
        self.robots = robots
        self.requests: list[tuple[str, str, float]] = []
        self.lock = threading.Lock()

    def paths(self) -> list[str]:
        """Every path asked for, in order, without queries."""
        with self.lock:
            return [path.split("?", 1)[0] for _, path, _ in self.requests]


def start(port: int = 0, robots: str = "none") -> Recording:
    """Start the server on a background thread. Port 0 picks a free one."""
    server = Recording(("127.0.0.1", port), robots)
    serve_in_thread(server)
    return server


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--port", type=int, default=4177)
    parser.add_argument("--robots", choices=sorted(ROBOTS), default="none")
    args = parser.parse_args()
    server = start(args.port, args.robots)
    print(f"Map fixture: http://127.0.0.1:{server.server_address[1]}/", flush=True)
    with contextlib.suppress(KeyboardInterrupt):
        while True:
            time.sleep(1)


if __name__ == "__main__":
    main()
