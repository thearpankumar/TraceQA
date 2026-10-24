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
import subprocess
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from traceqa_protocol import BaseDriver, models
from traceqa_protocol.driver import check_scroll_target, installed_version, png_dimensions, scrolled_to_end
from traceqa_protocol.frames import is_cross_origin
from traceqa_protocol.rpc import RpcError
from traceqa_protocol.settle import PAGE_STATE, wait_until_settled
from traceqa_protocol.urls import without_query

from .wire import WebDriverError, WebDriverSession

NAME = "traceqa-driver-webdriver"
VERSION = installed_version(NAME)
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


def give_state(webdriver: WebDriverSession, state: models.StorageState) -> None:
    """Puts cookies and stored values in place. WebDriver only sets them for the site the browser
    is on, so the browser visits each site once, then leaves for a blank page."""
    for origin in state.origins:
        webdriver.navigate(origin.origin)
        for item in origin.local_storage:
            webdriver.execute("localStorage.setItem(arguments[0], arguments[1]);", item.name, item.value)
    by_domain: dict[str, list[models.StorageCookie]] = {}
    for cookie in state.cookies:
        by_domain.setdefault(cookie.domain.lstrip("."), []).append(cookie)
    for domain, cookies in by_domain.items():
        known = [o.origin for o in state.origins if o.origin.split("://", 1)[-1].split(":")[0].endswith(domain)]
        webdriver.navigate(known[0] if known else f"https://{domain}/")
        for cookie in cookies:
            webdriver.add_cookie(webdriver_cookie(cookie))
    if state.origins or state.cookies:
        webdriver.navigate("about:blank")


def webdriver_cookie(cookie: models.StorageCookie) -> dict[str, Any]:
    out: dict[str, Any] = {"name": cookie.name, "value": cookie.value, "path": cookie.path, "domain": cookie.domain}
    if cookie.expires is not None and cookie.expires >= 0:
        out["expiry"] = int(cookie.expires)
    if cookie.secure is not None:
        out["secure"] = cookie.secure
    if cookie.http_only is not None:
        out["httpOnly"] = cookie.http_only
    if cookie.same_site is not None:
        out["sameSite"] = cookie.same_site.value
    return out


def storage_cookie(cookie: dict[str, Any]) -> models.StorageCookie:
    return models.StorageCookie(
        name=cookie["name"],
        value=cookie["value"],
        domain=cookie.get("domain", ""),
        path=cookie.get("path", "/"),
        expires=cookie.get("expiry", -1),
        http_only=cookie.get("httpOnly"),
        secure=cookie.get("secure"),
        same_site=cookie.get("sameSite"),
    )


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


class WebDriverDriver(BaseDriver[Session]):
    NAME = NAME
    VERSION = VERSION
    BROWSERS = (SAFARI, REMOTE)
    # Safari allows one automated session on the whole machine.
    EXCLUSIVE = (SAFARI,)

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

        session_id = self._new_session_id()
        self._sessions[session_id] = Session(webdriver, server)
        try:
            reached = fit_viewport(webdriver, params.viewport.width, params.viewport.height)
        except WebDriverError as error:
            self._close(session_id)
            raise RpcError("internal", f"could not size the window: {error.message}") from error
        # Reported, so the engine records the size the page was really read at.
        unreached = None
        if reached != (params.viewport.width, params.viewport.height):
            unreached = models.Viewport(width=reached[0], height=reached[1])
            self._channel.log(
                "warn",
                f"asked for a {params.viewport.width}x{params.viewport.height} viewport, the browser gave {reached[0]}x{reached[1]}",
            )

        if params.storage_state is not None:
            try:
                give_state(webdriver, params.storage_state)
            except WebDriverError as error:
                self._close(session_id)
                raise RpcError("internal", f"could not put the stored session in place: {error.message}") from error

        name = str(webdriver.capabilities.get("browserName") or target.browser)
        version = str(webdriver.capabilities.get("browserVersion") or "unknown")
        self._channel.log("info", f"opened {name} {version}")
        return models.SessionOpenResult(
            session_id=session_id,
            browser=models.BrowserInfo(name=name, version=version),
            capabilities=CAPABILITIES,
            viewport=unreached,
        )

    def session_storage(self, params: models.SessionStorageParams) -> models.StorageState:
        """The cookies and stored values of the site the browser is on. WebDriver shows no other."""
        webdriver = self._session(params.session_id).webdriver
        try:
            webdriver.switch_to_frame(None)
            origin = webdriver.execute("return location.origin;")
            values = webdriver.execute(
                "var out = []; for (var i = 0; i < localStorage.length; i++) {"
                " var key = localStorage.key(i); out.push({name: key, value: localStorage.getItem(key)}); }"
                " return out;"
            )
            cookies = [storage_cookie(cookie) for cookie in webdriver.cookies()]
        except WebDriverError as error:
            raise RpcError("internal", f"could not read the session's state: {error.message}") from error
        origins = [models.StorageOrigin(origin=origin, local_storage=values)] if origin and origin != "null" else []
        return models.StorageState(cookies=cookies, origins=origins)

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
        load_complete = True
        try:
            webdriver.set_timeouts(page_load_ms=timeout, script_ms=timeout)
            try:
                webdriver.navigate(params.url)  # returns once the load event has fired
            except WebDriverError as error:
                if error.code != "timeout":
                    raise
                # One image or script that never arrives holds the load event back forever. When the
                # content itself has arrived, the page is still there to read.
                if webdriver.execute("return document.readyState;") == "loading":
                    raise
                load_complete = False
            if params.wait_until == models.WaitUntil.settled:
                settle(webdriver, SETTLE_AFTER_LOAD_S)
            # WebDriver does not expose the HTTP status, so `status` is left out.
            return models.PageGotoResult(
                final_url=webdriver.execute("return location.href;"),
                title=webdriver.execute("return document.title;"),
                load_complete=None if load_complete else False,
            )
        except WebDriverError as error:
            if error.code == "timeout":
                raise RpcError(
                    "timeout", f"{without_query(params.url)} did not load in {timeout} ms", retryable=True
                ) from error
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
        check_scroll_target(params)
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
            at_end=scrolled_to_end(state["y"], state["view"], state["height"]),
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
        width, height = png_dimensions(data)
        return models.PageScreenshotResult(path=str(path), width=width, height=height)
