"""Drives the real driver process against the Buggy Shop.

WebDriver is one protocol for every browser, so the driver is exercised with
chromedriver, which is easy to run headless. Set TRACEQA_CHROMEDRIVER or put
chromedriver on PATH. Without it the test is skipped, unless
TRACEQA_REQUIRE_CHROMEDRIVER=1, which makes a missing chromedriver a failure.
The Safari test needs "Allow remote automation" switched on in Safari's
Developer settings, and runs only when TRACEQA_TEST_SAFARI=1.
"""

from __future__ import annotations

import functools
import http.server
import os
import shutil
import socket
import subprocess
import sys
import threading
import time
import urllib.request
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
from traceqa_protocol import contracts, models
from traceqa_protocol.testing import DriverClient, RpcFailure

ROOT = Path(__file__).resolve().parents[3]
COLLECTOR = (ROOT / "page-script" / "page-script.js").read_text(encoding="utf-8")
HELLO = {"protocolVersion": contracts.DRIVER_PROTOCOL_VERSION, "engineVersion": "test"}
VIEWPORT = {"width": 1280, "height": 800}
CHROMEDRIVER = os.environ.get("TRACEQA_CHROMEDRIVER") or shutil.which("chromedriver")


@pytest.fixture(scope="module")
def chrome_target() -> Iterator[dict[str, Any]]:
    if not CHROMEDRIVER or not Path(CHROMEDRIVER).is_file():
        missing = "chromedriver not found: set TRACEQA_CHROMEDRIVER or put it on PATH"
        # CI sets this, so a machine without chromedriver fails loudly instead of skipping the driver's tests.
        if os.environ.get("TRACEQA_REQUIRE_CHROMEDRIVER") == "1":
            pytest.fail(missing)
        pytest.skip(missing)
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    server = subprocess.Popen([CHROMEDRIVER, f"--port={port}"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    url = f"http://127.0.0.1:{port}"
    for _ in range(50):
        try:
            urllib.request.urlopen(f"{url}/status", timeout=1).close()
            break
        except OSError:
            time.sleep(0.1)
    # --no-sandbox: Chrome's sandbox cannot start on some CI machines, and this only ever opens the demo shop.
    # The --disable flags are ones Playwright always passes: without them Chrome slows the timers of a
    # window it thinks is in the background, which a CI machine's headless window can be.
    args = [
        "--headless=new",
        "--no-sandbox",
        "--disable-background-timer-throttling",
        "--disable-backgrounding-occluded-windows",
        "--disable-renderer-backgrounding",
    ]
    capabilities = {"browserName": "chrome", "goog:chromeOptions": {"args": args}}
    yield {"browser": "webdriver", "webdriverUrl": url, "webdriverCapabilities": capabilities}
    server.terminate()
    server.wait(timeout=10)


@pytest.fixture
def driver() -> Iterator[DriverClient]:
    client = DriverClient([sys.executable, "-m", "traceqa_driver_webdriver"])
    yield client
    client.shutdown()


def names(result: models.CollectorResult, role: str) -> list[str]:
    return [element.name for element in result.elements if element.role == role]


def collect(driver: DriverClient, session_id: str) -> dict[str, models.FrameEvalResult]:
    raw = driver.call("page.eval", {"sessionId": session_id, "script": COLLECTOR, "arg": {}, "allFrames": True})
    results = models.PageEvalResult.model_validate(raw).results
    return {result.frame.name or result.frame.id: result for result in results}


LAZY_REVIEWS = 9


def scroll_until_the_reviews_load(
    driver: DriverClient, session_id: str, limit_s: float = 20.0
) -> tuple[dict[str, Any], dict[str, models.FrameEvalResult]]:
    """Scroll to the bottom and read the page until every lazy review has loaded, or time runs out.

    The reviews load when the browser sees the end of the list, which it only checks when it
    draws the page. Chrome under chromedriver on a CI machine can draw seldom, so the last
    batch may arrive seconds later. Waiting for the result, not for a number of scrolls,
    makes the test as strict on a slow machine as on a fast one.
    """
    deadline = time.monotonic() + limit_s
    while True:
        driver.call("page.scroll", {"sessionId": session_id, "to": "bottom"})
        frames = collect(driver, session_id)
        main = models.CollectorResult.model_validate(frames["main"].value)
        if names(main, "button").count("Helpful") >= LAZY_REVIEWS or time.monotonic() > deadline:
            break
        time.sleep(0.25)
    # The last reviews made the page longer, so scroll once more to stand at its new end.
    return driver.call("page.scroll", {"sessionId": session_id, "to": "bottom"}), frames


def read_the_shop(driver: DriverClient, target: dict[str, Any], shop_url: str, tmp_path: Path) -> None:
    assert target["browser"] in driver.call("driver.hello", HELLO)["browsers"]
    session = driver.call("session.open", {"target": target, "viewport": VIEWPORT})
    session_id = session["sessionId"]
    # What the driver cannot do is declared, so the engine never asks for it.
    assert session["capabilities"]["fullPageScreenshot"] is False

    page = driver.call("page.goto", {"sessionId": session_id, "url": shop_url, "waitUntil": "settled"})
    assert page["title"] == "Buggy Shop" and page["finalUrl"] == shop_url

    frames = collect(driver, session_id)
    assert set(frames) == {"main", "newsletter", "pay"}
    main = models.CollectorResult.model_validate(frames["main"].value)
    assert names(main, "button").count("Add to cart") == 12
    assert [element.in_shadow_dom for element in main.elements if element.name == "Increase quantity"] == [True]
    pay = frames["pay"]
    assert pay.frame.cross_origin and not frames["newsletter"].frame.cross_origin
    pay_box, newsletter_box = pay.frame.box, frames["newsletter"].frame.box
    assert pay_box is not None
    assert newsletter_box is not None
    assert pay_box.w == 320 and pay_box.y > newsletter_box.y
    assert names(models.CollectorResult.model_validate(pay.value), "button") == ["Pay now"]

    scrolled, again = scroll_until_the_reviews_load(driver, session_id)
    assert scrolled["atEnd"]
    assert names(models.CollectorResult.model_validate(again["main"].value), "button").count("Helpful") == LAZY_REVIEWS
    # Frames keep their ids from one call to the next.
    assert {name: result.frame.id for name, result in again.items()} == {
        name: result.frame.id for name, result in frames.items()
    }

    shot = driver.call("page.screenshot", {"sessionId": session_id, "path": str(tmp_path / "page.png")})
    assert (tmp_path / "page.png").stat().st_size > 0 and shot["width"] >= VIEWPORT["width"]
    with pytest.raises(RpcFailure) as full_page:
        driver.call("page.screenshot", {"sessionId": session_id, "path": str(tmp_path / "full.png"), "fullPage": True})
    assert full_page.value.kind == "unsupported"

    driver.call("session.close", {"sessionId": session_id})
    with pytest.raises(RpcFailure) as closed:
        driver.call("page.goto", {"sessionId": session_id, "url": shop_url})
    assert closed.value.kind == "session_not_found"


def test_reads_the_shop_through_a_webdriver_server(
    driver: DriverClient, chrome_target: dict[str, Any], shop_url: str, tmp_path: Path
) -> None:
    read_the_shop(driver, chrome_target, shop_url, tmp_path)


@pytest.mark.skipif(
    os.environ.get("TRACEQA_TEST_SAFARI") != "1", reason="set TRACEQA_TEST_SAFARI=1 to drive real Safari"
)
def test_reads_the_shop_in_safari(driver: DriverClient, shop_url: str, tmp_path: Path) -> None:
    read_the_shop(driver, {"browser": "safari"}, shop_url, tmp_path)


def test_refuses_what_it_cannot_do(driver: DriverClient) -> None:
    cases = [
        ("driver.hello", {"protocolVersion": 999, "engineVersion": "test"}, "version_mismatch"),
        ("session.open", {"target": {"browser": "firefox"}, "viewport": VIEWPORT}, "unsupported"),
        ("session.open", {"target": {"browser": "webdriver"}, "viewport": VIEWPORT}, "invalid_params"),
        # Nothing listens on port 9: the failure is reported, not raised through the pipe.
        (
            "session.open",
            {"target": {"browser": "webdriver", "webdriverUrl": "http://127.0.0.1:9"}, "viewport": VIEWPORT},
            "internal",
        ),
        ("page.act", {"sessionId": "s1", "action": "click", "target": {"selector": "a"}}, "unsupported"),
    ]
    for method, params, kind in cases:
        with pytest.raises(RpcFailure) as failure:
            driver.call(method, params)
        assert failure.value.kind == kind, method
    assert driver.call("driver.hello", HELLO)["driverName"] == "traceqa-driver-webdriver"


class _QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass


@pytest.fixture(scope="module")
def case_pages() -> Iterator[str]:
    """testbed/sites/collector-cases/ on a free port."""
    handler = functools.partial(_QuietHandler, directory=str(ROOT / "testbed" / "sites" / "collector-cases"))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_address[1]}/"
    server.shutdown()


def test_a_hostile_page_makes_the_collector_report_tampering(
    driver: DriverClient, chrome_target: dict[str, Any], case_pages: str
) -> None:
    # WebDriver cannot run a script before the page's, so the collector cannot get the originals.
    # The call must still return, with a warning, and not fail the frame.
    driver.call("driver.hello", HELLO)
    session = driver.call("session.open", {"target": chrome_target, "viewport": VIEWPORT})
    session_id = session["sessionId"]
    driver.call("page.goto", {"sessionId": session_id, "url": case_pages + "hostile.html", "waitUntil": "settled"})

    main = models.CollectorResult.model_validate(collect(driver, session_id)["main"].value)
    tampered = [warning for warning in main.warnings if warning.code == "collector_tampered"]
    assert len(tampered) == 1
    assert "String.prototype.trim" in tampered[0].message
    assert "could not supply the originals" in tampered[0].message

    # A normal page in the same browser raises no such warning.
    driver.call("page.goto", {"sessionId": session_id, "url": case_pages + "names.html", "waitUntil": "settled"})
    clean = models.CollectorResult.model_validate(collect(driver, session_id)["main"].value)
    assert [warning.code for warning in clean.warnings if warning.code == "collector_tampered"] == []
    driver.call("session.close", {"sessionId": session_id})


ASYNC_DOUBLE = "async (n) => { await new Promise((resolve) => setTimeout(resolve, 30)); return n * 2; }"


def test_eval_waits_for_a_promise_when_asked(
    driver: DriverClient, chrome_target: dict[str, Any], shop_url: str
) -> None:
    driver.call("driver.hello", HELLO)
    session = driver.call("session.open", {"target": chrome_target, "viewport": VIEWPORT})
    session_id = session["sessionId"]
    assert session["capabilities"]["evalAsync"] is True
    driver.call("page.goto", {"sessionId": session_id, "url": shop_url, "waitUntil": "settled"})

    waited = driver.call(
        "page.eval",
        {"sessionId": session_id, "script": ASYNC_DOUBLE, "arg": 21, "awaitPromise": True, "allFrames": True},
    )
    values = {result["frame"]["id"]: result["value"] for result in waited["results"]}
    assert len(values) == 3 and set(values.values()) == {42}
    # Without the flag the old synchronous path runs.
    plain = driver.call("page.eval", {"sessionId": session_id, "script": "(n) => n + 1", "arg": 1})
    assert plain["results"][0]["value"] == 2

    with pytest.raises(RpcFailure) as rejected:
        driver.call(
            "page.eval",
            {
                "sessionId": session_id,
                "script": "async () => { throw new Error('nope from the page'); }",
                "awaitPromise": True,
            },
        )
    assert rejected.value.kind == "eval_failed"
    assert "nope from the page" in str(rejected.value)
    # A promise that never settles ends at the script timeout, not never.
    with pytest.raises(RpcFailure) as hung:
        driver.call(
            "page.eval",
            {
                "sessionId": session_id,
                "script": "() => new Promise(() => {})",
                "awaitPromise": True,
                "timeoutMs": 700,
            },
        )
    assert hung.value.kind == "eval_failed"
    driver.call("session.close", {"sessionId": session_id})
