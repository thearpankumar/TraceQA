"""The Playwright driver.

It holds no logic about what to test. Each method does one thing to a browser
and reports what happened, as defined in spec/driver-protocol.schema.json.
"""

from __future__ import annotations

import contextlib
import json
import os
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, cast

from playwright.sync_api import (
    Browser,
    BrowserContext,
    FloatRect,
    Frame,
    Page,
    Playwright,
    Request,
    Response,
    ViewportSize,
    sync_playwright,
)
from playwright.sync_api import Error as PlaywrightError
from playwright.sync_api import StorageState as PlaywrightStorageState
from playwright.sync_api import TimeoutError as PlaywrightTimeout
from traceqa_protocol import BaseDriver, models, to_wire
from traceqa_protocol.driver import check_scroll_target, installed_version, png_dimensions, scrolled_to_end
from traceqa_protocol.frames import is_cross_origin
from traceqa_protocol.rpc import Channel, RpcError
from traceqa_protocol.settle import PAGE_STATE, wait_until_settled
from traceqa_protocol.urls import without_query

NAME = "traceqa-driver-playwright"
VERSION = installed_version(NAME)
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
    failed_requests=True,
    # frame.evaluate always waits for a returned Promise, so params.await_promise needs no code here.
    eval_async=True,
)

# Requests that change the list of components when they fail. Pictures, fonts and media do not, and a
# page loads dozens of them, so counting them would only add noise.
COUNTED_RESOURCES = frozenset({"script", "stylesheet", "fetch", "xhr"})
# A navigation, a redirect or a replaced frame cancels its requests. That is normal, not a failure.
# WebKit says "Load request cancelled" (and also reports the response of such a request).
CANCELLED = ("ERR_ABORTED", "NS_BINDING_ABORTED", "aborted", "cancelled")
# How many addresses are named.
FAILED_URLS_SHOWN = 5

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
    # Requests that failed since the last page.goto began: how many, and the first few addresses
    # (without query or fragment, which can carry a token).
    failed_count: int = 0
    failed_urls: list[str] = field(default_factory=list)
    failed_seen: set[Request] = field(default_factory=set)

    def note_failed(self, request: Request) -> None:
        # One request counts once, even when the browser reports both its answer and its failure.
        if request in self.failed_seen:
            return
        self.failed_seen.add(request)
        self.failed_count += 1
        if len(self.failed_urls) < FAILED_URLS_SHOWN:
            self.failed_urls.append(without_query(request.url))

    def reset_failed(self) -> None:
        self.failed_count = 0
        self.failed_urls = []
        self.failed_seen = set()

    def watch_requests(self) -> None:
        """Counts failed requests for scripts, styles, data and frames. A response with status 400 or
        more is not a `requestfailed` event in Playwright, so both events are needed."""
        self.page.on("requestfailed", self._request_failed)
        self.page.on("response", self._responded)

    def _counted(self, request: Request) -> bool:
        if request.resource_type in COUNTED_RESOURCES:
            return True
        if request.resource_type != "document":
            return False
        try:
            return request.frame.parent_frame is not None
        except PlaywrightError:
            return False

    def _request_failed(self, request: Request) -> None:
        reason = request.failure or ""
        if self._counted(request) and not any(mark in reason for mark in CANCELLED):
            self.note_failed(request)

    def _responded(self, response: Response) -> None:
        if response.status >= 400 and self._counted(response.request):
            self.note_failed(response.request)

    def frame_id(self, frame: Frame) -> str:
        if frame == self.page.main_frame:
            return "main"
        if frame not in self.frame_ids:
            self.frame_ids[frame] = f"f{len(self.frame_ids) + 1}"
        return self.frame_ids[frame]


def playwright_state(state: models.StorageState | None) -> dict[str, Any] | None:
    """The state in the form Playwright takes, with the defaults it wants spelled out."""
    if state is None:
        return None
    wire = to_wire(state)
    for cookie in wire["cookies"]:
        cookie.setdefault("expires", -1)
        cookie.setdefault("httpOnly", False)
        cookie.setdefault("secure", False)
        cookie.setdefault("sameSite", "Lax")
    return wire


# The browser's own built-in functions, captured before the page's scripts run (the collector
# then reads the page with them). GENERATED: do not edit between the markers. Edit page-script/pristine.js
# and run scripts/sync_pristine.py; a test and CI check that the text is identical.
# BEGIN pristine.js
PRISTINE_JS = r"""
(function traceqaPristine() {
  "use strict";
  // Captures the browser's own built-in functions, so that what a page does to its
  // prototypes afterwards cannot change what the collector reads. The result is a table
  // of "uncurried" functions: table.getAttribute(element, "id") calls the original
  // Element.prototype.getAttribute without looking it up on the page-modifiable prototype.
  //
  // This text exists in three places: this file, the capture block inside page-script.js (used
  // when the page already ran its scripts), and the Playwright driver (which runs it before the
  // page's scripts, as an init script). THIS FILE IS THE ONLY ONE TO EDIT. Afterwards run
  // `python3 scripts/sync_pristine.py` to rewrite the other two; CI and a test run it with
  // --check. Do not use a triple double quote or a backslash in this file: the driver holds the
  // text in a raw Python string.
  //
  // Best effort: a page that ran before this did can have replaced anything, and
  // Function.prototype.toString, which tells native code from page code, can be faked too.
  var uncurry = Function.prototype.bind.bind(Function.prototype.call);
  var describe = Object.getOwnPropertyDescriptor;
  var parentOf = Object.getPrototypeOf;
  var fnToString = uncurry(Function.prototype.toString);
  var strIndexOf = uncurry(String.prototype.indexOf);

  var T = {};
  var watched = [];

  function find(owner, key) {
    try {
      for (var obj = owner; obj; obj = parentOf(obj)) {
        var d = describe(obj, key);
        if (d) return d;
      }
    } catch (e) { /* a hostile getter or proxy: treat as missing */ }
    return null;
  }

  function proto(name) {
    var ctor = typeof self !== "undefined" ? self[name] : undefined;
    return ctor && ctor.prototype ? ctor.prototype : null;
  }

  function watch(label, owner, key, kind) {
    var d = owner ? find(owner, key) : null;
    var fn = d ? (kind === "get" ? d.get : d.value) : null;
    if (typeof fn !== "function") fn = null;
    watched[watched.length] = { label: label, owner: owner, key: key, kind: kind, original: fn };
    return fn;
  }

  function method(name, label, owner, key) {
    var fn = watch(label, owner, key, "value");
    T[name] = fn ? uncurry(fn) : null;
  }

  function getter(name, label, owner, key) {
    var fn = watch(label, owner, key, "get");
    T[name] = fn ? uncurry(fn) : null;
  }

  // Functions with no receiver to speak of: Math.round(x), CSS.escape(x).
  function staticFn(name, label, owner, key, arity) {
    var fn = watch(label, owner, key, "value");
    if (!fn) {
      T[name] = null;
      return;
    }
    var call = uncurry(fn);
    T[name] = arity === 2
      ? function (a, b) { return call(owner, a, b); }
      : function (a) { return call(owner, a); };
  }

  // Only to notice a change: the collector does not call these.
  function watchOnly(label, owner, key) {
    watch(label, owner, key, "value");
  }

  var ArrayP = Array.prototype;
  var StringP = String.prototype;
  var NodeP = proto("Node");
  var ElementP = proto("Element");
  var DocumentP = proto("Document");
  var FragmentP = proto("DocumentFragment");
  var ShadowP = proto("ShadowRoot");
  var NodeListP = proto("NodeList");
  var RectP = proto("DOMRectReadOnly");

  method("push", "Array.prototype.push", ArrayP, "push");
  method("unshift", "Array.prototype.unshift", ArrayP, "unshift");
  method("concat", "Array.prototype.concat", ArrayP, "concat");
  method("indexOf", "Array.prototype.indexOf", ArrayP, "indexOf");
  method("join", "Array.prototype.join", ArrayP, "join");
  method("sliceArray", "Array.prototype.slice", ArrayP, "slice");
  method("map", "Array.prototype.map", ArrayP, "map");
  method("trim", "String.prototype.trim", StringP, "trim");
  method("sliceString", "String.prototype.slice", StringP, "slice");
  method("toLowerCase", "String.prototype.toLowerCase", StringP, "toLowerCase");
  method("charCodeAt", "String.prototype.charCodeAt", StringP, "charCodeAt");
  method("stringIndexOf", "String.prototype.indexOf", StringP, "indexOf");
  method("weakMapGet", "WeakMap.prototype.get", WeakMap.prototype, "get");
  method("weakMapSet", "WeakMap.prototype.set", WeakMap.prototype, "set");
  watchOnly("Set.prototype.has", Set.prototype, "has");
  watchOnly("Set.prototype.add", Set.prototype, "add");
  watchOnly("Function.prototype.toString", Function.prototype, "toString");

  method("getAttribute", "Element.prototype.getAttribute", ElementP, "getAttribute");
  method("hasAttribute", "Element.prototype.hasAttribute", ElementP, "hasAttribute");
  method("matches", "Element.prototype.matches", ElementP, "matches");
  method("closest", "Element.prototype.closest", ElementP, "closest");
  method("elementQuerySelector", "Element.prototype.querySelector", ElementP, "querySelector");
  method("elementQuerySelectorAll", "Element.prototype.querySelectorAll", ElementP, "querySelectorAll");
  method("getRootNode", "Node.prototype.getRootNode", NodeP, "getRootNode");
  method("getBoundingClientRect", "Element.prototype.getBoundingClientRect", ElementP, "getBoundingClientRect");
  method("checkVisibility", "Element.prototype.checkVisibility", ElementP, "checkVisibility");
  method("contains", "Node.prototype.contains", NodeP, "contains");
  method("documentElementFromPoint", "Document.prototype.elementFromPoint", DocumentP, "elementFromPoint");
  method("documentQuerySelectorAll", "Document.prototype.querySelectorAll", DocumentP, "querySelectorAll");
  method("documentGetElementById", "Document.prototype.getElementById", DocumentP, "getElementById");
  method("rootElementFromPoint", "ShadowRoot.prototype.elementFromPoint", ShadowP, "elementFromPoint");
  method("fragmentQuerySelectorAll", "DocumentFragment.prototype.querySelectorAll", FragmentP, "querySelectorAll");
  method("fragmentGetElementById", "DocumentFragment.prototype.getElementById", FragmentP, "getElementById");
  method("getPropertyValue", "CSSStyleDeclaration.prototype.getPropertyValue", proto("CSSStyleDeclaration"), "getPropertyValue");

  staticFn("round", "Math.round", Math, "round", 1);
  staticFn("max", "Math.max", Math, "max", 2);
  staticFn("min", "Math.min", Math, "min", 2);
  staticFn("cssEscape", "CSS.escape", typeof CSS !== "undefined" ? CSS : null, "escape", 1);
  staticFn("getComputedStyle", "window.getComputedStyle", typeof window !== "undefined" ? window : null, "getComputedStyle", 1);

  getter("nodeType", "Node.prototype.nodeType", NodeP, "nodeType");
  getter("nodeValue", "Node.prototype.nodeValue", NodeP, "nodeValue");
  getter("firstChild", "Node.prototype.firstChild", NodeP, "firstChild");
  getter("nextSibling", "Node.prototype.nextSibling", NodeP, "nextSibling");
  getter("parentNode", "Node.prototype.parentNode", NodeP, "parentNode");
  getter("parentElement", "Node.prototype.parentElement", NodeP, "parentElement");
  getter("textContent", "Node.prototype.textContent", NodeP, "textContent");
  getter("localName", "Element.prototype.localName", ElementP, "localName");
  getter("shadowRoot", "Element.prototype.shadowRoot", ElementP, "shadowRoot");
  getter("elementFirstElementChild", "Element.prototype.firstElementChild", ElementP, "firstElementChild");
  getter("fragmentFirstElementChild", "DocumentFragment.prototype.firstElementChild", FragmentP, "firstElementChild");
  getter("documentFirstElementChild", "Document.prototype.firstElementChild", DocumentP, "firstElementChild");
  getter("nextElementSibling", "Element.prototype.nextElementSibling", ElementP, "nextElementSibling");
  getter("host", "ShadowRoot.prototype.host", ShadowP, "host");
  getter("listLength", "NodeList.prototype.length", NodeListP, "length");
  getter("rectLeft", "DOMRect.prototype.left", RectP, "left");
  getter("rectRight", "DOMRect.prototype.right", RectP, "right");
  getter("rectTop", "DOMRect.prototype.top", RectP, "top");
  getter("rectBottom", "DOMRect.prototype.bottom", RectP, "bottom");
  getter("rectWidth", "DOMRect.prototype.width", RectP, "width");
  getter("rectHeight", "DOMRect.prototype.height", RectP, "height");

  // `labels` lives on each kind of form control, and each one's getter only takes its own kind.
  var labelGetters = {};
  var labelKinds = [
    ["input", "HTMLInputElement"], ["select", "HTMLSelectElement"], ["textarea", "HTMLTextAreaElement"],
    ["button", "HTMLButtonElement"], ["meter", "HTMLMeterElement"], ["output", "HTMLOutputElement"],
    ["progress", "HTMLProgressElement"],
  ];
  for (var k = 0; k < labelKinds.length; k++) {
    var labelGet = watch(labelKinds[k][1] + ".prototype.labels", proto(labelKinds[k][1]), "labels", "get");
    if (labelGet) labelGetters[labelKinds[k][0]] = uncurry(labelGet);
  }

  var weakMapNew = WeakMap;
  T.newWeakMap = function () { return new weakMapNew(); };

  // ---- helpers built on the table; they use nothing else ----------------------

  T.labels = function (el) {
    var get = labelGetters[T.localName(el)];
    return get ? get(el) : undefined;
  };

  T.childElements = function (parent) {
    var out = [];
    var type = T.nodeType(parent);
    var first = type === 11 ? T.fragmentFirstElementChild : type === 9 ? T.documentFirstElementChild : T.elementFirstElementChild;
    for (var child = first(parent); child; child = T.nextElementSibling(child)) out[out.length] = child;
    return out;
  };

  T.querySelectorAll = function (root, selector) {
    var type = T.nodeType(root);
    return (type === 9 ? T.documentQuerySelectorAll : type === 11 ? T.fragmentQuerySelectorAll : T.elementQuerySelectorAll)(root, selector);
  };

  T.getElementById = function (root, id) {
    var type = T.nodeType(root);
    if (type === 9) return T.documentGetElementById(root, id);
    if (type === 11) return T.fragmentGetElementById(root, id);
    return null;
  };

  T.hostOf = function (root) {
    if (T.nodeType(root) !== 11) return null;
    try { return T.host(root); } catch (e) { return null; }
  };

  T.rectOf = function (el) {
    var rect = T.getBoundingClientRect(el);
    return {
      left: T.rectLeft(rect), right: T.rectRight(rect), top: T.rectTop(rect), bottom: T.rectBottom(rect),
      width: T.rectWidth(rect), height: T.rectHeight(rect),
    };
  };

  // ---- noticing that a page changed built-ins ---------------------------------

  function looksNative(entry) {
    var fn = entry.original;
    try {
      var text = fnToString(fn);
      var at = strIndexOf(text, "[native code]");
      if (at < 0 || text.length - at > 30) return false;
      // Methods are named for their key. Getters are named differently by each engine.
      return entry.kind === "get" || fn.name === entry.key;
    } catch (e) {
      return false;
    }
  }

  // `replaced`: built-ins whose live version is no longer the one captured here.
  // `suspect`: captured ones that do not look like native code, which is what is left to
  // check when this table was made after the page had already run.
  T.verify = function () {
    var replaced = [];
    var suspect = [];
    for (var i = 0; i < watched.length; i++) {
      var entry = watched[i];
      if (entry.original === null) continue;
      var d = find(entry.owner, entry.key);
      var live = d ? (entry.kind === "get" ? d.get : d.value) : null;
      if (live !== entry.original) replaced[replaced.length] = entry.label;
      if (!looksNative(entry)) suspect[suspect.length] = entry.label;
    }
    return { replaced: replaced, suspect: suspect };
  };

  return Object.freeze(T);
})
"""
# END pristine.js

# Defines window.__traceqa_pristine__ so a page script can neither replace nor delete it. A page
# can call it, which hands it the originals and nothing else. Run in every frame of the context.
PRISTINE_INIT_SCRIPT = (
    "(function () {\n"
    "  try {\n"
    "    var table = (" + PRISTINE_JS.strip() + ")();\n"
    "    var offer = function () { return table; };\n"
    '    Object.defineProperty(window, "__traceqa_pristine__", '
    "{ value: offer, writable: false, enumerable: false, configurable: false });\n"
    "  } catch (e) { /* already defined, or this frame is not a page */ }\n"
    "})();"
)


def install_pristine(context: BrowserContext) -> None:
    """Has every page of the context capture the original built-ins before its own scripts run."""
    context.add_init_script(script=PRISTINE_INIT_SCRIPT)


def give_state(context: BrowserContext, state: dict[str, Any]) -> None:
    """Puts state into a context the driver did not create, as a browser reached over CDP has.
    Stored values are set before any of the site's own scripts run, as a new context would have them."""
    if state["cookies"]:
        context.add_cookies(state["cookies"])
    if state["origins"]:
        origins = json.dumps(state["origins"])
        context.add_init_script(
            script=f"""(() => {{
  const here = {origins}.find((entry) => entry.origin === location.origin);
  if (here) for (const item of here.localStorage) localStorage.setItem(item.name, item.value);
}})();"""
        )


# Browsers stop following redirects well before this, so a chain is never cut in practice.
MAX_REDIRECT_HOPS = 64


def redirect_chain(response: Any) -> list[models.RedirectHop] | None:
    """Every request that led to `response`, oldest first, each with the IP address the browser
    connected to when the browser says it. Lets the engine check every hop against the operator's
    address rules. None when there is no response (for example a page that did not navigate)."""
    if response is None:
        return None
    requests = []
    request = response.request
    while request is not None and len(requests) < MAX_REDIRECT_HOPS:
        requests.append(request)
        request = request.redirected_from
    hops = []
    for request in reversed(requests):
        remote = None
        try:
            answer = response if request is response.request else request.response()
            address = answer.server_addr() if answer is not None else None
            remote = address["ipAddress"] if address else None
        except PlaywrightError:
            remote = None
        hops.append(models.RedirectHop(url=request.url, remote_address=remote))
    return hops


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


def settle(page: Page, limit_s: float, *, animations: bool = True) -> None:
    """Wait until the page has stopped growing and, unless told otherwise, its animations have ended."""
    wait_until_settled(
        lambda: page.evaluate(PAGE_STATE),
        lambda seconds: page.wait_for_timeout(seconds * 1000),
        limit_s,
        animations=animations,
    )


def not_installed(text: str) -> bool:
    """True when Playwright's launch error says the browser is not on this machine.

    A bundled browser that was never downloaded says "Executable doesn't exist"; a system
    channel such as chrome or msedge says it "is not found".
    """
    return "Executable doesn't exist" in text or ("distribution" in text and "is not found" in text)


class PlaywrightDriver(BaseDriver[Session]):
    NAME = NAME
    VERSION = VERSION
    BROWSERS = (*BROWSERS, CDP)

    def __init__(self, channel: Channel) -> None:
        super().__init__(channel)
        self._playwright: Playwright | None = None

    def _stop(self) -> None:
        if self._playwright is not None:
            self._playwright.stop()
            self._playwright = None

    # ---- session ---------------------------------------------------------

    def session_open(self, params: models.SessionOpenParams) -> models.SessionOpenResult:
        if self._playwright is None:
            # Playwright waits for web fonts before a screenshot. A font request that never
            # finishes then blocks it for 30 seconds, and a late font is no reason for that.
            os.environ.setdefault("PW_TEST_SCREENSHOT_NO_FONTS_READY", "1")
            self._playwright = sync_playwright().start()
        target = params.target
        viewport = ViewportSize(width=params.viewport.width, height=params.viewport.height)
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
            state = playwright_state(params.storage_state)
            try:
                context = browser.new_context(
                    viewport=viewport, locale=params.locale, storage_state=cast(PlaywrightStorageState | None, state)
                )
                owns_context = True
            except PlaywrightError:
                # Some remote browsers only expose the context they started with.
                if not browser.contexts:
                    raise
                context = browser.contexts[0]
                owns_context = False
                if state:
                    give_state(context, state)
            # Also for a context the driver did not create: its pages are opened after this.
            install_pristine(context)
            page = context.new_page()
            if not owns_context:
                page.set_viewport_size(viewport)
        except PlaywrightError as error:
            # A browser that started but could not be set up must not be left running.
            if browser is not None:
                with contextlib.suppress(PlaywrightError):
                    browser.close()
            if target.browser in BROWSERS and not_installed(error.message):
                raise RpcError(
                    "unsupported", f"{target.browser} is not installed. Run: traceqa install {target.browser}"
                ) from error
            raise RpcError("internal", f"could not open {target.browser}: {first_line(error.message)}") from error

        session_id = self._new_session_id()
        chromium = target.browser == CDP or BROWSERS.get(target.browser, ("",))[0] == "chromium"
        session = Session(browser, context, page, owns_context, chromium)
        session.watch_requests()
        self._sessions[session_id] = session
        self._channel.log("info", f"opened {target.browser} {browser.version}")
        return models.SessionOpenResult(
            session_id=session_id,
            browser=models.BrowserInfo(name=target.browser, version=browser.version),
            capabilities=CAPABILITIES,
        )

    def session_storage(self, params: models.SessionStorageParams) -> models.StorageState:
        session = self._session(params.session_id)
        try:
            return models.StorageState.model_validate(session.context.storage_state())
        except PlaywrightError as error:
            raise RpcError("internal", f"could not read the session's state: {first_line(error.message)}") from error

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
        session.reset_failed()
        try:
            # The content first. Without it there is nothing to read, so this is the one that can fail.
            response = page.goto(params.url, wait_until="domcontentloaded", timeout=timeout)
        except PlaywrightTimeout as error:
            # Without the query or fragment: they can carry a token, and logs and run files keep this text.
            raise RpcError(
                "timeout", f"{without_query(params.url)} did not load in {timeout} ms", retryable=True
            ) from error
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
            redirect_chain=redirect_chain(response),
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
        return models.PageEvalResult(
            results=results,
            requests_failed=models.RequestsFailed(count=session.failed_count, urls=list(session.failed_urls)),
        )

    def page_scroll(self, params: models.PageScrollParams) -> models.PageScrollResult:
        page = self._session(params.session_id).page
        check_scroll_target(params)
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
            at_end=scrolled_to_end(state["y"], state["view"], state["height"]),
        )

    def page_screenshot(self, params: models.PageScreenshotParams) -> models.PageScreenshotResult:
        page = self._session(params.session_id).page
        path = Path(params.path)
        path.parent.mkdir(parents=True, exist_ok=True)
        clip: FloatRect | None = None
        if params.clip is not None:
            clip = FloatRect(x=params.clip.x, y=params.clip.y, width=params.clip.w, height=params.clip.h)
        try:
            # With a clip, full_page makes Playwright read the clip in page coordinates.
            page.screenshot(path=str(path), full_page=bool(params.full_page) or clip is not None, clip=clip)
        except PlaywrightError as error:
            raise RpcError("internal", f"screenshot failed: {first_line(error.message)}") from error
        width, height = png_dimensions(path.read_bytes()[:24])
        return models.PageScreenshotResult(path=str(path), width=width, height=height)
