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

# How long to wait for the element and for the action when the request names no time.
DEFAULT_TIMEOUT_MS = 10_000
# How long to wait for the page to come to rest after an action.
SETTLE_LIMIT_S = 1.5
# How long to wait for a new page's content, and for a started navigation to load.
LOAD_WAIT_MS = 5_000
POPUP_WAIT_MS = 2_000

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

# Counts the changes a page makes to itself. Installed once per frame, read before and after an action.
# A mutation observer does not see inside shadow roots, so every open one that exists when it is installed
# is observed too; a shadow root created later, and a closed one, are not. A page that replaced
# MutationObserver is not defended against: the answer is then simply "no change".
MUTATIONS_JS = """() => {
  if (!window.__traceqa_mutations__) {
    var counter = { n: 0 };
    var options = { subtree: true, childList: true, attributes: true, characterData: true };
    try {
      var observer = new MutationObserver(function (records) { counter.n += records.length; });
      var watch = function (root) {
        observer.observe(root, options);
        var all = root.querySelectorAll("*");
        for (var i = 0; i < all.length; i++) if (all[i].shadowRoot) watch(all[i].shadowRoot);
      };
      watch(document);
    } catch (e) { return -1; }
    Object.defineProperty(window, "__traceqa_mutations__", { value: counter, configurable: true });
  }
  return window.__traceqa_mutations__.n;
}"""
READ_MUTATIONS_JS = "() => (window.__traceqa_mutations__ ? window.__traceqa_mutations__.n : null)"


@dataclass
class Recorder:
    """What the browser did while one action ran."""

    navigated_status: int | None = None
    navigated: bool = False
    popups: list[Page] = field(default_factory=list)
    dialogs: list[models.ActDialog] = field(default_factory=list)
    downloads: list[models.ActDownload] = field(default_factory=list)
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
            if request.is_navigation_request() and request.frame == self.page.main_frame and not attachment:
                recorder.navigated = True
                recorder.navigated_status = response.status
        if response.status >= 400:
            self._note_failed(request, status=response.status, failure=None)

    def _popup(self, page: Page) -> None:
        recorder = self.recorder
        if recorder is not None and page != self.page:
            recorder.popups.append(page)


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


def _mutations(page: Page) -> dict[Frame, int | None]:
    counts: dict[Frame, int | None] = {}
    for frame in page.frames:
        try:
            counts[frame] = frame.evaluate(MUTATIONS_JS)
        except PlaywrightError:
            counts[frame] = None
    return counts


def _dom_changed(page: Page, before: dict[Frame, int | None], navigated: bool) -> bool:
    if navigated:
        return True
    for frame, was in before.items():
        if was is None or was < 0 or frame.is_detached():
            continue
        try:
            now = frame.evaluate(READ_MUTATIONS_JS)
        except PlaywrightError:
            # The frame's page went away or was replaced under the action.
            return True
        if now is None or now != was:
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


def act(page: Page, watch: Watch, frame: Frame, params: models.PageActParams) -> models.PageActResult:
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
        before = _mutations(page)
        watch.recorder = recorder
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
        settle = _settle(page)
        popups = []
        for popup in recorder.popups:
            with contextlib.suppress(PlaywrightError):
                popup.wait_for_load_state("domcontentloaded", timeout=POPUP_WAIT_MS)
            popups.append(without_query(popup.url))
            with contextlib.suppress(PlaywrightError):
                popup.close()
        url = without_query(page.url)
        moved = recorder.navigated or without_query(before_url) != url
        return models.PageActResult(
            ok=True,
            url=url,
            navigated=models.ActNavigation(status=recorder.navigated_status) if recorder.navigated else None,
            popups=popups,
            dialogs=recorder.dialogs,
            downloads=recorder.downloads,
            console_errors=recorder.console_errors,
            failed_requests=recorder.failed,
            dom_changed=_dom_changed(page, before, moved),
            settle=settle,
        )
    finally:
        watch.recorder = None
