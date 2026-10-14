"""A site that fails on purpose, one way per address. Used by tests/test_failures.py.

    python fixtures/trouble/serve.py [--port 0]

| Address    | What it does |
|------------|--------------|
| /ok        | A small page with one button. |
| /missing   | 404, with a page that has a link back. |
| /broken    | 500, with a page. |
| /moved     | 302 to /ok. |
| /hang      | A page whose image never arrives, so its load event never fires. |
| /endless   | A page that adds content every time it is scrolled to the end, forever. |
| /late      | A long page whose last part arrives 0.8 s after the bottom is reached, as a feed fetches it. |
| /banner    | A page whose button is under a cookie banner. |
| /closed    | A page with a button inside a closed shadow root, which no script can read. |
| /ticking   | A countdown button whose name changes four times a second, beside one that does not. |
| /signin    | A sign-in page with a password field. It signs in by itself: a cookie and a stored value, as a real sign-in leaves. |
| /members   | Shows a members-only page when both are present, and sends the browser to /signin otherwise. |
| /virtual   | A list of 300 rows that keeps only the rows near the screen, as long tables and feeds do. |
| /text      | Plain text, not HTML. |
"""

from __future__ import annotations

import argparse
import contextlib
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

OK = b"""<!doctype html><title>Fine</title><main><button>Continue</button></main>"""
MISSING = b"""<!doctype html><title>Not found</title><h1>Not found</h1><a href="/ok">Home</a>"""
BROKEN = b"""<!doctype html><title>Server error</title><h1>Something broke</h1>"""
HANG = b"""<!doctype html><title>Still loading</title><button>Early</button><img src="/never" alt="">"""
ENDLESS = b"""<!doctype html><title>Endless</title>
<style>.row { height: 400px; border-bottom: 1px solid #ccc; }</style>
<main id="feed"></main>
<script>
  const feed = document.getElementById("feed");
  function more() {
    for (let i = 0; i < 3; i++) {
      const row = document.createElement("div");
      row.className = "row";
      const like = document.createElement("button");
      like.textContent = "Like";
      row.append(like);
      feed.append(row);
    }
  }
  more();
  addEventListener("scroll", () => {
    if (innerHeight + scrollY >= document.documentElement.scrollHeight - 50) more();
  });
</script>"""

LATE = b"""<!doctype html><title>Late</title>
<style>.row { height: 600px; border-bottom: 1px solid #ccc; }</style>
<main><div class="row"><button>First</button></div><div class="row"></div><div class="row"></div>
<div id="end"></div></main>
<script>
  // Like a feed: reaching the end starts a fetch, and the rows arrive a moment later.
  const observer = new IntersectionObserver((entries) => {
    if (!entries.some((entry) => entry.isIntersecting)) return;
    observer.disconnect();
    setTimeout(() => {
      const row = document.createElement("div");
      row.className = "row";
      const late = document.createElement("button");
      late.textContent = "Late arrival";
      row.append(late);
      document.getElementById("end").before(row);
    }, 800);
  });
  observer.observe(document.getElementById("end"));
</script>"""

BANNER = b"""<!doctype html><title>Banner</title>
<main style="position: relative"><button>Buy now</button>
<div id="cookie-banner" style="position: absolute; inset: 0; z-index: 2; background: #fff">We use cookies
<button>Accept cookies</button></div></main>"""
CLOSED = b"""<!doctype html><title>Closed</title>
<button>In the page</button>
<div><template shadowrootmode="closed"><button>Hidden from scripts</button></template></div>"""

VIRTUAL = b"""<!doctype html><title>Virtual list</title>
<style>body { margin: 0; } #list { position: relative; } .row { position: absolute; left: 0; height: 40px; }</style>
<div id="list"></div>
<script>
  // 300 rows of 40 px, but only the ones within a screen of the viewport exist in the page.
  const list = document.getElementById("list"), ROWS = 300, HEIGHT = 40;
  list.style.height = ROWS * HEIGHT + "px";
  function render() {
    const first = Math.max(0, Math.floor((scrollY - innerHeight) / HEIGHT));
    const last = Math.min(ROWS, Math.ceil((scrollY + 2 * innerHeight) / HEIGHT));
    list.replaceChildren();
    for (let i = first; i < last; i++) {
      const row = document.createElement("div");
      row.className = "row";
      row.style.top = i * HEIGHT + "px";
      const open = document.createElement("button");
      open.textContent = "Open row " + (i + 1);
      row.append(open);
      list.append(row);
    }
  }
  render();
  addEventListener("scroll", render);
</script>"""

TICKING = b"""<!doctype html><title>Ticking</title>
<main><button id="countdown">Sale ends in 600 s</button> <button>Buy</button></main>
<script>
  // Starts from the time of day, so two runs never see the same names.
  let left = 100000 - (Date.now() % 100000);
  const countdown = document.getElementById("countdown");
  setInterval(() => { left -= 250; countdown.textContent = "Sale ends in " + Math.round(left / 10) / 100 + " s"; }, 250);
</script>"""

SIGNIN = b"""<!doctype html><title>Sign in</title>
<main><form aria-label="Sign in"><label>Email <input type="email"></label>
<label>Password <input type="password"></label><button>Sign in</button></form></main>
<script>
  // Stands in for a person signing in: what a real sign-in leaves behind.
  localStorage.setItem("token", "secret-token-123");
  document.cookie = "sid=secret-cookie-456; path=/";
</script>"""
MEMBERS = b"""<!doctype html><title>Members</title>
<script>
  if (localStorage.getItem("token") !== "secret-token-123" || !document.cookie.includes("sid=secret-cookie-456")) {
    location.replace("/signin");
  }
</script>
<main><h1>Members area</h1><button>Members only</button></main>"""

# Requests to /never are held until the server stops, so a page that needs one never finishes loading.
STOPPING = threading.Event()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass

    def send(self, status: int, body: bytes, content_type: str = "text/html; charset=utf-8") -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        path = self.path.split("?")[0]
        if path == "/ok":
            self.send(200, OK)
        elif path == "/missing":
            self.send(404, MISSING)
        elif path == "/broken":
            self.send(500, BROKEN)
        elif path == "/moved":
            self.send_response(302)
            self.send_header("Location", "/ok")
            self.send_header("Content-Length", "0")
            self.end_headers()
        elif path == "/hang":
            self.send(200, HANG)
        elif path == "/never":
            STOPPING.wait()
        elif path == "/banner":
            self.send(200, BANNER)
        elif path == "/closed":
            self.send(200, CLOSED)
        elif path == "/ticking":
            self.send(200, TICKING)
        elif path == "/signin":
            self.send(200, SIGNIN)
        elif path == "/members":
            self.send(200, MEMBERS)
        elif path == "/virtual":
            self.send(200, VIRTUAL)
        elif path == "/late":
            self.send(200, LATE)
        elif path == "/endless":
            self.send(200, ENDLESS)
        elif path == "/text":
            self.send(200, b"Just some text, no HTML here.", "text/plain; charset=utf-8")
        else:
            self.send(404, MISSING)


def start(port: int = 0) -> ThreadingHTTPServer:
    """Start the server on a background thread and return it. Port 0 picks a free port."""
    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def stop(server: ThreadingHTTPServer) -> None:
    STOPPING.set()
    server.shutdown()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--port", type=int, default=0)
    server = start(parser.parse_args().port)
    print(f"Trouble site: http://127.0.0.1:{server.server_address[1]}/", flush=True)
    with contextlib.suppress(KeyboardInterrupt):
        while True:
            time.sleep(1)


if __name__ == "__main__":
    main()
