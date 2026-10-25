"""The collector against the case pages in testbed/sites/collector-cases/, in every browser.

Each case page states what the collector must report, on the elements themselves:

    <button id="save" data-expect="role=button; name=Save; source=content">Save</button>
    <div id="text" data-expect="absent">Plain text</div>

Every element with `data-expect` becomes one test per browser. A page also fails when the
collector lists an element that states no expectation. `data-known-gap="why"` marks a case
the collector is known to get wrong; it is reported as an expected failure until it is fixed.
See testbed/sites/collector-cases/README.md.
"""

from __future__ import annotations

import functools
import http.server
import importlib.util
import os
import sys
import threading
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import pytest
from traceqa_protocol import contracts
from traceqa_protocol.testing import DriverClient

ROOT = Path(__file__).resolve().parents[2]
CASES = ROOT / "testbed" / "sites" / "collector-cases"
COLLECTOR = (ROOT / "page-script" / "page-script.js").read_text(encoding="utf-8")
# Real Safari runs only where it can: on a Mac with "Allow remote automation" switched on,
# which CI machines are not. TRACEQA_TEST_SAFARI=1 adds it, as in the WebDriver driver's tests.
BROWSERS = ["chromium", "firefox", "webkit"] + (["safari"] if os.environ.get("TRACEQA_TEST_SAFARI") == "1" else [])
# Safari is driven by the WebDriver driver, every other browser by the Playwright driver.
DRIVER_OF = {"safari": "traceqa_driver_webdriver"}
HELLO = {"protocolVersion": contracts.DRIVER_PROTOCOL_VERSION, "engineVersion": "test"}
VIEWPORT = {"width": 1280, "height": 800}
HOSTILE = "hostile.html"


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
    # Copied attributes; "-" means the attribute is absent.
    "attrtabindex": lambda e: e["attrs"].get("tabindex", "-"),
    "attrautocomplete": lambda e: e["attrs"].get("autocomplete", "-"),
    "attrrole": lambda e: e["attrs"].get("role", "-"),
    "shadow": lambda e: str(e["inShadowDom"]).lower(),
    "visible": lambda e: str(e["state"]["visible"]).lower(),
    "enabled": lambda e: str(e["state"]["enabled"]).lower(),
    "inviewport": lambda e: str(e["state"]["inViewport"]).lower(),
    # Tested only for visible elements on screen: absent means it was not tested.
    "covered": lambda e: str(e["state"]["covered"]).lower() if "covered" in e["state"] else "unchecked",
    "coveredby": lambda e: e["state"].get("coveredBy", ""),
    # Optional states: absent means false.
    **{key: optional_state(key) for key in ["checked", "expanded", "selected", "pressed", "readonly", "required"]},
}


def _load_expectations():
    spec = importlib.util.spec_from_file_location("expectations", ROOT / "tests" / "support" / "expectations.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module  # dataclasses look their module up while being defined
    spec.loader.exec_module(module)
    return module


expectations = _load_expectations()
Case = expectations.Case


ALL_CASES = expectations.read_cases(CASES, FIELDS)
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
        self.drivers: dict[str, DriverClient] = {}
        self.sessions: dict[str, tuple[DriverClient, str]] = {}
        self.pages: dict[tuple[str, str], dict[str, Any]] = {}

    def _session(self, browser: str) -> tuple[DriverClient, str]:
        if browser not in self.sessions:
            module = DRIVER_OF.get(browser, "traceqa_driver_playwright")
            if module not in self.drivers:
                self.drivers[module] = DriverClient([sys.executable, "-m", module])
                self.drivers[module].call("driver.hello", HELLO)
            driver = self.drivers[module]
            opened = driver.call("session.open", {"target": {"browser": browser}, "viewport": VIEWPORT})
            self.sessions[browser] = (driver, opened["sessionId"])
        return self.sessions[browser]

    def result(self, browser: str, page: str) -> dict[str, Any]:
        if (browser, page) not in self.pages:
            driver, session = self._session(browser)
            driver.call("page.goto", {"sessionId": session, "url": self.site + page, "waitUntil": "settled"})
            raw = driver.call("page.eval", {"sessionId": session, "script": COLLECTOR, "arg": {}})
            self.pages[(browser, page)] = raw["results"][0]["value"]
        return self.pages[(browser, page)]

    def elements(self, browser: str, page: str) -> list[dict[str, Any]]:
        return self.result(browser, page)["elements"]

    def close(self) -> None:
        for driver, session in self.sessions.values():
            driver.call("session.close", {"sessionId": session})
        for driver in self.drivers.values():
            driver.shutdown()


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
    skip_if_no_init_script(browser, case.page)
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
    skip_if_no_init_script(browser, page)
    expected = {case.id for case in ALL_CASES if case.page == page and case.expect}
    extra = [
        f"{e['tag']}#{e['attrs'].get('id', '?')} {e['role']} {e['name']!r}"
        for e in readings.elements(browser, page)
        if e["attrs"].get("id") not in expected
    ]
    assert not extra, f"listed without an expectation on {page} in {browser}: {extra}"


def skip_if_no_init_script(browser: str, page: str) -> None:
    """The hostile page is only read correctly where a driver can capture the originals first."""
    if page == HOSTILE and browser in DRIVER_OF:
        pytest.skip("the WebDriver driver has no hook to run a script before the page's")


def warning_codes(result: dict[str, Any]) -> list[str]:
    return [warning["code"] for warning in result["warnings"]]


@pytest.mark.parametrize("browser", [b for b in BROWSERS if b not in DRIVER_OF])
def test_a_hostile_page_is_reported_as_tampered_and_still_read_correctly(readings: Readings, browser: str) -> None:
    # The elements are checked by test_case. Here: the page's replacements are named.
    warnings = [w for w in readings.result(browser, HOSTILE)["warnings"] if w["code"] == "collector_tampered"]
    assert len(warnings) == 1, warning_codes(readings.result(browser, HOSTILE))
    message = warnings[0]["message"]
    for replaced in [
        "Array.prototype.map",
        "Element.prototype.getBoundingClientRect",
        "window.getComputedStyle",
        "Set.prototype.has",
        "String.prototype.trim",
    ]:
        assert replaced in message, message
    assert "Function.prototype.toString" not in message
    assert "original functions" in message


@pytest.mark.parametrize("browser", BROWSERS)
@pytest.mark.parametrize("page", [page for page in PAGES if page != HOSTILE])
def test_a_normal_page_is_not_reported_as_tampered(readings: Readings, browser: str, page: str) -> None:
    assert "collector_tampered" not in warning_codes(readings.result(browser, page))
