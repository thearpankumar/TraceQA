"""The Playwright driver.

It holds no logic about what to test. Each method does one thing to a browser
and reports what happened, as defined in spec/driver-protocol.schema.json.
"""

from __future__ import annotations

import contextlib
import os
import struct
import time
from dataclasses import dataclass, field
from pathlib import Path

from playwright.sync_api import Browser, BrowserContext, Frame, Page, Playwright, sync_playwright
from playwright.sync_api import Error as PlaywrightError
from playwright.sync_api import TimeoutError as PlaywrightTimeout
from traceqa_protocol import contracts, models
from traceqa_protocol.frames import is_cross_origin
from traceqa_protocol.rpc import Channel, Handler, RpcError
from traceqa_protocol.settle import PAGE_STATE, wait_until_settled

NAME = "traceqa-driver-playwright"
VERSION = "0.1.0"
DEFAULT_TIMEOUT_MS = 30_000
# How long to wait for a page to come to rest: after loading it, and after each scroll.
SETTLE_AFTER_LOAD_S = 5.0
SETTLE_AFTER_SCROLL_S = 1.5

# Browser name -> (Playwright engine, channel). "cdp" attaches to a running browser instead.
BROWSERS: dict[str, tuple[str, str | None]] = {
    "chromium": ("chromium", None),
    "chrome": ("chromium", "chrome"),
    "msedge": ("chromium", "msedge"),
    "firefox": ("firefox", None),
    "webkit": ("webkit", None),
}
CDP = "cdp"

CAPABILITIES = models.Capabilities(
    clip_screenshot=True,
    full_page_screenshot=True,
    eval_all_frames=True,
    cross_origin_frames=True,
    parallel_sessions=True,
    set_viewport=True,
    console_events=False,
    network_events=False,
)

SCROLL_STATE = "() => ({ y: window.scrollY, height: document.documentElement.scrollHeight, view: window.innerHeight })"


@dataclass
class Session:
    browser: Browser
    context: BrowserContext
    page: Page
    owns_context: bool
    # Only Chromium lets a driver see closed shadow roots, through its own protocol.
    chromium: bool = False
    frame_ids: dict[Frame, str] = field(default_factory=dict)

    def frame_id(self, frame: Frame) -> str:
        if frame == self.page.main_frame:
            return "main"
        if frame not in self.frame_ids:
            self.frame_ids[frame] = f"f{len(self.frame_ids) + 1}"
        return self.frame_ids[frame]


def first_line(message: str) -> str:
    """Playwright appends a multi-line call log to its errors. People only need the first line."""
    return message.strip().splitlines()[0] if message.strip() else message


def count_closed_shadow_roots(page: Page) -> int | None:
    """Closed shadow roots on the page, which no script can read inside. None when it cannot be told."""
    try:
        cdp = page.context.new_cdp_session(page)
    except PlaywrightError:
        return None
    try:
        document = cdp.send("DOM.getDocument", {"depth": -1, "pierce": True})
    except PlaywrightError:
        return None
    finally:
        with contextlib.suppress(PlaywrightError):
            cdp.detach()
    closed = 0
    pending = [document["root"]]
    while pending:
        node = pending.pop()
        closed += node.get("shadowRootType") == "closed"
        pending.extend(node.get("children", []))
        pending.extend(node.get("shadowRoots", []))
        if "contentDocument" in node:
            pending.append(node["contentDocument"])
    return closed


def png_size(path: Path) -> tuple[int, int]:
    with path.open("rb") as file:
        header = file.read(24)
    return struct.unpack(">II", header[16:24])


def settle(page: Page, limit_s: float, *, animations: bool = True) -> None:
    """Wait until the page has stopped growing and, unless told otherwise, its animations have ended."""
    wait_until_settled(
        lambda: page.evaluate(PAGE_STATE),
        lambda seconds: page.wait_for_timeout(seconds * 1000),
        limit_s,
        animations=animations,
    )


class PlaywrightDriver:
    def __init__(self, channel: Channel) -> None:
        self._channel = channel
        self._playwright: Playwright | None = None
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
            browsers=[*BROWSERS, CDP],
        )

    def shutdown(self, _params: models.Empty) -> models.Empty:
        for session_id in list(self._sessions):
            self._close(session_id)
        if self._playwright is not None:
            self._playwright.stop()
            self._playwright = None
        return models.Empty()

    # ---- session ---------------------------------------------------------

    def session_open(self, params: models.SessionOpenParams) -> models.SessionOpenResult:
        if self._playwright is None:
            # Playwright waits for web fonts before a screenshot. A font request that never
            # finishes then blocks it for 30 seconds, and a late font is no reason for that.
            os.environ.setdefault("PW_TEST_SCREENSHOT_NO_FONTS_READY", "1")
            self._playwright = sync_playwright().start()
        target = params.target
        viewport = {"width": params.viewport.width, "height": params.viewport.height}
        browser = None
        try:
            if target.browser == CDP:
                if not target.cdp_endpoint:
                    raise RpcError("invalid_params", "browser 'cdp' needs target.cdpEndpoint")
                browser = self._playwright.chromium.connect_over_cdp(target.cdp_endpoint)
            elif target.browser in BROWSERS:
                engine, channel = BROWSERS[target.browser]
                # The engine closes the browser when a run is cancelled, not Playwright on Ctrl-C.
                browser = getattr(self._playwright, engine).launch(
                    headless=target.headless is not False, channel=channel, handle_sigint=False
                )
            else:
                raise RpcError("unsupported", f"this driver cannot open {target.browser!r}")
            try:
                context = browser.new_context(viewport=viewport, locale=params.locale)
                owns_context = True
            except PlaywrightError:
                # Some remote browsers only expose the context they started with.
                if not browser.contexts:
                    raise
                context = browser.contexts[0]
                owns_context = False
            page = context.new_page()
            if not owns_context:
                page.set_viewport_size(viewport)
        except PlaywrightError as error:
            # A browser that started but could not be set up must not be left running.
            if browser is not None:
                with contextlib.suppress(PlaywrightError):
                    browser.close()
            raise RpcError("internal", f"could not open {target.browser}: {first_line(error.message)}") from error

        self._opened += 1
        session_id = f"s{self._opened}"
        chromium = target.browser == CDP or BROWSERS.get(target.browser, ("",))[0] == "chromium"
        self._sessions[session_id] = Session(browser, context, page, owns_context, chromium)
        self._channel.log("info", f"opened {target.browser} {browser.version}")
        return models.SessionOpenResult(
            session_id=session_id,
            browser=models.BrowserInfo(name=target.browser, version=browser.version),
            capabilities=CAPABILITIES,
        )

    def session_close(self, params: models.SessionCloseParams) -> models.Empty:
        self._session(params.session_id)
        self._close(params.session_id)
        return models.Empty()

    def _close(self, session_id: str) -> None:
        session = self._sessions.pop(session_id)
        closers = [session.page.close, session.browser.close]
        if session.owns_context:
            closers.insert(1, session.context.close)
        # Each step is tried on its own, so one failure does not leave the browser open.
        for close in closers:
            try:
                close()
            except PlaywrightError as error:
                self._channel.log("warn", f"closing {session_id}: {error.message}")

    # ---- page ------------------------------------------------------------

    def page_goto(self, params: models.PageGotoParams) -> models.PageGotoResult:
        session = self._session(params.session_id)
        page = session.page
        timeout = params.timeout_ms or DEFAULT_TIMEOUT_MS
        started = time.monotonic()
        try:
            # The content first. Without it there is nothing to read, so this is the one that can fail.
            response = page.goto(params.url, wait_until="domcontentloaded", timeout=timeout)
        except PlaywrightTimeout as error:
            raise RpcError("timeout", f"{params.url} did not load in {timeout} ms", retryable=True) from error
        except PlaywrightError as error:
            raise RpcError("navigation_failed", first_line(error.message), retryable=True) from error

        # Then the load event, in the time that is left. One image or script that never arrives
        # holds it back forever, and the page is still there to read.
        load_complete = True
        left = max(1, timeout - int((time.monotonic() - started) * 1000))
        try:
            page.wait_for_load_state("load", timeout=left)
        except PlaywrightTimeout:
            load_complete = False
        try:
            if params.wait_until == models.WaitUntil.settled:
                if load_complete:
                    # A page that never goes quiet is still usable.
                    with contextlib.suppress(PlaywrightTimeout):
                        page.wait_for_load_state("networkidle", timeout=3_000)
                settle(page, SETTLE_AFTER_LOAD_S)
            title = page.title()
        except PlaywrightError as error:
            raise RpcError("navigation_failed", first_line(error.message), retryable=True) from error
        return models.PageGotoResult(
            final_url=page.url,
            status=response.status if response else None,
            title=title,
            load_complete=None if load_complete else False,
            closed_shadow_roots=count_closed_shadow_roots(page) if session.chromium else None,
        )

    def page_eval(self, params: models.PageEvalParams) -> models.PageEvalResult:
        session = self._session(params.session_id)
        page = session.page
        frames = page.frames if params.all_frames else [page.main_frame]
        scroll = page.evaluate("() => [window.scrollX, window.scrollY]")
        results = []
        for frame in frames:
            if frame.is_detached():
                continue
            info = models.FrameInfo(id=session.frame_id(frame), url=frame.url, cross_origin=False)
            if frame != page.main_frame:
                info.parent_id = session.frame_id(frame.parent_frame) if frame.parent_frame else None
                info.name = frame.name or None
                info.cross_origin = is_cross_origin(frame.url, page.url)
                try:
                    box = frame.frame_element().bounding_box()
                except PlaywrightError:
                    box = None
                if box:
                    # bounding_box is relative to the viewport. The spec wants page coordinates.
                    info.box = models.Rect(
                        x=box["x"] + scroll[0], y=box["y"] + scroll[1], w=box["width"], h=box["height"]
                    )
            try:
                value = frame.evaluate(params.script, params.arg)
                results.append(models.FrameEvalResult(frame=info, value=value))
            except PlaywrightError as error:
                if frame == page.main_frame:
                    raise RpcError("eval_failed", error.message) from error
                results.append(models.FrameEvalResult(frame=info, error=error.message))
        return models.PageEvalResult(results=results)

    def page_scroll(self, params: models.PageScrollParams) -> models.PageScrollResult:
        page = self._session(params.session_id).page
        if (params.to is None) == (params.by_y is None):
            raise RpcError("invalid_params", "give exactly one of 'to' or 'byY'")
        if params.to == models.ScrollTo.top:
            page.evaluate("window.scrollTo(0, 0)")
        elif params.to == models.ScrollTo.bottom:
            page.evaluate("window.scrollTo(0, document.documentElement.scrollHeight)")
        else:
            page.evaluate("(dy) => window.scrollBy(0, dy)", params.by_y)
        settle(page, SETTLE_AFTER_SCROLL_S, animations=params.wait_for != models.ScrollWait.height)
        state = page.evaluate(SCROLL_STATE)
        return models.PageScrollResult(
            y=state["y"],
            height=state["height"],
            at_end=state["y"] + state["view"] >= state["height"] - 1,
        )

    def page_screenshot(self, params: models.PageScreenshotParams) -> models.PageScreenshotResult:
        page = self._session(params.session_id).page
        path = Path(params.path)
        path.parent.mkdir(parents=True, exist_ok=True)
        clip = None
        if params.clip is not None:
            clip = {"x": params.clip.x, "y": params.clip.y, "width": params.clip.w, "height": params.clip.h}
        try:
            # With a clip, full_page makes Playwright read the clip in page coordinates.
            page.screenshot(path=str(path), full_page=bool(params.full_page) or clip is not None, clip=clip)
        except PlaywrightError as error:
            raise RpcError("internal", f"screenshot failed: {first_line(error.message)}") from error
        width, height = png_size(path)
        return models.PageScreenshotResult(path=str(path), width=width, height=height)
