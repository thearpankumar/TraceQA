"""Serve one long page, built on request, for measuring how long the explorer takes to read a big page.

    python testbed/sites/long-page/serve.py [--port 4178]

With --port 0 any free port is used; the first line printed names the address either way.

| Address            | What it does |
|--------------------|--------------|
| /?n=2000           | A page with about `n` controls (default 2000, 10 at least, 20000 at most). |
| /?n=2000&cards=40  | The same with `cards` product cards in a list of repeated items (default 40). |

The page is the same for the same `n` and `cards`, byte for byte. It holds, in this order:

* a disclosure button ("Show more") that opens a region of 5 more buttons, and closes it again;
* a button ("Another card") that adds one more card to the list, which can turn the list into a template;
* the list of cards (each has a name, a "Details" link that goes nowhere and an unnamed icon button);
* a row of 6 unnamed icon buttons;
* `n` named controls in sections of 20 (a heading, then buttons, links and fields).

Nothing on the page leaves the page, and nothing is sent anywhere. Every request is recorded in
`server.requests` as (method, path).
"""

from __future__ import annotations

import argparse
import contextlib
import sys
import time
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

from _server import FixtureServer, serve_in_thread  # noqa: E402

DEFAULT_N = 2000
MIN_N = 10
MAX_N = 20000
DEFAULT_CARDS = 40
MAX_CARDS = 500
PER_SECTION = 20


def page(n: int = DEFAULT_N, cards: int = DEFAULT_CARDS) -> str:
    """The HTML of the page with about `n` controls and `cards` cards. The same arguments give the same bytes."""
    n = max(MIN_N, min(MAX_N, n))
    cards = max(0, min(MAX_CARDS, cards))
    out = [
        "<!doctype html><html lang='en'><head><meta charset='utf-8'><title>Long page</title>",
        "<style>.hidden{display:none}</style></head><body>",
        "<header><h1>Long page</h1>",
        "<button id='more' aria-expanded='false'>Show more</button>",
        "<button id='add'>Another card</button></header>",
        "<div id='extra' class='hidden'>",
    ]
    out += [f"<button>Extra {i}</button>" for i in range(5)]
    out.append("</div><main><h2>Products</h2><ul id='cards'>")
    for i in range(cards):
        out.append(card(i))
    out.append("</ul><div id='icons'>")
    out += ["<button aria-label=''><svg width='8' height='8'></svg></button>" for _ in range(6)]
    out.append("</div>")
    sections = (n + PER_SECTION - 1) // PER_SECTION
    made = 0
    for s in range(sections):
        out.append(f"<section><h2>Section {s}</h2>")
        for k in range(min(PER_SECTION, n - made)):
            name = f"Item {s}-{k}"
            if k % 3 == 0:
                out.append(f"<button>{name}</button>")
            elif k % 3 == 1:
                out.append(f"<a href='#{s}-{k}'>{name}</a>")
            else:
                out.append(f"<label>{name}<input type='text' name='f{s}_{k}'></label>")
            made += 1
        out.append("</section>")
    out.append("</main><script>")
    out.append(SCRIPT)
    out.append("</script></body></html>")
    return "".join(out)


def card(i: int) -> str:
    return (
        f"<li><h3>Product {i}</h3><a href='#p{i}'>Details of product {i}</a>"
        "<button aria-label=''><svg width='8' height='8'></svg></button></li>"
    )


SCRIPT = (
    "var more=document.getElementById('more'),extra=document.getElementById('extra');"
    "more.addEventListener('click',function(){var open=extra.classList.toggle('hidden')===false;"
    "more.setAttribute('aria-expanded',String(open));more.textContent=open?'Show less':'Show more';});"
    "var list=document.getElementById('cards');"
    "document.getElementById('add').addEventListener('click',function(){var n=list.children.length,li=document.createElement('li');"
    "li.innerHTML='<h3>Product '+n+'</h3><a href=\"#p'+n+'\">Details of product '+n+'</a>"
    '<button aria-label=""><svg width="8" height="8"></svg></button>\';list.appendChild(li);});'
)


class Server(FixtureServer):
    def __init__(self, *args: object, **kwargs: object) -> None:
        super().__init__(*args, **kwargs)  # type: ignore[arg-type]
        self.requests: list[tuple[str, str]] = []


class Handler(BaseHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass

    def _send(self, status: int, body: bytes, content_type: str = "text/html; charset=utf-8") -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def _handle(self) -> None:
        parts = urlsplit(self.path)
        requests: list[tuple[str, str]] = self.server.requests  # type: ignore[attr-defined]
        requests.append((self.command, parts.path))
        if parts.path == "/":
            query = parse_qs(parts.query)

            def number(key: str, default: int) -> int:
                try:
                    return int(query.get(key, [str(default)])[0])
                except ValueError:
                    return default

            self._send(200, page(number("n", DEFAULT_N), number("cards", DEFAULT_CARDS)).encode())
        elif parts.path == "/favicon.ico":
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
    parser.add_argument("--port", type=int, default=4178)
    server = start(parser.parse_args().port)
    print(f"Long page: http://127.0.0.1:{server.server_address[1]}/", flush=True)
    with contextlib.suppress(KeyboardInterrupt):
        while True:
            time.sleep(1)


if __name__ == "__main__":
    main()
