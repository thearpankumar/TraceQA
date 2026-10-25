"""A small site for tests/test_heal.py: one page in five versions.

    python fixtures/heal/serve.py [--port 0]

| Address | What it is |
|---------|------------|
| /a      | The baseline: a shop page with a test id on one button, and a Help button in main. |
| /b      | The page after a release: "Save for later" became "Save for later!" (a rename), and a second Help button in the footer made the locator of the first one need its landmark (the same id, another locator). |
| /c      | A page without the "Return policy" link (a removal). |
| /d      | "Save for later" became two similar buttons (an ambiguous rename). |
| /e      | Sends the browser to /signin, a sign-in page with a password field, as a page does when a stored session is not accepted. |
| /signin | The sign-in page. |
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
PAGES = {"/a": "a.html", "/b": "b.html", "/c": "c.html", "/d": "d.html", "/signin": "e.html"}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass

    def do_GET(self) -> None:
        path = self.path.split("?")[0]
        if path == "/e":
            self.send_response(302)
            self.send_header("Location", "/signin")
            self.send_header("Content-Length", "0")
            self.end_headers()
        elif path in PAGES:
            body = (HERE / PAGES[path]).read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            self.send_error(404)


def start(port: int = 0) -> FixtureServer:
    """Start the server on a background thread and return it. Port 0 picks a free port."""
    server = FixtureServer(("127.0.0.1", port), Handler)
    serve_in_thread(server)
    return server


def stop(server: FixtureServer) -> None:
    server.shutdown()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--port", type=int, default=0)
    server = start(parser.parse_args().port)
    print(f"Heal fixture: http://127.0.0.1:{server.server_address[1]}/a", flush=True)
    with contextlib.suppress(KeyboardInterrupt):
        while True:
            time.sleep(1)


if __name__ == "__main__":
    main()
