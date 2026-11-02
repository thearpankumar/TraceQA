"""Drives the real driver process against the Buggy Shop in three browsers."""

from __future__ import annotations

import contextlib
import socket
import subprocess
import threading
import time
import urllib.request
from collections.abc import Iterator
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest
from playwright.sync_api import sync_playwright
from traceqa_protocol import contracts, models
from traceqa_protocol.testing import DriverClient, RpcFailure

COLLECTOR = (Path(__file__).resolve().parents[3] / "page-script" / "page-script.js").read_text(encoding="utf-8")
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
        ("page.act", {"sessionId": "s1", "action": "click", "target": {"selector": "a"}}, "session_not_found"),
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


class Redirects(BaseHTTPRequestHandler):
    """/start goes to /middle goes to /end, which answers. /hang never answers."""

    def do_GET(self) -> None:
        path = self.path.split("?")[0]
        if path in ("/start", "/middle"):
            self.send_response(302)
            self.send_header("Location", "/middle" if path == "/start" else "/end?step=3")
            self.send_header("Content-Length", "0")
            self.end_headers()
        elif path == "/hang":
            time.sleep(8)
        else:
            body = b"<!doctype html><title>End</title><p>done</p>"
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        pass


@pytest.fixture
def redirecting() -> Iterator[str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), Redirects)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_address[1]}"
    server.shutdown()
    server.server_close()


@pytest.mark.parametrize("browser", ["chromium", "firefox", "webkit"])
def test_goto_reports_the_redirect_chain_with_server_addresses(
    driver: DriverClient, redirecting: str, browser: str
) -> None:
    driver.call("driver.hello", HELLO)
    session = driver.call("session.open", {"target": {"browser": browser}, "viewport": VIEWPORT})
    page = driver.call("page.goto", {"sessionId": session["sessionId"], "url": f"{redirecting}/start"})
    chain = models.PageGotoResult.model_validate(page).redirect_chain
    assert chain is not None
    assert [hop.url for hop in chain] == [f"{redirecting}/start", f"{redirecting}/middle", f"{redirecting}/end?step=3"]
    assert page["finalUrl"] == chain[-1].url
    if browser == "webkit":
        # WebKit gives no address for the hops that redirected. The engine resolves their names instead.
        assert all(hop.remote_address in (None, "127.0.0.1") for hop in chain)
    else:
        assert [hop.remote_address for hop in chain] == ["127.0.0.1"] * 3
    driver.call("session.close", {"sessionId": session["sessionId"]})


def test_a_timeout_message_does_not_carry_the_query(driver: DriverClient, redirecting: str) -> None:
    driver.call("driver.hello", HELLO)
    session = driver.call("session.open", {"target": {"browser": "chromium"}, "viewport": VIEWPORT})
    with pytest.raises(RpcFailure) as failure:
        driver.call(
            "page.goto",
            {
                "sessionId": session["sessionId"],
                "url": f"{redirecting}/hang?token=SEKRET#access_token=SEKRET",
                "timeoutMs": 1500,
            },
        )
    assert failure.value.kind == "timeout"
    assert "SEKRET" not in str(failure.value)
    assert f"{redirecting}/hang did not load" in str(failure.value)


class Failing(BaseHTTPRequestHandler):
    """/page loads a script that answers 503 and a picture that answers 404. /navigates starts a
    slow request and leaves the page at once, so the browser cancels it. /clean fails at nothing."""

    def do_GET(self) -> None:
        path = self.path.split("?")[0]
        status, kind, body = 200, "text/html", b"<!doctype html><title>Page</title><p>hi</p>"
        if path == "/page":
            body = b'<!doctype html><title>P</title><script src="/app.js?token=SEKRET"></script><img src="/pic.png">'
        elif path == "/app.js":
            status, kind, body = 503, "application/javascript", b"nope"
        elif path == "/pic.png":
            status, kind, body = 404, "image/png", b""
        elif path == "/navigates":
            body = b"<!doctype html><title>N</title><script>fetch('/slow'); location.replace('/clean');</script>"
        elif path == "/slow":
            time.sleep(2)
        with contextlib.suppress(OSError):
            self.send_response(status)
            self.send_header("Content-Type", kind)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        pass


@pytest.fixture
def failing() -> Iterator[str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), Failing)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_address[1]}"
    server.shutdown()
    server.server_close()


def failed_requests(driver: DriverClient, session_id: str) -> models.RequestsFailed | None:
    raw = driver.call("page.eval", {"sessionId": session_id, "script": "() => 1", "arg": {}})
    return models.PageEvalResult.model_validate(raw).requests_failed


@pytest.mark.parametrize("browser", ["chromium", "firefox", "webkit"])
def test_failed_script_requests_are_counted_without_the_query(driver: DriverClient, failing: str, browser: str) -> None:
    hello = driver.call("driver.hello", HELLO)
    assert browser in hello["browsers"]
    session = driver.call("session.open", {"target": {"browser": browser}, "viewport": VIEWPORT})
    assert session["capabilities"]["failedRequests"] is True
    sid = session["sessionId"]

    # The script answers 503 and counts; the picture answers 404 and does not.
    driver.call("page.goto", {"sessionId": sid, "url": f"{failing}/page", "waitUntil": "settled"})
    result = failed_requests(driver, sid)
    assert result is not None
    assert (result.count, result.urls) == (1, [f"{failing}/app.js"])

    # A request the page's own navigation cancels is not a failure, and the count starts over.
    driver.call("page.goto", {"sessionId": sid, "url": f"{failing}/navigates", "waitUntil": "settled"})
    result = failed_requests(driver, sid)
    assert result is not None
    assert (result.count, result.urls) == (0, [])
    driver.call("session.close", {"sessionId": sid})


ASYNC_DOUBLE = "async (n) => { await new Promise((resolve) => setTimeout(resolve, 30)); return n * 2; }"


@pytest.mark.parametrize("browser", ["chromium", "firefox", "webkit"])
def test_eval_waits_for_a_promise_when_asked(driver: DriverClient, shop_url: str, browser: str) -> None:
    driver.call("driver.hello", HELLO)
    session = driver.call("session.open", {"target": {"browser": browser}, "viewport": VIEWPORT})
    session_id = session["sessionId"]
    assert session["capabilities"]["evalAsync"] is True
    driver.call("page.goto", {"sessionId": session_id, "url": shop_url, "waitUntil": "settled"})

    waited = driver.call(
        "page.eval", {"sessionId": session_id, "script": ASYNC_DOUBLE, "arg": 21, "awaitPromise": True}
    )
    assert waited["results"][0]["value"] == 42
    # A synchronous function is unchanged by the flag being absent.
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
    driver.call("session.close", {"sessionId": session_id})
