"""The collector against the case pages in fixtures/collector-cases/, in every browser.

Each case page states what the collector must report, on the elements themselves:

    <button id="save" data-expect="role=button; name=Save; source=content">Save</button>
    <div id="text" data-expect="absent">Plain text</div>

Every element with `data-expect` becomes one test per browser. A page also fails when the
collector lists an element that states no expectation. `data-known-gap="why"` marks a case
the collector is known to get wrong; it is reported as an expected failure until it is fixed.
See fixtures/collector-cases/README.md.
"""

from __future__ import annotations

import functools
import http.server
import sys
import threading
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from html.parser import HTMLParser
from pathlib import Path
from typing import Any

import pytest
from traceqa_protocol import contracts
from traceqa_protocol.testing import DriverClient

ROOT = Path(__file__).resolve().parents[2]
CASES = ROOT / "fixtures" / "collector-cases"
COLLECTOR = (ROOT / "collector" / "collector.js").read_text(encoding="utf-8")
BROWSERS = ["chromium", "firefox", "webkit"]
HELLO = {"protocolVersion": contracts.DRIVER_PROTOCOL_VERSION, "engineVersion": "test"}
VIEWPORT = {"width": 1280, "height": 800}


def optional_state(field: str) -> Callable[[dict[str, Any]], str]:
    return lambda element: str(element["state"].get(field, False)).lower()


# What each expectation key reads from a collected element.
FIELDS = {
    "role": lambda e: e["role"],
    "name": lambda e: e["name"],
    "namelength": lambda e: str(len(e["name"])),
    "source": lambda e: e.get("nameSource", ""),
    "tag": lambda e: e["tag"],
    "confidence": lambda e: e["interactive"]["confidence"],
    "signals": lambda e: ",".join(sorted(e["interactive"]["signals"])),
    "shadow": lambda e: str(e["inShadowDom"]).lower(),
    "visible": lambda e: str(e["state"]["visible"]).lower(),
    "enabled": lambda e: str(e["state"]["enabled"]).lower(),
    "inviewport": lambda e: str(e["state"]["inViewport"]).lower(),
    # Optional states: absent means false.
    **{key: optional_state(key) for key in ["checked", "expanded", "selected", "pressed", "readonly", "required"]},
}


@dataclass(frozen=True)
class Case:
    page: str
    id: str
    expect: dict[str, str]  # empty means the element must be absent
    gap: str | None


def parse_expectation(text: str) -> dict[str, str]:
    if text.strip() == "absent":
        return {}
    expect = {}
    for part in text.split(";"):
        if not part.strip():
            continue
        key, separator, value = part.partition("=")
        key = key.strip()
        if not separator or key not in FIELDS:
            raise ValueError(f"unknown expectation {part.strip()!r}. Known keys: {', '.join(FIELDS)}")
        value = value.strip()
        expect[key] = ",".join(sorted(value.split(","))) if key == "signals" else value
    return expect


class _Reader(HTMLParser):
    def __init__(self, page: str) -> None:
        super().__init__()
        self.page = page
        self.cases: list[Case] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        found = dict(attrs)
        if "data-expect" not in found:
            return
        if not found.get("id"):
            raise ValueError(f"{self.page}: a <{tag}> with data-expect has no id")
        expect = parse_expectation(found["data-expect"] or "")
        self.cases.append(Case(self.page, found["id"], expect, found.get("data-known-gap")))


def read_cases() -> list[Case]:
    cases = []
    for path in sorted(CASES.glob("*.html")):
        reader = _Reader(path.name)
        reader.feed(path.read_text(encoding="utf-8"))
        cases.extend(reader.cases)
    ids = [(case.page, case.id) for case in cases]
    duplicates = {item for item in ids if ids.count(item) > 1}
    if duplicates:
        raise ValueError(f"case ids must be unique within a page: {sorted(duplicates)}")
    return cases


ALL_CASES = read_cases()
PAGES = sorted({case.page for case in ALL_CASES})


@pytest.fixture(scope="module")
def site() -> Iterator[str]:
    handler = functools.partial(_QuietHandler, directory=str(CASES))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_address[1]}/"
    server.shutdown()


class _QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass


class Readings:
    """Runs the collector once per browser and page, and keeps the result for every case on it."""

    def __init__(self, site: str) -> None:
        self.site = site
        self.driver = DriverClient([sys.executable, "-m", "traceqa_driver_playwright"])
        self.driver.call("driver.hello", HELLO)
        self.sessions: dict[str, str] = {}
        self.pages: dict[tuple[str, str], list[dict[str, Any]]] = {}

    def elements(self, browser: str, page: str) -> list[dict[str, Any]]:
        if (browser, page) not in self.pages:
            if browser not in self.sessions:
                opened = self.driver.call("session.open", {"target": {"browser": browser}, "viewport": VIEWPORT})
                self.sessions[browser] = opened["sessionId"]
            session = self.sessions[browser]
            self.driver.call("page.goto", {"sessionId": session, "url": self.site + page, "waitUntil": "settled"})
            raw = self.driver.call("page.eval", {"sessionId": session, "script": COLLECTOR, "arg": {}})
            self.pages[(browser, page)] = raw["results"][0]["value"]["elements"]
        return self.pages[(browser, page)]

    def close(self) -> None:
        for session in self.sessions.values():
            self.driver.call("session.close", {"sessionId": session})
        self.driver.shutdown()


@pytest.fixture(scope="module")
def readings(site: str) -> Iterator[Readings]:
    readings = Readings(site)
    yield readings
    readings.close()


def test_the_case_pages_are_found() -> None:
    assert len(PAGES) >= 6 and len(ALL_CASES) >= 80


@pytest.mark.parametrize("browser", BROWSERS)
@pytest.mark.parametrize("case", ALL_CASES, ids=lambda case: f"{case.page.removesuffix('.html')}:{case.id}")
def test_case(readings: Readings, browser: str, case: Case) -> None:
    found = [e for e in readings.elements(browser, case.page) if e["attrs"].get("id") == case.id]
    problem = mismatch(case, found)
    if case.gap:
        if problem is None:
            pytest.fail(f"#{case.id} now passes in {browser}: the known gap is fixed, remove data-known-gap")
        pytest.xfail(f"known gap: {case.gap}")
    assert problem is None, f"#{case.id} in {browser}: {problem}"


def mismatch(case: Case, found: list[dict[str, Any]]) -> str | None:
    """What is wrong with what the collector reported for one case, or None."""
    if not case.expect:
        return f"must not be listed, but was: {found[0]['role']} {found[0]['name']!r}" if found else None
    if len(found) != 1:
        return f"must be listed once, was listed {len(found)} times"
    actual = {key: FIELDS[key](found[0]) for key in case.expect}
    wrong = {key: f"{actual[key]!r}, expected {value!r}" for key, value in case.expect.items() if actual[key] != value}
    return "; ".join(f"{key} is {text}" for key, text in wrong.items()) or None


@pytest.mark.parametrize("browser", BROWSERS)
@pytest.mark.parametrize("page", PAGES)
def test_nothing_unexpected_is_listed(readings: Readings, browser: str, page: str) -> None:
    expected = {case.id for case in ALL_CASES if case.page == page and case.expect}
    extra = [
        f"{e['tag']}#{e['attrs'].get('id', '?')} {e['role']} {e['name']!r}"
        for e in readings.elements(browser, page)
        if e["attrs"].get("id") not in expected
    ]
    assert not extra, f"listed without an expectation on {page} in {browser}: {extra}"
