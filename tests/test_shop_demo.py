"""The demo shop under /shop/ of the Buggy Shop (testbed/sites/buggy-shop): the site the explorer is walked on.

What is pinned, without a browser: the original home page is byte for byte what it was (the extraction bench and the
id tests read it); every page of the demo exists and every link inside it leads to a page that exists; the offer is
made once per server and not again; the demo's actions answer for any method and nothing else does; every request is
recorded with its method and path. The explorer's own tests walk it in real browsers (tests/test_explore.py).
"""

from __future__ import annotations

import importlib.util
import re
import sys
import urllib.error
import urllib.request
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import pytest

# Real-browser walks: minutes each. CI runs them in their own job (pytest -m slow); the rest runs with -m "not slow".
pytestmark = pytest.mark.slow

ROOT = Path(__file__).resolve().parents[1]
SHOP = ROOT / "testbed" / "sites" / "buggy-shop"


def load_shop() -> Any:
    spec = importlib.util.spec_from_file_location("buggy_shop_demo_serve", SHOP / "serve.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules["buggy_shop_demo_serve"] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def demo() -> Iterator[Callable[..., Any]]:
    """`demo()` starts a fresh shop (the offer has not been made yet); `.base` is its address and `.requests` its log."""
    started: list[Any] = []

    def start() -> Any:
        port, servers = load_shop().start_on_free_ports()
        servers[0].base = f"http://127.0.0.1:{port}"
        started.extend(servers)
        return servers[0]

    yield start
    for server in started:
        server.shutdown()
        server.server_close()


def fetch(server: Any, path: str, method: str = "GET") -> tuple[int, bytes]:
    data = b"x" if method in {"POST", "PUT"} else None
    request = urllib.request.Request(server.base + path, method=method, data=data)
    try:
        with urllib.request.urlopen(request, timeout=10) as answer:
            return answer.status, answer.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


PAGES = [
    "/shop/",
    "/shop/products.html",
    "/shop/product-1.html",
    "/shop/product-2.html",
    "/shop/product-3.html",
    "/shop/cart.html",
    "/shop/search.html",
    "/shop/account.html",
    "/shop/shipping.html",
    "/shop/returns.html",
    "/shop/offer.html",
    "/shop/specs.html",
    "/shop/app",
    "/shop/app/docs",
    "/shop/app/docs/install",
    "/shop/app/charts/bar",
    "/shop/app/report",
    "/shop/about.html",
    "/shop/team.html",
    "/shop/terms.html",
    "/shop/archive.html",
    "/shop/covered-target.html",
    "/shop/hang-start.html",
    *(f"/shop/chain/{n}.html" for n in range(1, 13)),
]
# Addresses that are linked and are not there, or that leave the shop, on purpose.
LINKS_NOT_FOLLOWED = {"/shop/pay", "/shop/gone", "/shop/checkout"}
NOT_THE_SHOP = ("http://other.invalid/", "mailto:hi@example.test")


def test_the_original_home_page_is_untouched(demo: Callable[..., Any]) -> None:
    server = demo()
    status, body = fetch(server, "/")
    assert status == 200
    assert body == (SHOP / "site" / "index.html").read_bytes()
    assert b"/shop/" not in body, "nothing of the original page points into the demo"


@pytest.mark.parametrize("path", PAGES)
def test_every_page_of_the_demo_exists_and_every_link_in_it_leads_to_a_page_that_exists(
    demo: Callable[..., Any], path: str
) -> None:
    server = demo()
    status, body = fetch(server, path)
    assert status == 200 and b"<title>" in body
    for href in re.findall(rb'href="([^"]+)"', body):
        target = href.decode()
        if target in LINKS_NOT_FOLLOWED or target in NOT_THE_SHOP or target.startswith("/shop/hang/"):
            # On purpose: a page that answers for any method, a page that is gone, another site, a mail link, and pages
            # that never answer.
            continue
        assert target.startswith("/shop/"), f"{path} links outside the demo: {target}"
        assert fetch(server, target)[0] == 200, f"{path} links to {target}, which is not there"


def test_the_demo_pages_have_what_the_explorer_has_to_deal_with(demo: Callable[..., Any]) -> None:
    server = demo()
    home = fetch(server, "/shop/")[1]
    assert b'role="tablist"' in home and b'role="dialog"' in home and b'id="offer-btn"' in home
    # What the explorer's first test site had, now in the one shop: a disclosure, a link an overlay covers, a link to another
    # site and a mail link, a link to a page that is gone, a form button and a text field, and twelve disabled buttons.
    assert b'id="more-btn"' in home and b'class="overlay"' in home and b"Partner site" in home
    assert b"mailto:" in home and b"Old page" in home and b"Continue" in home and b"Your note" in home
    assert home.count(b"disabled>Size ") == 12 and b'id="promo"' in home
    assert fetch(server, "/shop/gone")[0] == 404
    assert b"Hanging page 6" in fetch(server, "/shop/hang-start.html")[1]
    assert (
        b'type="tel"' in fetch(server, "/shop/search.html")[1] and b"Company" in fetch(server, "/shop/search.html")[1]
    )
    assert b"Clear cart" in fetch(server, "/shop/cart.html")[1]
    assert b"Pay now" in fetch(server, "/shop/cart.html")[1]
    assert b"Delete account" in fetch(server, "/shop/account.html")[1]
    assert b'type="email"' in fetch(server, "/shop/search.html")[1]
    assert b"Add to cart" in fetch(server, "/shop/product-1.html")[1]


def test_every_address_of_the_demo_app_is_the_apps_page_and_the_report_is_the_slow_route(
    demo: Callable[..., Any],
) -> None:
    server = demo()
    shell = fetch(server, "/shop/app")[1]
    for path in ("/shop/app/docs", "/shop/app/charts/bar", "/shop/app/report", "/shop/app/nothing-here"):
        assert fetch(server, path) == (200, shell), path
    assert fetch(server, "/shop/app", "HEAD")[0] == 200
    assert b"data-link" in shell and b'"/report": 2500' in shell, "the report is the route that arrives late"
    assert b"history.pushState" in shell, "a route change is not a page load"
    assert b"Open the app" in fetch(server, "/shop/")[1], "the shop links into the app"
    assert b'aria-label="Main"' in shell, "the app page has the shop's header"
    assert fetch(server, "/shop/app", "POST")[0] == 501, "a write there is not supported, as anywhere else"


def test_the_offer_is_made_once_per_server_and_not_again(demo: Callable[..., Any]) -> None:
    first = demo()
    assert b"Ten per cent" in fetch(first, "/shop/api/offer")[1]
    assert b'"offer": null' in fetch(first, "/shop/api/offer")[1]
    assert b'"offer": null' in fetch(first, "/shop/api/offer")[1]
    other = demo()
    assert b"Ten per cent" in fetch(other, "/shop/api/offer")[1], "a new server makes it again"


def test_the_demo_actions_answer_for_any_method_and_nothing_else_does(demo: Callable[..., Any]) -> None:
    server = demo()
    assert fetch(server, "/shop/api/cart", "POST")[0] == 200
    assert fetch(server, "/shop/api/clear", "POST")[0] == 200
    assert fetch(server, "/shop/api/account", "DELETE")[0] == 200
    assert fetch(server, "/shop/api/typed/Search%20products/laptop", "POST")[0] == 200
    assert fetch(server, "/shop/pay", "GET")[0] == 200
    # As before: a write anywhere else is not supported.
    assert fetch(server, "/", "POST")[0] == 501
    assert fetch(server, "/shop/cart.html", "DELETE")[0] == 501


def test_every_request_is_recorded_with_its_method_and_path_without_the_query(demo: Callable[..., Any]) -> None:
    server = demo()
    fetch(server, "/shop/cart.html?x=1")
    fetch(server, "/shop/api/cart", "POST")
    fetch(server, "/shop/", "HEAD")
    assert server.requests == [("GET", "/shop/cart.html"), ("POST", "/shop/api/cart"), ("HEAD", "/shop/")]


def test_the_promo_text_is_the_same_each_time_unless_the_server_rotates_it(demo: Callable[..., Any]) -> None:
    server = demo()
    assert fetch(server, "/shop/api/promo")[1] == fetch(server, "/shop/api/promo")[1] == b"Promo of the day"
    server.rotate = True
    assert fetch(server, "/shop/api/promo")[1] == b"Promo number 3"
    assert fetch(server, "/shop/api/promo")[1] == b"Promo number 4", "a new text on every load"


def test_a_hanging_page_answers_late_and_a_checkout_answers_a_post_and_nothing_else_does(
    demo: Callable[..., Any],
) -> None:
    server = demo()
    server.hang_seconds = 0.2
    assert fetch(server, "/shop/hang/3") == (200, b"<!doctype html><title>Hung</title><p>Late.</p>")
    assert fetch(server, "/shop/checkout", "POST")[0] == 200
    assert fetch(server, "/shop/cart.html", "POST")[0] == 501
