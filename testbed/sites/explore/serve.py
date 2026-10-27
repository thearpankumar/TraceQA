"""Serve the explorer fixture: a small shop-like site with one of everything the walker has to treat differently.

    python testbed/sites/explore/serve.py [--port 4178]

With --port 0 any free port is used; the first line printed names the address either way. Every request is recorded
in `server.requests` as (method, path), so a test can say what the browser really sent. Used by tests/test_explore.py.

| Address            | What it is |
|--------------------|------------|
| /                  | Home. Twelve disabled size buttons make it big. A header and footer that every page shares; links to the products, the about page, a page that is gone (404), a page behind an overlay, one to another site, one to mail; a disclosure ("More") that shows a link; a dialog ("Help") with two links; tabs; a promo link whose text changes on every load; and the controls a read-only walk must leave alone: "Delete account", "Pay now", a form whose button says only "Continue", and a text field. |
| /products          | Three product links. |
| /products/1..3     | A product with an "Add to cart" button (posts to /api/cart) and a link back. |
| /about, /team, /terms, /archive, /shipping, /returns, /covered-target | Pages with a few links or none. |
| /form              | Five text fields and nothing else (no links, no submit): Search products, Email (type=email), Phone (type=tel), City, Company. Typing into one reports it to /typed/LABEL/VALUE, so a test can see exactly what was typed. For the model that suggests text (EXP-6). |
| /typed/LABEL/VALUE | 200 for any method: where /form reports what was typed. |
| /chain/1..12       | A bare chain: each page has one "Next" link, and the last one links back to the first. For depth and screen limits. |
| /hang-start        | Six links to pages that never answer. For the time limit of an action. |
| /hang/1..6         | Never answers (waits 25 s). |
| /checkout, /api/cart, /api/account | 200 for any method: where the page's forms and fetches post to. |
| anything else      | 404 with a page that has nothing on it to act on. |
"""

from __future__ import annotations

import argparse
import contextlib
import sys
import time
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from urllib.parse import urlsplit

HERE = Path(__file__).resolve().parent
# This file runs as a script and is also loaded by path, so the shared module is found by path too.
sys.path.insert(0, str(HERE.parent))

from _server import FixtureServer, serve_in_thread  # noqa: E402

HEADER = """<header><nav aria-label="Main">
<a href="/">Home</a> <a href="/products">Products</a> <a href="/about">About</a>
</nav></header>"""
FOOTER = """<footer><a href="/terms">Terms</a></footer>"""


def page(title: str, body: str, *, bare: bool = False, script: str = "") -> bytes:
    chrome_top = "" if bare else HEADER
    chrome_bottom = "" if bare else FOOTER
    return f"""<!doctype html><html lang="en"><head><meta charset="utf-8"><title>{title}</title>
<style>
.cover {{ position: relative; display: inline-block; }}
.cover .overlay {{ position: absolute; inset: 0; background: rgba(0,0,0,.2); }}
[hidden] {{ display: none !important; }}
dialog-box {{ display: block; }}
</style></head><body>{chrome_top}<main><h1>{title}</h1>{body}</main>{chrome_bottom}{script}</body></html>""".encode()


HOME_BODY = """
<p><a href="/products">Browse products</a> <a href="/about">About us</a> <a href="/gone">Old page</a></p>
<p><a id="promo" href="/products">PROMO_TEXT</a></p>
<p><button id="more-btn" type="button" aria-expanded="false" aria-controls="more">More</button></p>
<div id="more" hidden><a href="/archive">Archive</a></div>
<p><button id="help-btn" type="button" aria-expanded="false" aria-controls="help">Help</button></p>
<div id="help" role="dialog" aria-modal="true" aria-label="Help" hidden>
  <a href="/shipping">Shipping</a> <a href="/returns">Returns</a>
  <button id="help-close" type="button">Close help</button>
</div>
<div role="tablist" aria-label="Details">
  <button role="tab" id="tab-1" aria-selected="true" aria-controls="panel-1" type="button">Overview</button>
  <button role="tab" id="tab-2" aria-selected="false" aria-controls="panel-2" type="button">Specifications</button>
</div>
<div role="tabpanel" id="panel-1">Overview text.</div>
<div role="tabpanel" id="panel-2" hidden><a href="/specs-sheet">Spec sheet</a></div>
<p class="cover"><a id="covered" href="/covered-target">Hidden behind an overlay</a><span class="overlay"></span></p>
<p><a href="http://other.invalid/">Partner site</a> <a href="mailto:hi@example.test">Email us</a>
   <a href="/pay">Pay now</a></p>
<p><button id="delete" type="button">Delete account</button></p>
<form method="post" action="/checkout"><button type="submit">Continue</button></form>
<p><label>Your note <input type="text" name="note"></label></p>
<p>SIZES</p>
"""

# Twelve controls that cannot be used. They make the page big enough that its promo link, which changes on every load,
# is a small part of it (a rotating banner on a page of 25 controls is the same screen at 90%; on a page of 12 it is not).
SIZES = " ".join(f'<button type="button" disabled>Size {c}</button>' for c in "ABCDEFGHIJKL")

HOME_SCRIPT = """<script>
function wire(buttonId, targetId) {
  var b = document.getElementById(buttonId), t = document.getElementById(targetId);
  b.addEventListener("click", function () {
    var open = b.getAttribute("aria-expanded") === "true";
    b.setAttribute("aria-expanded", open ? "false" : "true");
    t.hidden = open;
  });
}
wire("more-btn", "more");
wire("help-btn", "help");
document.getElementById("help-close").addEventListener("click", function () {
  document.getElementById("help-btn").click();
});
document.getElementById("delete").addEventListener("click", function () {
  fetch("/api/account", { method: "DELETE" });
});
[1, 2].forEach(function (n) {
  document.getElementById("tab-" + n).addEventListener("click", function () {
    [1, 2].forEach(function (m) {
      document.getElementById("tab-" + m).setAttribute("aria-selected", String(m === n));
      document.getElementById("panel-" + m).hidden = m !== n;
    });
  });
});
</script>"""

FORM_BODY = """
<p><label>Search products <input type="text" name="q"></label></p>
<p><label>Your email <input type="email" name="email"></label></p>
<p><label>Phone number <input type="tel" name="phone"></label></p>
<p><label>City <input type="text" name="city"></label></p>
<p><label>Company <input type="text" name="company"></label></p>
"""

# What was typed is reported as a request, so that a test can read it from the server's log.
FORM_SCRIPT = """<script>
document.querySelectorAll("input").forEach(function (input) {
  input.addEventListener("input", function () {
    var label = input.closest("label").firstChild.textContent.trim();
    fetch("/typed/" + encodeURIComponent(label) + "/" + encodeURIComponent(input.value), { method: "POST", body: "x" });
  });
});
</script>"""

PRODUCT_SCRIPT = """<script>
document.getElementById("add").addEventListener("click", function () {
  fetch("/api/cart", { method: "POST", body: "x" });
});
</script>"""


class Server(FixtureServer):
    def __init__(self, *args: object, **kwargs: object) -> None:
        super().__init__(*args, **kwargs)  # type: ignore[arg-type]
        self.requests: list[tuple[str, str]] = []
        self.promo = 0
        # The promo link's text changes on every load, as a rotating banner does. A test that needs the site to answer
        # the same way every time (two walks with the same bytes) starts it with rotate=False.
        self.rotate = True


class Handler(BaseHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass

    def _send(self, status: int, body: bytes, content_type: str = "text/html; charset=utf-8") -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _handle(self) -> None:
        path = urlsplit(self.path).path
        server: Server = self.server  # type: ignore[assignment]
        server.requests.append((self.command, path))
        if path == "/":
            server.promo += 1
            text = f"Promo number {server.promo}" if server.rotate else "Promo of the day"
            body = HOME_BODY.replace("PROMO_TEXT", text).replace("SIZES", SIZES)
            self._send(200, page("Buggy fixture home", body, script=HOME_SCRIPT))
        elif path == "/products":
            links = "".join(f'<li><a href="/products/{n}">Product {n}</a></li>' for n in (1, 2, 3))
            self._send(200, page("Products", f"<ul>{links}</ul>"))
        elif path.startswith("/products/") and path[10:] in ("1", "2", "3"):
            n = path[10:]
            body = f'<p>Details of product {n}.</p><button id="add" type="button">Add to cart</button>'
            body += '<p><a href="/products">Back to products</a></p>'
            self._send(200, page(f"Product {n}", body, script=PRODUCT_SCRIPT))
        elif path == "/about":
            self._send(200, page("About", '<p><a href="/team">Meet the team</a></p>'))
        elif path in ("/team", "/archive", "/shipping", "/returns", "/covered-target", "/terms", "/specs-sheet"):
            self._send(200, page(path.strip("/").replace("-", " ").title(), "<p>Nothing more to do here.</p>"))
        elif path.startswith("/chain/") and path[7:].isdigit() and 1 <= int(path[7:]) <= 12:
            n = int(path[7:])
            # The last page links back to the first: a page with nothing to act on is not a screen.
            nxt = f'<a href="/chain/{n + 1}">Next {n + 1}</a>' if n < 12 else '<a href="/chain/1">Start again</a>'
            self._send(200, page(f"Chain {n}", f"<p>{nxt}</p>", bare=True))
        elif path == "/hang-start":
            links = "".join(f'<li><a href="/hang/{n}">Hanging page {n}</a></li>' for n in range(1, 7))
            self._send(200, page("Hang start", f"<ul>{links}</ul>", bare=True))
        elif path.startswith("/hang/"):
            time.sleep(25)
            self._send(200, page("Hung", "<p>Late.</p>", bare=True))
        elif path == "/form":
            self._send(200, page("Form", FORM_BODY, bare=True, script=FORM_SCRIPT))
        elif path.startswith("/typed/") or path in ("/checkout", "/api/cart", "/api/account"):
            self._send(200, b"done", "text/plain")
        elif path == "/favicon.ico":
            self._send(204, b"", "image/x-icon")
        else:
            self._send(
                404,
                b"<!doctype html><html><head><title>Not found</title></head><body><p>No such page.</p></body></html>",
            )

    do_GET = do_HEAD = do_POST = do_PUT = do_DELETE = _handle


def start(port: int = 0, *, rotate: bool = True) -> Server:
    """Start the server on a background thread. Port 0 picks a free one."""
    server = Server(("127.0.0.1", port), Handler)
    server.rotate = rotate
    serve_in_thread(server)
    return server


def main() -> None:
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    parser.add_argument("--port", type=int, default=4178)
    server = start(parser.parse_args().port)
    print(f"Explorer fixture: http://127.0.0.1:{server.server_address[1]}/", flush=True)
    with contextlib.suppress(KeyboardInterrupt):
        while True:  # a plain wait() cannot be interrupted with Ctrl-C on Windows
            time.sleep(1)


if __name__ == "__main__":
    main()
