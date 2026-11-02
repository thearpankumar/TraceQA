"""`page.act`: one action on one element, and a report of what it did.

The driver decides nothing here. It performs what it is told, watches what the browser does while it
happens, and reports it. Nothing in this file runs during component extraction: the watchers are
installed by the first `page.act` of a session, so a session that only reads pages behaves as before.
"""

from __future__ import annotations

import contextlib
import time
from dataclasses import dataclass, field

from playwright.sync_api import BrowserContext, Dialog, Download, Frame, Locator, Page, Request, Response
from playwright.sync_api import Error as PlaywrightError
from playwright.sync_api import TimeoutError as PlaywrightTimeout
from traceqa_protocol import models
from traceqa_protocol.rpc import RpcError
from traceqa_protocol.settle import PAGE_STATE, wait_until_settled
from traceqa_protocol.urls import redact_text, without_query

from .safety import BLOCKED, Guard, origin_of

# How long to wait for the element and for the action when the request names no time.
DEFAULT_TIMEOUT_MS = 10_000
# How long to wait for the page to come to rest after an action.
SETTLE_LIMIT_S = 1.5
# How long to wait for a new page's content, and for a started navigation to load.
LOAD_WAIT_MS = 5_000
# How long to wait for the element to be scrolled into view, before the action does its own waiting.
SCROLL_WAIT_MS = 2_000
POPUP_WAIT_MS = 2_000
# How long to wait for the browser's `download` event once a response that is a download has arrived. On a slow machine
# (WebKit on a CI runner) the event can come after the page has settled.
DOWNLOAD_WAIT_S = 3.0
DOWNLOAD_POLL_MS = 50

MAX_FAILED_REQUESTS = 50
MAX_ITEMS = 50
TEXT_LIMIT = 500

# What a driver with these watchers tells the engine it reports. See Capabilities.eventKinds.
EVENT_KINDS = ["navigation", "popups", "dialogs", "downloads", "console", "network", "dom"]
ACTION_KINDS = [
    models.ActionKind.click,
    models.ActionKind.hover,
    models.ActionKind.type,
    models.ActionKind.select,
    models.ActionKind.press,
]

# A navigation, a redirect or a replaced frame cancels its requests. That is normal, not a failure.
CANCELLED = ("ERR_ABORTED", "NS_BINDING_ABORTED", "aborted", "cancelled")

# Counts the changes a page makes, by kind: elements added or removed, text, attributes (any but `style`), and `style`
# changes that show or hide something (`display` or `visibility`). A style change that only moves or fades an element
# (a transform, an opacity) is not counted: pages restyle themselves that way in answer to any mouse move or scroll
# (parallax, a cursor that is followed), so it says nothing about what an action did. Installed once per frame and read
# before and after an action. A mutation observer does not see inside shadow roots, so every open one that exists when
# it is installed is observed too; a shadow root created later, and a closed one, are not. A page that replaced
# MutationObserver is not defended against: the answer is then simply "no change".
MUTATIONS_JS = """() => {
  if (!window.__traceqa_mutations__) {
    var counts = { elements: 0, text: 0, attributes: 0, style: 0 };
    var options = { subtree: true, childList: true, attributes: true, attributeOldValue: true, characterData: true };
    var hasElement = function (nodes) {
      for (var i = 0; i < nodes.length; i++) if (nodes[i].nodeType === 1) return true;
      return false;
    };
    var property = function (style, name) {
      var parts = (style || "").split(";");
      for (var i = 0; i < parts.length; i++) {
        var colon = parts[i].indexOf(":");
        if (colon > 0 && parts[i].slice(0, colon).trim().toLowerCase() === name) {
          return parts[i].slice(colon + 1).trim().toLowerCase();
        }
      }
      return "";
    };
    var shows = function (style) { return property(style, "display") + "|" + property(style, "visibility"); };
    try {
      var observer = new MutationObserver(function (records) {
        for (var i = 0; i < records.length; i++) {
          var record = records[i];
          if (record.type === "childList") {
            if (hasElement(record.addedNodes) || hasElement(record.removedNodes)) counts.elements++;
            else counts.text++;
          } else if (record.type === "characterData") counts.text++;
          else if (record.attributeName === "style") {
            if (shows(record.oldValue) !== shows(record.target.getAttribute("style"))) counts.style++;
          } else counts.attributes++;
        }
      });
      var watch = function (root) {
        observer.observe(root, options);
        var all = root.querySelectorAll("*");
        for (var i = 0; i < all.length; i++) if (all[i].shadowRoot) watch(all[i].shadowRoot);
      };
      watch(document);
    } catch (e) { return null; }
    Object.defineProperty(window, "__traceqa_mutations__", { value: counts, configurable: true });
  }
  var now = window.__traceqa_mutations__;
  return { elements: now.elements, text: now.text, attributes: now.attributes, style: now.style };
}"""
KINDS = ("elements", "text", "attributes", "style")
# How long the page is watched at rest before an action, to learn how much it changes by itself. A page that animates,
# runs a ticker or plays a video changes all the time, and "it changed" then says nothing about the action.
AT_REST_S = 0.3
# An action changed the page when some kind of change happened more than this many times as often as at rest.
NOISE_FACTOR = 3

Counts = dict[str, int]
Rates = dict[str, float]


@dataclass
class Recorder:
    """What the browser did while one action ran."""

    navigated_status: int | None = None
    navigated: bool = False
    popups: list[Page] = field(default_factory=list)
    dialogs: list[models.ActDialog] = field(default_factory=list)
    downloads: list[models.ActDownload] = field(default_factory=list)
    # Responses to navigations (in any tab) that are downloads. Each is followed by a `download` event, which can be late.
    attachments: int = 0
    console_errors: list[models.ActConsoleError] = field(default_factory=list)
    failed: list[models.ActFailedRequest] = field(default_factory=list)
    failed_seen: set[Request] = field(default_factory=set)


class Watch:
    """Listeners on one page and its context. They record only while `recorder` is set."""

    def __init__(self, page: Page, context: BrowserContext) -> None:
        self.page = page
        self.recorder: Recorder | None = None
        page.on("dialog", self._dialog)
        page.on("download", self._download)
        page.on("console", self._console)
        page.on("pageerror", self._page_error)
        page.on("requestfailed", self._request_failed)
        page.on("response", self._response)
        context.on("page", self._popup)
        context.on("response", self._count_attachment)

    def _dialog(self, dialog: Dialog) -> None:
        # A dialog is always dismissed, as Playwright does when nobody listens.
        recorder = self.recorder
        handled = True
        try:
            dialog.dismiss()
        except PlaywrightError:
            handled = False
        if recorder is not None and len(recorder.dialogs) < MAX_ITEMS:
            recorder.dialogs.append(
                models.ActDialog(type=dialog.type, message=redact_text(dialog.message, TEXT_LIMIT), handled=handled)
            )

    def _download(self, download: Download) -> None:
        # The file is never stored.
        recorder = self.recorder
        with contextlib.suppress(PlaywrightError):
            download.cancel()
        if recorder is not None and len(recorder.downloads) < MAX_ITEMS:
            recorder.downloads.append(
                models.ActDownload(url=without_query(download.url), suggested_name=download.suggested_filename or None)
            )

    def _console(self, message: object) -> None:
        recorder = self.recorder
        if recorder is None or len(recorder.console_errors) >= MAX_ITEMS:
            return
        if getattr(message, "type", "") == "error":
            recorder.console_errors.append(
                models.ActConsoleError(
                    text=redact_text(str(getattr(message, "text", "")), TEXT_LIMIT), source="console"
                )
            )

    def _page_error(self, error: object) -> None:
        recorder = self.recorder
        if recorder is not None and len(recorder.console_errors) < MAX_ITEMS:
            recorder.console_errors.append(
                models.ActConsoleError(text=redact_text(str(error), TEXT_LIMIT), source="uncaught")
            )

    def _note_failed(self, request: Request, *, status: int | None, failure: str | None) -> None:
        recorder = self.recorder
        if recorder is None or request in recorder.failed_seen or len(recorder.failed) >= MAX_FAILED_REQUESTS:
            return
        recorder.failed_seen.add(request)
        recorder.failed.append(
            models.ActFailedRequest(
                url=without_query(request.url), method=request.method, status=status, failure=failure
            )
        )

    def note_blocked(self, address: str, method: str, request: Request | None) -> None:
        """The session's policy aborted a request. Said as `blocked_by_policy`, and the browser's own report of the
        same request, which follows, is not counted a second time."""
        recorder = self.recorder
        if recorder is None or len(recorder.failed) >= MAX_FAILED_REQUESTS:
            return
        if request is not None:
            recorder.failed_seen.add(request)
        recorder.failed.append(models.ActFailedRequest(url=without_query(address), method=method, failure=BLOCKED))

    def _request_failed(self, request: Request) -> None:
        reason = request.failure or ""
        if not any(mark in reason for mark in CANCELLED):
            self._note_failed(request, status=None, failure=reason or "failed")

    def _response(self, response: Response) -> None:
        recorder = self.recorder
        if recorder is None:
            return
        request = response.request
        with contextlib.suppress(PlaywrightError):
            # A response that is a download replaces nothing: the page stays where it was.
            attachment = response.headers.get("content-disposition", "").lower().startswith("attachment")
            # A navigation the policy answered with 204 is a request that was stopped, not a page that was loaded.
            stopped = request in recorder.failed_seen
            if (
                request.is_navigation_request()
                and request.frame == self.page.main_frame
                and not (attachment or stopped)
            ):
                recorder.navigated = True
                recorder.navigated_status = response.status
        if response.status >= 400:
            self._note_failed(request, status=response.status, failure=None)

    def _count_attachment(self, response: Response) -> None:
        recorder = self.recorder
        if recorder is None:
            return
        with contextlib.suppress(PlaywrightError):
            attachment = response.headers.get("content-disposition", "").lower().startswith("attachment")
            if attachment and response.request.is_navigation_request():
                recorder.attachments += 1

    def _popup(self, page: Page) -> None:
        recorder = self.recorder
        if recorder is not None and page != self.page:
            recorder.popups.append(page)
            # A new tab that turns into a download: some browsers report the download on the tab, not the opener.
            page.on("download", self._download)


def _await_downloads(page: Page, recorder: Recorder) -> None:
    """Gives the `download` event of every response that was a download the time to arrive."""
    deadline = time.monotonic() + DOWNLOAD_WAIT_S
    while len(recorder.downloads) < recorder.attachments and time.monotonic() < deadline:
        with contextlib.suppress(PlaywrightError):
            page.wait_for_timeout(DOWNLOAD_POLL_MS)


def _locator(frame: Frame, selector: str) -> Locator:
    # The collector writes a selector inside shadow DOM as "host >>> inner". Playwright chains with ">>".
    return frame.locator(selector.replace(" >>> ", " >> "))


def _first_line(message: str) -> str:
    stripped = message.strip()
    return stripped.splitlines()[0] if stripped else message


def _need_value(params: models.PageActParams) -> str:
    if params.value is None:
        raise RpcError("invalid_params", f"action {params.action.value!r} needs a value")
    return params.value


def _mutations(page: Page) -> dict[Frame, Counts | None]:
    """How many changes of each kind every frame has had since the observer was installed (it is, on the first call)."""
    counts: dict[Frame, Counts | None] = {}
    for frame in page.frames:
        try:
            counts[frame] = frame.evaluate(MUTATIONS_JS)
        except PlaywrightError:
            counts[frame] = None
    return counts


def _at_rest(page: Page) -> tuple[dict[Frame, Counts | None], dict[Frame, Rates]]:
    """Watches the page for a moment, and returns where the counts stand and how fast each kind of change goes by
    itself, per second, in each frame."""
    start = _mutations(page)
    page.wait_for_timeout(AT_REST_S * 1000)
    later = _mutations(page)
    rates: dict[Frame, Rates] = {}
    for frame, was in start.items():
        now = later.get(frame)
        if was is not None and now is not None:
            rates[frame] = {kind: max(0, now[kind] - was[kind]) / AT_REST_S for kind in KINDS}
    return later, rates


def _dom_changed(
    before: dict[Frame, Counts | None],
    rates: dict[Frame, Rates],
    window_s: float,
    navigated: bool,
) -> bool:
    """Whether an action changed the page: some kind of change happened in some frame more than NOISE_FACTOR times as
    often as the page changes by itself at rest, over the `window_s` the action took. A page at rest makes none, so for
    it any change counts. Kinds are judged one by one, so a page that restyles itself all the time still shows an
    element that appears. What this cannot see: a change of a kind the page also makes by itself, that is no bigger than
    its noise (a label changing on a page with a ticking clock)."""
    if navigated:
        return True
    for frame, was in before.items():
        if was is None or frame.is_detached():
            continue
        try:
            now = frame.evaluate(MUTATIONS_JS)
        except PlaywrightError:
            # The frame's page went away or was replaced under the action.
            return True
        if now is None:
            return True
        quiet = rates.get(frame, dict.fromkeys(KINDS, 0.0))
        for kind in KINDS:
            happened = now[kind] - was[kind]
            if happened > 0 and happened > NOISE_FACTOR * quiet[kind] * window_s:
                return True
    return False


def _settle(page: Page) -> models.ActSettle:
    started = time.monotonic()
    quiet = False
    for attempt in range(2):
        try:
            quiet = wait_until_settled(
                lambda: page.evaluate(PAGE_STATE), lambda seconds: page.wait_for_timeout(seconds * 1000), SETTLE_LIMIT_S
            )
            break
        except PlaywrightError:
            # The page is being replaced: wait for the new one, once.
            if attempt:
                break
            with contextlib.suppress(PlaywrightError):
                page.wait_for_load_state("domcontentloaded", timeout=LOAD_WAIT_MS)
    return models.ActSettle(waited_ms=int((time.monotonic() - started) * 1000), quiet=quiet)


def _left_the_allowed_origins(page: Page, watch: Watch, guard: Guard, before_url: str) -> bool:
    """Puts the page back when an action took it somewhere the policy does not allow. Returns True when it did.

    A navigation to such a place is stopped before it is made. Two cases get past that. A redirect: the browser
    follows it without asking Playwright, so the request to the other site has already been made when this notices
    (the same limit as the operator's address rules). And a navigation that was stopped: Chromium then shows an error
    page instead of the page, where Firefox and WebKit stay on it. Either way the way back is the browser's history,
    and only when the page really moved: with nothing to go back to, in WebKit, going back lands on a blank page."""
    landed = page.url
    if landed == before_url or guard.allows_page(landed):
        return False
    if origin_of(landed) is not None:
        # A real address of another site, so nothing stopped it: a redirect. An error page has no origin, and what led
        # to it was reported by the guard when it stopped it.
        watch.note_blocked(landed, "GET", None)
    with contextlib.suppress(PlaywrightError):
        page.go_back(wait_until="load", timeout=LOAD_WAIT_MS)
    return True


def _scroll_y(page: Page) -> float | None:
    try:
        return page.evaluate("() => window.scrollY")
    except PlaywrightError:
        return None


def _bring_into_view(page: Page, locator: Locator, timeout_ms: int) -> None:
    """Scrolls the element into view before anything is measured, and lets the page finish reacting to the scroll.

    A click scrolls the element into view first, and many pages answer a scroll (an image sequence, classes on the
    page, parallax). If that happened inside the measured time, the action would be credited with it. Nothing is
    waited for when the page did not move, and a failure here is left for the action itself to report, in its own words."""
    before = _scroll_y(page)
    with contextlib.suppress(PlaywrightError):
        locator.scroll_into_view_if_needed(timeout=min(timeout_ms, SCROLL_WAIT_MS))
    if _scroll_y(page) != before:
        _settle(page)


def _perform(locator: Locator, params: models.PageActParams, timeout_ms: int) -> None:
    action = params.action
    if action == models.ActionKind.click:
        locator.click(timeout=timeout_ms)
    elif action == models.ActionKind.hover:
        locator.hover(timeout=timeout_ms)
    elif action == models.ActionKind.type:
        locator.fill(_need_value(params), timeout=timeout_ms)
    elif action == models.ActionKind.select:
        # A string matches an option by its value or by its label.
        locator.select_option(_need_value(params), timeout=timeout_ms)
    else:
        locator.press(_need_value(params), timeout=timeout_ms)


def act(page: Page, watch: Watch, guard: Guard, frame: Frame, params: models.PageActParams) -> models.PageActResult:
    """Performs the action in `frame` and reports what happened. Raises `RpcError` when it cannot."""
    timeout_ms = params.timeout_ms or DEFAULT_TIMEOUT_MS
    if params.value is None and params.action in (
        models.ActionKind.type,
        models.ActionKind.select,
        models.ActionKind.press,
    ):
        _need_value(params)
    locator = _locator(frame, params.target.selector)
    recorder = Recorder()
    before_url = page.url
    try:
        try:
            locator.wait_for(state="attached", timeout=timeout_ms)
        except PlaywrightTimeout as error:
            raise RpcError(
                "element_not_found", f"no element matches {params.target.selector!r} within {timeout_ms} ms"
            ) from error
        _bring_into_view(page, locator, timeout_ms)
        before, rates = _at_rest(page)
        watch.recorder = recorder
        guard.take_unowned()  # what was stopped before this action is not this action's
        started = time.monotonic()
        _perform(locator, params, timeout_ms)
    except PlaywrightTimeout as error:
        watch.recorder = None
        raise RpcError("timeout", f"{params.action.value} did not finish: {_first_line(error.message)}") from error
    except PlaywrightError as error:
        watch.recorder = None
        message = _first_line(error.message)
        kind = "invalid_params" if "strict mode violation" in error.message else "internal"
        raise RpcError(kind, f"{params.action.value} failed: {message}") from error
    except RpcError:
        watch.recorder = None
        raise

    try:
        # A navigation the action started is given time to load before the page is judged at rest.
        if recorder.navigated:
            with contextlib.suppress(PlaywrightError):
                page.wait_for_load_state("load", timeout=LOAD_WAIT_MS)
        guard.drain_web_sockets()
        settle = _settle(page)
        _await_downloads(page, recorder)
        taken_back = _left_the_allowed_origins(page, watch, guard, before_url)
        if taken_back:
            # What came back is the page that was there, loaded again: nothing is different, and the counters of the
            # page that was lost with it say nothing.
            recorder.navigated = False
            recorder.navigated_status = None
            settle = _settle(page)
        popups = []
        stopped_tabs = guard.take_unowned()
        for popup in recorder.popups:
            with contextlib.suppress(PlaywrightError):
                popup.wait_for_load_state("domcontentloaded", timeout=POPUP_WAIT_MS)
            # A popup that was stopped shows an error page, and one that became a download has no address: say where
            # the first was going (the stops the guard could not tie to a page, in the order they happened, match the
            # pages that show an error), and call the second what it is.
            address = without_query(popup.url)
            if origin_of(popup.url) is None and stopped_tabs:
                address = without_query(stopped_tabs.pop(0))
            popups.append(address or "about:blank")
            with contextlib.suppress(PlaywrightError):
                popup.close()
        url = without_query(page.url)
        moved = recorder.navigated or without_query(before_url) != url
        return models.PageActResult(
            ok=True,
            url=url,
            request_counts=guard.counts(),
            navigated=models.ActNavigation(status=recorder.navigated_status) if recorder.navigated else None,
            popups=popups,
            dialogs=recorder.dialogs,
            downloads=recorder.downloads,
            console_errors=recorder.console_errors,
            failed_requests=recorder.failed,
            dom_changed=False if taken_back else _dom_changed(before, rates, time.monotonic() - started, moved),
            settle=settle,
        )
    finally:
        watch.recorder = None
