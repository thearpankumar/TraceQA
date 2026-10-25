"""Serve the findings fixture: one page with a positive and a negative case for every rule.

    python fixtures/findings/serve.py [--port 4175]

With --port 0 any free port is used; the first line printed names the address either way.
Used by tests/test_findings.py.
"""

from __future__ import annotations

import argparse
import contextlib
import functools
import sys
import time
from http.server import SimpleHTTPRequestHandler
from pathlib import Path

HERE = Path(__file__).resolve().parent
# This file runs as a script and is also loaded by path, so the shared module is found by path too.
sys.path.insert(0, str(HERE.parent))

from _server import FixtureServer, serve_in_thread  # noqa: E402


class QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass


def start(port: int = 0) -> FixtureServer:
    """Start the server on a background thread. Port 0 picks a free one."""
    handler = functools.partial(QuietHandler, directory=str(HERE))
    server = FixtureServer(("127.0.0.1", port), handler)
    serve_in_thread(server)
    return server


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--port", type=int, default=4175)
    server = start(parser.parse_args().port)
    # Flushed, because tests in other languages start this script and wait for the line.
    print(f"Findings fixture: http://127.0.0.1:{server.server_address[1]}/", flush=True)
    with contextlib.suppress(KeyboardInterrupt):
        while True:  # a plain wait() cannot be interrupted with Ctrl-C on Windows
            time.sleep(1)


if __name__ == "__main__":
    main()
