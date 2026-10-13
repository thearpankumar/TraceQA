"""The WebDriver driver.

It opens real Safari through `safaridriver`, or any browser a W3C WebDriver
server offers, including cloud grids. Like every driver it holds no logic about
what to test: each method does one thing to a browser and reports what happened,
as defined in spec/driver-protocol.schema.json.

WebDriver can do less than Playwright, and says so through its capabilities: it
screenshots only the current viewport.
"""

from __future__ import annotations

import base64
import shutil
import socket
import struct
import subprocess
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from traceqa_protocol import contracts, models
from traceqa_protocol.frames import is_cross_origin
from traceqa_protocol.rpc import Channel, Handler, RpcError
from traceqa_protocol.settle import PAGE_STATE, wait_until_settled

from .wire import WebDriverError, WebDriverSession

NAME = "traceqa-driver-webdriver"
VERSION = "0.1.0"
DEFAULT_TIMEOUT_MS = 30_000
# How long to wait for a page to come to rest: after loading it, and after each scroll.
SETTLE_AFTER_LOAD_S = 5.0
SETTLE_AFTER_SCROLL_S = 1.5
MAX_FRAME_DEPTH = 5

SAFARI = "safari"
REMOTE = "webdriver"

CAPABILITIES = models.Capabilities(
    clip_screenshot=False,
    full_page_screenshot=False,
    eval_all_frames=True,
    cross_origin_frames=True,
    parallel_sessions=False,
    set_viewport=True,
    console_events=False,
    network_events=False,
)

LIST_FRAMES = """
return Array.from(document.querySelectorAll("iframe, frame")).map(function (el) {
  var r = el.getBoundingClientRect();
  return { element: el, name: el.getAttribute("name") || "", x: r.left, y: r.top, w: r.width, h: r.height };
});"""
SCROLL_STATE = "return { y: window.scrollY, height: document.documentElement.scrollHeight, view: window.innerHeight };"


@dataclass
class Session:
    webdriver: WebDriverSession
    server: subprocess.Popen[bytes] | None
    # Frames keep their id across calls as long as the page keeps them in the same place.
    frame_ids: dict[str, str] = field(default_factory=dict)

    def frame_id(self, parent_id: str, index: int) -> str:
        return self.frame_ids.setdefault(f"{parent_id}/{index}", f"f{len(self.frame_ids) + 1}")


def png_size(data: bytes) -> tuple[int, int]:
    return struct.unpack(">II", data[16:24])


def start_safaridriver() -> tuple[subprocess.Popen[bytes], str]:
    """Start Apple's WebDriver server for Safari on a free port and wait until it answers."""
    program = shutil.which("safaridriver")
    if program is None:
        raise RpcError("unsupported", "safaridriver was not found. Safari can only be driven on macOS.")
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    server = subprocess.Popen([program, "-p", str(port)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    url = f"http://127.0.0.1:{port}"
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(f"{url}/status", timeout=1):
                return server, url
        except (urllib.error.URLError, OSError):
            time.sleep(0.1)
    server.kill()
    raise RpcError("internal", "safaridriver did not start within 10 seconds")


def stop_server(server: subprocess.Popen[bytes] | None) -> None:
    """Stop a WebDriver server this driver started, however reluctant it is."""
    if server is None:
        return
    server.terminate()
    try:
        server.wait(timeout=10)
    except subprocess.TimeoutExpired:
        server.kill()
        server.wait()


def fit_viewport(webdriver: WebDriverSession, width: int, height: int) -> tuple[int, int]:
    """Resize the window until the page area has the requested size. Returns the size reached."""

    def measure() -> tuple[int, int]:
        return tuple(webdriver.execute("return [window.innerWidth, window.innerHeight];"))

    inner = measure()
    for _ in range(3):
        if inner == (width, height):
            break
        outer = webdriver.window_rect()
        webdriver.set_window_size(int(outer["width"] + width - inner[0]), int(outer["height"] + height - inner[1]))
        inner = measure()
    return inner


def settle(webdriver: WebDriverSession, limit_s: float, *, animations: bool = True) -> None:
    """Wait until the page has stopped growing and, unless told otherwise, its animations have ended."""
    wait_until_settled(lambda: webdriver.execute(f"return {PAGE_STATE};"), time.sleep, limit_s, animations=animations)


class WebDriverDriver:
    def __init__(self, channel: Channel) -> None:
        self._channel = channel
        self._sessions: dict[str, Session] = {}
        self._opened = 0

    def handlers(self) -> dict[str, Handler]:
        return {
            "driver.hello": self.hello,
            "driver.shutdown": self.shutdown,
            "session.open": self.session_open,
            "session.close": self.session_close,
            "page.goto": self.page_goto,
            "page.eval": self.page_eval,
            "page.scroll": self.page_scroll,
            "page.screenshot": self.page_screenshot,
        }

    def _session(self, session_id: str) -> Session:
        try:
            return self._sessions[session_id]
        except KeyError:
            raise RpcError("session_not_found", f"no session {session_id}") from None

    # ---- driver ----------------------------------------------------------

    def hello(self, params: models.DriverHelloParams) -> models.DriverHelloResult:
        if params.protocol_version != contracts.DRIVER_PROTOCOL_VERSION:
            raise RpcError(
                "version_mismatch",
                f"driver speaks protocol {contracts.DRIVER_PROTOCOL_VERSION}, engine asked for {params.protocol_version}",
            )
        return models.DriverHelloResult(
            driver_name=NAME,
            driver_version=VERSION,
            protocol_version=contracts.DRIVER_PROTOCOL_VERSION,
            browsers=[SAFARI, REMOTE],
            # Safari allows one automated session on the whole machine.
            exclusive_browsers=[SAFARI],
        )

    def shutdown(self, _params: models.Empty) -> models.Empty:
        for session_id in list(self._sessions):
            self._close(session_id)
        return models.Empty()

    # ---- session ---------------------------------------------------------

    def session_open(self, params: models.SessionOpenParams) -> models.SessionOpenResult:
        target = params.target
        server = None
        if target.browser == SAFARI:
            server, url = start_safaridriver()
            wanted: dict[str, Any] = {"browserName": "safari"}
        elif target.browser == REMOTE:
            if not target.webdriver_url:
                raise RpcError("invalid_params", "browser 'webdriver' needs target.webdriverUrl")
            url = target.webdriver_url
            wanted = dict(target.webdriver_capabilities or {})
        else:
            raise RpcError("unsupported", f"this driver cannot open {target.browser!r}")

        try:
            webdriver = WebDriverSession(url, wanted)
        except WebDriverError as error:
            stop_server(server)
            raise RpcError("internal", f"could not open {target.browser}: {error.message}") from error

        self._opened += 1
        session_id = f"s{self._opened}"
        self._sessions[session_id] = Session(webdriver, server)
        try:
            reached = fit_viewport(webdriver, params.viewport.width, params.viewport.height)
        except WebDriverError as error:
            self._close(session_id)
            raise RpcError("internal", f"could not size the window: {error.message}") from error
        if reached != (params.viewport.width, params.viewport.height):
            self._channel.log(
                "warn",
                f"asked for a {params.viewport.width}x{params.viewport.height} viewport, the browser gave {reached[0]}x{reached[1]}",
            )

        name = str(webdriver.capabilities.get("browserName") or target.browser)
        version = str(webdriver.capabilities.get("browserVersion") or "unknown")
        self._channel.log("info", f"opened {name} {version}")
        return models.SessionOpenResult(
            session_id=session_id,
            browser=models.BrowserInfo(name=name, version=version),
            capabilities=CAPABILITIES,
        )

    def session_close(self, params: models.SessionCloseParams) -> models.Empty:
        self._session(params.session_id)
        self._close(params.session_id)
        return models.Empty()

    def _close(self, session_id: str) -> None:
        session = self._sessions.pop(session_id)
        try:
            session.webdriver.quit()
        except WebDriverError as error:
            self._channel.log("warn", f"closing {session_id}: {error.message}")
        finally:
            stop_server(session.server)

    # ---- page ------------------------------------------------------------

    def page_goto(self, params: models.PageGotoParams) -> models.PageGotoResult:
        webdriver = self._session(params.session_id).webdriver
        timeout = params.timeout_ms or DEFAULT_TIMEOUT_MS
        try:
            webdriver.set_timeouts(page_load_ms=timeout, script_ms=timeout)
            webdriver.navigate(params.url)  # returns once the load event has fired
            if params.wait_until == models.WaitUntil.settled:
                settle(webdriver, SETTLE_AFTER_LOAD_S)
            # WebDriver does not expose the HTTP status, so `status` is left out.
            return models.PageGotoResult(
                final_url=webdriver.execute("return location.href;"),
                title=webdriver.execute("return document.title;"),
            )
        except WebDriverError as error:
            if error.code == "timeout":
                raise RpcError("timeout", f"{params.url} did not load in {timeout} ms", retryable=True) from error
            raise RpcError("navigation_failed", error.message, retryable=True) from error

    def page_eval(self, params: models.PageEvalParams) -> models.PageEvalResult:
        session = self._session(params.session_id)
        webdriver = session.webdriver
        # `script` is an expression that evaluates to a function. WebDriver wants a function body.
        body = f"return (\n{params.script}\n)(arguments[0]);"
        try:
            webdriver.switch_to_frame(None)
            if params.timeout_ms:
                webdriver.set_timeouts(page_load_ms=params.timeout_ms, script_ms=params.timeout_ms)
            url = webdriver.execute("return location.href;")
            scroll = webdriver.execute("return [window.scrollX, window.scrollY];")
            value = webdriver.execute(body, params.arg)
        except WebDriverError as error:
            raise RpcError("eval_failed", error.message) from error

        main = models.FrameInfo(id="main", url=url, cross_origin=False)
        results = [models.FrameEvalResult(frame=main, value=value)]
        if params.all_frames:
            try:
                self._eval_child_frames(session, body, params.arg, "main", (scroll[0], scroll[1]), url, 1, results)
                webdriver.switch_to_frame(None)
            except WebDriverError as error:
                raise RpcError("eval_failed", error.message) from error
        return models.PageEvalResult(results=results)

    def _eval_child_frames(
        self,
        session: Session,
        body: str,
        arg: Any,
        parent_id: str,
        parent_at: tuple[float, float],
        page_url: str,
        depth: int,
        results: list[models.FrameEvalResult],
    ) -> None:
        """Run the script in every frame below the current one. `parent_at` is where the
        current frame's viewport starts in top-level page coordinates, and `depth` is how
        many frames deep those child frames are."""
        webdriver = session.webdriver
        for index, found in enumerate(webdriver.execute(LIST_FRAMES)):
            box = models.Rect(x=parent_at[0] + found["x"], y=parent_at[1] + found["y"], w=found["w"], h=found["h"])
            info = models.FrameInfo(
                id=session.frame_id(parent_id, index),
                parent_id=parent_id,
                url="",
                name=found["name"] or None,
                cross_origin=False,
                box=box,
            )
            try:
                webdriver.switch_to_frame(found["element"])
            except WebDriverError as error:
                results.append(models.FrameEvalResult(frame=info, error=error.message))
                continue
            try:
                # One frame's failure is that frame's result. Its own frames are reported by the call below.
                try:
                    info.url = webdriver.execute("return location.href;")
                    info.cross_origin = is_cross_origin(info.url, page_url)
                    value = webdriver.execute(body, arg)
                except WebDriverError as error:
                    results.append(models.FrameEvalResult(frame=info, error=error.message))
                    continue
                results.append(models.FrameEvalResult(frame=info, value=value))
                if depth < MAX_FRAME_DEPTH:
                    self._eval_child_frames(session, body, arg, info.id, (box.x, box.y), page_url, depth + 1, results)
            finally:
                webdriver.switch_to_parent_frame()

    def page_scroll(self, params: models.PageScrollParams) -> models.PageScrollResult:
        webdriver = self._session(params.session_id).webdriver
        if (params.to is None) == (params.by_y is None):
            raise RpcError("invalid_params", "give exactly one of 'to' or 'byY'")
        try:
            webdriver.switch_to_frame(None)
            if params.to == models.ScrollTo.top:
                webdriver.execute("window.scrollTo(0, 0);")
            elif params.to == models.ScrollTo.bottom:
                webdriver.execute("window.scrollTo(0, document.documentElement.scrollHeight);")
            else:
                webdriver.execute("window.scrollBy(0, arguments[0]);", params.by_y)
            settle(webdriver, SETTLE_AFTER_SCROLL_S, animations=params.wait_for != models.ScrollWait.height)
            state = webdriver.execute(SCROLL_STATE)
        except WebDriverError as error:
            raise RpcError("internal", f"scroll failed: {error.message}") from error
        return models.PageScrollResult(
            y=state["y"],
            height=state["height"],
            at_end=state["y"] + state["view"] >= state["height"] - 1,
        )

    def page_screenshot(self, params: models.PageScreenshotParams) -> models.PageScreenshotResult:
        webdriver = self._session(params.session_id).webdriver
        if params.full_page or params.clip is not None:
            raise RpcError("unsupported", "WebDriver can only screenshot the current viewport")
        try:
            data = base64.b64decode(webdriver.screenshot_base64())
        except WebDriverError as error:
            raise RpcError("internal", f"screenshot failed: {error.message}") from error
        path = Path(params.path)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        width, height = png_size(data)
        return models.PageScreenshotResult(path=str(path), width=width, height=height)
