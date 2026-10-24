"""The server class both fixture sites share. No side effects when imported.

The standard server listens with a backlog of 5. Twelve browser targets that load a page at
once (3 browsers x 4 viewports, each fetching a dozen files) can overflow that on Windows, and the
dropped connections show up as scripts that "failed to load", so the page is read incompletely.
A backlog of 128 holds them all.
"""

from __future__ import annotations

import threading
from http.server import ThreadingHTTPServer


class FixtureServer(ThreadingHTTPServer):
    request_queue_size = 128
    daemon_threads = True


def serve_in_thread(server: ThreadingHTTPServer) -> threading.Thread:
    """Run the server on a background thread that ends with the process."""
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return thread
