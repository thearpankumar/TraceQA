"""Drives the real driver process against the Buggy Shop in three browsers."""

from __future__ import annotations

import socket
import subprocess
import time
import urllib.request
from pathlib import Path

import pytest
from playwright.sync_api import sync_playwright
from traceqa_protocol import contracts, models
from traceqa_protocol.testing import DriverClient, RpcFailure

COLLECTOR = (Path(__file__).resolve().parents[3] / "collector" / "collector.js").read_text(encoding="utf-8")
HELLO = {"protocolVersion": contracts.DRIVER_PROTOCOL_VERSION, "engineVersion": "test"}
VIEWPORT = {"width": 1280, "height": 800}


def names(result: models.CollectorResult, role: str) -> list[str]:
    return [element.name for element in result.elements if element.role == role]


def collect(driver: DriverClient, session_id: str) -> dict[str, models.FrameEvalResult]:
    """Run the collector in every frame and key the results by frame name."""
    raw = driver.call("page.eval", {"sessionId": session_id, "script": COLLECTOR, "arg": {}, "allFrames": True})
    results = models.PageEvalResult.model_validate(raw).results
    return {result.frame.name or result.frame.id: result for result in results}


@pytest.mark.parametrize("browser", ["chromium", "firefox", "webkit"])
def test_reads_the_shop(driver: DriverClient, shop_url: str, browser: str, tmp_path: Path) -> None:
    hello = driver.call("driver.hello", HELLO)
    assert browser in hello["browsers"]

    session = driver.call("session.open", {"target": {"browser": browser}, "viewport": VIEWPORT})
    session_id = session["sessionId"]
    assert session["browser"]["version"]

    page = driver.call("page.goto", {"sessionId": session_id, "url": shop_url, "waitUntil": "settled"})
    assert page["title"] == "Buggy Shop"
    assert page["status"] == 200

    frames = collect(driver, session_id)
    assert set(frames) == {"main", "newsletter", "pay"}
    main = models.CollectorResult.model_validate(frames["main"].value)
    assert names(main, "button").count("Add to cart") == 12
    assert names(main, "button").count("Helpful") == 3
    assert [element.in_shadow_dom for element in main.elements if element.name == "Increase quantity"] == [True]

    # A frame from another origin is read like any other, and placed on the page.
    pay = frames["pay"]
    assert pay.frame.cross_origin and not frames["newsletter"].frame.cross_origin
    assert pay.frame.box is not None and pay.frame.box.w > 0
    assert names(models.CollectorResult.model_validate(pay.value), "button") == ["Pay now"]

    # Scrolling to the end makes the lazy reviews load. They load when the browser next draws
    # the page, which a slow machine does seldom, so wait for them rather than count scrolls.
    deadline = time.monotonic() + 20.0
    while True:
        driver.call("page.scroll", {"sessionId": session_id, "to": "bottom"})
        main = models.CollectorResult.model_validate(collect(driver, session_id)["main"].value)
        if names(main, "button").count("Helpful") >= 9 or time.monotonic() > deadline:
            break
        time.sleep(0.25)
    # The last reviews made the page longer, so scroll once more to stand at its new end.
    scrolled = driver.call("page.scroll", {"sessionId": session_id, "to": "bottom"})
    assert scrolled["atEnd"]
    assert names(main, "button").count("Helpful") == 9

    shot = driver.call(
        "page.screenshot", {"sessionId": session_id, "path": str(tmp_path / "page.png"), "fullPage": True}
    )
    assert (tmp_path / "page.png").stat().st_size > 0
    assert shot["width"] >= VIEWPORT["width"] and shot["height"] > VIEWPORT["height"]

    driver.call("session.close", {"sessionId": session_id})
    with pytest.raises(RpcFailure) as closed:
        driver.call("page.goto", {"sessionId": session_id, "url": shop_url})
    assert closed.value.kind == "session_not_found"


def test_attaches_to_a_running_browser_over_cdp(driver: DriverClient, shop_url: str, tmp_path: Path) -> None:
    """The path used for agent browsers and cloud browsers: they are already running and expose CDP."""
    with sync_playwright() as playwright:
        chromium = playwright.chromium.executable_path
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    flags = [
        f"--remote-debugging-port={port}",
        "--headless=new",
        # Playwright passes this too. Ubuntu 24.04 does not let Chromium start its sandbox in CI.
        "--no-sandbox",
        f"--user-data-dir={tmp_path / 'profile'}",
        "about:blank",
    ]
    log = tmp_path / "chromium.log"
    with log.open("wb") as output:
        browser = subprocess.Popen([chromium, *flags], stdout=output, stderr=subprocess.STDOUT)
    endpoint = f"http://127.0.0.1:{port}"
    try:
        for _ in range(100):
            try:
                urllib.request.urlopen(f"{endpoint}/json/version", timeout=1).close()
                break
            except OSError:
                time.sleep(0.1)
        else:
            exited = "" if browser.poll() is None else f" It exited with {browser.returncode}."
            pytest.fail(
                f"Chromium did not open its CDP port in 10 s.{exited} Its output:\n{log.read_text(errors='replace')[-2000:]}"
            )

        driver.call("driver.hello", HELLO)
        session = driver.call(
            "session.open", {"target": {"browser": "cdp", "cdpEndpoint": endpoint}, "viewport": VIEWPORT}
        )
        session_id = session["sessionId"]
        driver.call("page.goto", {"sessionId": session_id, "url": shop_url, "waitUntil": "settled"})
        frames = collect(driver, session_id)
        assert set(frames) == {"main", "newsletter", "pay"}
        main = models.CollectorResult.model_validate(frames["main"].value)
        assert names(main, "button").count("Add to cart") == 12
        # The viewport that was asked for is the one the page sees.
        assert main.scroll.viewport_height == VIEWPORT["height"]

        driver.call("session.close", {"sessionId": session_id})
        # Closing the session lets go of the browser. It does not kill a browser we did not start.
        assert browser.poll() is None
    finally:
        browser.terminate()
        browser.wait(timeout=10)


def test_refuses_what_it_cannot_do(driver: DriverClient) -> None:
    cases = [
        ("driver.hello", {"protocolVersion": 999, "engineVersion": "test"}, "version_mismatch"),
        ("session.open", {"target": {"browser": "netscape"}, "viewport": VIEWPORT}, "unsupported"),
        ("session.open", {"target": {"browser": "cdp"}, "viewport": VIEWPORT}, "invalid_params"),
        ("page.goto", {"url": "http://127.0.0.1/"}, "invalid_params"),
        ("page.act", {"sessionId": "s1", "action": "click", "target": {"selector": "a"}}, "unsupported"),
        ("page.fly", {}, "unsupported"),
    ]
    for method, params, kind in cases:
        with pytest.raises(RpcFailure) as failure:
            driver.call(method, params)
        assert failure.value.kind == kind, method
    # The driver is still alive and answering after six failures.
    assert driver.call("driver.hello", HELLO)["driverName"] == "traceqa-driver-playwright"


# What a page's script does to try to take the original built-ins away from the collector. Runs in
# the page's world after the driver's init script, as any page script would.
ATTACK = """() => {
  const outcome = {};
  try { Object.defineProperty(window, "__traceqa_pristine__", { value: () => ({}) }); outcome.define = "allowed"; }
  catch (e) { outcome.define = "refused"; }
  outcome.delete = delete window.__traceqa_pristine__ ? "allowed" : "refused";
  try { window.__traceqa_pristine__ = () => ({}); } catch (e) { /* strict mode would throw */ }
  outcome.kept = typeof window.__traceqa_pristine__ === "function" && typeof window.__traceqa_pristine__().getAttribute === "function";
  outcome.enumerable = Object.keys(window).includes("__traceqa_pristine__");
  return outcome;
}"""

# Then it replaces functions the collector would otherwise call, with ones that count their use.
# (Ones that throw would break Playwright's own evaluate as well.)
SPY = """() => {
  window.__hits = { call: 0, push: 0, trim: 0 };
  const call = Function.prototype.call, push = Array.prototype.push, trim = String.prototype.trim;
  Function.prototype.call = function (...args) { window.__hits.call++; return call.apply(this, args); };
  Array.prototype.push = function (...args) { window.__hits.push++; return push.apply(this, args); };
  String.prototype.trim = function () { window.__hits.trim++; return trim.call(this); };
  return true;
}"""


@pytest.mark.parametrize("browser", ["chromium", "firefox", "webkit"])
def test_the_pristine_table_cannot_be_replaced_or_deleted_by_the_page(
    driver: DriverClient, shop_url: str, browser: str
) -> None:
    driver.call("driver.hello", HELLO)
    session = driver.call("session.open", {"target": {"browser": browser}, "viewport": VIEWPORT})
    session_id = session["sessionId"]
    driver.call("page.goto", {"sessionId": session_id, "url": shop_url, "waitUntil": "settled"})

    result = driver.call("page.eval", {"sessionId": session_id, "script": ATTACK, "arg": {}})["results"][0]
    outcome = result.get("value", result)
    assert outcome == {"define": "refused", "delete": "refused", "kept": True, "enumerable": False}
    driver.call("page.eval", {"sessionId": session_id, "script": SPY, "arg": {}})

    # The collector still reads the page, and says the page replaced built-ins.
    frames = collect(driver, session_id)
    main = models.CollectorResult.model_validate(frames["main"].value)
    assert names(main, "button").count("Add to cart") == 12
    codes = [warning.code for warning in main.warnings]
    assert codes.count("collector_tampered") == 1
    # ... and it read the page without calling the replaced String.prototype.trim, which the shop's own
    # scripts and Playwright never call but the collector used to. (call and push are counted for the
    # record only: the page's scripts and Playwright's evaluate use them too.)
    hits = driver.call("page.eval", {"sessionId": session_id, "script": "() => window.__hits", "arg": {}})
    assert hits["results"][0]["value"]["trim"] == 0
    driver.call("session.close", {"sessionId": session_id})
