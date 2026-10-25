"""The collector on a large page, and on a page where its selector choices must not change.

The collector used to do work per element that grew with the size of the page (a selector
query per data-testid and id, a scan of all siblings, `labels` on every control), so a page of
20 000 elements took a minute. These tests keep the cost in line with the page size and keep
the selectors it picks the same.
"""

from __future__ import annotations

import functools
import http.server
import json
import sys
import threading
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
from traceqa_protocol import contracts
from traceqa_protocol.testing import DriverClient

ROOT = Path(__file__).resolve().parents[2]
COLLECTOR = (ROOT / "page-script" / "page-script.js").read_text(encoding="utf-8")
BROWSERS = ["chromium", "firefox", "webkit"]
HELLO = {"protocolVersion": contracts.DRIVER_PROTOCOL_VERSION, "engineVersion": "test"}
VIEWPORT = {"width": 1280, "height": 800}
# Loose on purpose: machines are shared and noisy, and the point is to rule out quadratic
# growth, not to time the collector. Before the fix Chromium needed 61 600 ms for this page.
BUDGET_MS = 5000

LARGE_PAGE = (
    "<!doctype html><meta charset=utf-8><title>large</title><body>"
    + "<ul>"
    + "".join(f"<li><button data-testid=t{i}>Item {i}</button></li>" for i in range(5000))
    + "</ul><div>"
    + "".join(f'<div id=i{i}><a id=l{i} href="#l{i}">Link {i}</a></div>' for i in range(5000))
    + "</div><div>"
    + "<div><span>x</span></div>" * 10000
    + "</div></body>"
)

SELECTOR_PAGE = """<!doctype html><meta charset=utf-8><title>selectors</title>
<body><div id="app"><ul>
<li><button data-testid="dup">A</button></li>
<li><button data-testid="dup">B</button></li>
<li><button data-testid="solo">C</button></li>
</ul>
<section id="unique"><span><a href="#x">L1</a></span><span><a href="#y">L2</a></span></section>
<div id="twin"><button>T1</button></div>
<div id="twin"><button>T2</button></div>
<div id="a:b"><button>Odd id</button></div>
<button id="btn">Btn</button>
<p><i>x</i><b>y</b><i>z</i><button>After tags</button></p>
</div></body>"""

# What the collector chose before the lookups were indexed, by element name.
EXPECTED_SELECTORS: dict[str, list[str]] = {
    "A": ["#app > ul > li:nth-of-type(1) > button"],
    "B": ["#app > ul > li:nth-of-type(2) > button"],
    "C": ['[data-testid="solo"]', "#app > ul > li:nth-of-type(3) > button"],
    "L1": ["#unique > span:nth-of-type(1) > a"],
    "L2": ["#unique > span:nth-of-type(2) > a"],
    "T1": ["#app > div:nth-of-type(1) > button"],
    "T2": ["#app > div:nth-of-type(2) > button"],
    "Odd id": [r"#a\:b > button"],
    "Btn": ["#btn", "#app > button"],
    "After tags": ["#app > p > button"],
}


class _Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass


@pytest.fixture(scope="module")
def site(tmp_path_factory: pytest.TempPathFactory) -> Iterator[str]:
    directory = tmp_path_factory.mktemp("large-dom")
    (directory / "large.html").write_text(LARGE_PAGE, encoding="utf-8")
    (directory / "selectors.html").write_text(SELECTOR_PAGE, encoding="utf-8")
    handler = functools.partial(_Quiet, directory=str(directory))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_address[1]}/"
    server.shutdown()


@pytest.fixture(scope="module")
def driver() -> Iterator[DriverClient]:
    client = DriverClient([sys.executable, "-m", "traceqa_driver_playwright"])
    client.call("driver.hello", HELLO)
    yield client
    client.shutdown()


def open_page(driver: DriverClient, site: str, browser: str, page: str) -> str:
    opened = driver.call("session.open", {"target": {"browser": browser}, "viewport": VIEWPORT})
    session: str = opened["sessionId"]
    driver.call("page.goto", {"sessionId": session, "url": site + page, "waitUntil": "settled"})
    return session


@pytest.mark.parametrize("browser", BROWSERS)
def test_20k_elements_are_read_within_budget(driver: DriverClient, site: str, browser: str) -> None:
    session = open_page(driver, site, browser, "large.html")
    try:
        # Timed inside the page, so that sending the result back is not counted.
        script = (
            "(() => { const t = performance.now(); const r = (0, eval)("
            + json.dumps(COLLECTOR)
            + ")({}); return { n: r.elements.length, ms: performance.now() - t }; })()"
        )
        raw = driver.call("page.eval", {"sessionId": session, "script": script, "arg": {}})
        result = raw["results"][0]["value"]
        print(f"{browser}: {result['n']} elements in {result['ms']:.0f} ms")
        assert result["n"] == 10000
        assert result["ms"] < BUDGET_MS, f"{browser}: {result['ms']:.0f} ms for 20 000 elements"
    finally:
        driver.call("session.close", {"sessionId": session})


@pytest.mark.parametrize("browser", BROWSERS)
def test_ids_testids_and_siblings_still_pick_the_same_selectors(driver: DriverClient, site: str, browser: str) -> None:
    session = open_page(driver, site, browser, "selectors.html")
    try:
        raw = driver.call("page.eval", {"sessionId": session, "script": COLLECTOR, "arg": {}})
        found: list[dict[str, Any]] = raw["results"][0]["value"]["elements"]
    finally:
        driver.call("session.close", {"sessionId": session})
    actual = {e["name"]: e["selectors"] for e in found}
    assert actual == EXPECTED_SELECTORS
