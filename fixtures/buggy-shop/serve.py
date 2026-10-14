"""Serve the Buggy Shop.

    python fixtures/buggy-shop/serve.py [--port 4173]

The shop is on PORT. The "third party" payment frame is on PORT + 1, which
makes it a different origin. With --port 0 any free pair of ports is used; the
first line printed names the shop's address either way.
"""

from __future__ import annotations

import argparse
import contextlib
import functools
import socket
import threading
import time
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

HERE = Path(__file__).resolve().parent


class QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass


def start(port: int) -> list[ThreadingHTTPServer]:
    """Start both servers on background threads and return them."""
    servers: list[ThreadingHTTPServer] = []
    try:
        for offset, folder in enumerate(["site", "third-party"]):
            handler = functools.partial(QuietHandler, directory=str(HERE / folder))
            servers.append(ThreadingHTTPServer(("127.0.0.1", port + offset), handler))
    except OSError:
        for server in servers:
            server.server_close()
        raise
    for server in servers:
        threading.Thread(target=server.serve_forever, daemon=True).start()
    return servers


def start_on_free_ports() -> tuple[int, list[ThreadingHTTPServer]]:
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
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
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
