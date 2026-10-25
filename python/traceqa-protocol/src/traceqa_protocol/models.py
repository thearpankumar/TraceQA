# Generated from spec/ by scripts/gen_python_protocol.py. Do not edit.

from __future__ import annotations

from pydantic import BaseModel, ConfigDict, Field
from typing import Any
from enum import Enum


class BaselineSource(BaseModel):
    """
    The run a baseline was made from.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    run_id: str = Field(..., validation_alias="runId", serialization_alias="runId")
    engine_version: str = Field(
        ...,
        validation_alias="engineVersion", serialization_alias="engineVersion",
        description="Or 'unknown' when the run had no run.json.",
    )


class BaselineAlias(BaseModel):
    """
    Says that the removed component `from` is the added component `to`. A diff then reports one renamed pair (a pairedWith link with reason 'aliased') and the removed, added and paired gates ignore it.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    from_: str = Field(..., validation_alias="from", serialization_alias="from", description="A component id in the baseline.")
    to: str = Field(..., description="A component id in the new run.")
    reason: str = Field(..., description="Known values: rename, heal, manual.")
    evidence: str | None = Field(
        default=None, description="Free text: why they are the same."
    )
    added_at: str = Field(
        ..., validation_alias="addedAt", serialization_alias="addedAt", description="RFC 3339 timestamp in UTC."
    )


class BaselinePromotion(BaseModel):
    """
    One save or promote.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    run_id: str = Field(..., validation_alias="runId", serialization_alias="runId")
    at: str
    summary: str = Field(
        ...,
        description="What changed against the baseline it replaced, for example '2 removed, 1 added, 0 changed', or 'first save'.",
    )


class BaselineRef(BaseModel):
    """
    Which baseline was compared.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    key: str
    url: str
    promoted_at: str = Field(..., validation_alias="promotedAt", serialization_alias="promotedAt")


class EngineHelloParams(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    protocol_version: int = Field(..., validation_alias="protocolVersion", serialization_alias="protocolVersion", ge=0)
    client_name: str | None = Field(default=None, validation_alias="clientName", serialization_alias="clientName")
    client_version: str | None = Field(default=None, validation_alias="clientVersion", serialization_alias="clientVersion")


class EngineHelloResult(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    engine_version: str = Field(..., validation_alias="engineVersion", serialization_alias="engineVersion")
    protocol_version: int = Field(..., validation_alias="protocolVersion", serialization_alias="protocolVersion", ge=0)
    features: list[str] = Field(
        ...,
        description="Features this engine can run. Known values: extract, diff, findings. diff and findings are the methods diff.compare and findings.analyse; a client that needs them checks this list.",
    )


class ExtractOptions(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    max_scroll_steps: int | None = Field(
        default=None,
        validation_alias="maxScrollSteps", serialization_alias="maxScrollSteps",
        description="How many screens to scroll through before giving up on a very long page. 0 reads the page without scrolling.",
        ge=0,
    )
    timeout_ms: int | None = Field(
        default=None, validation_alias="timeoutMs", serialization_alias="timeoutMs", description="Per page.", ge=0
    )
    settle_ms: int | None = Field(
        default=None,
        validation_alias="settleMs", serialization_alias="settleMs",
        description="Extra time to wait after the page has loaded, before reading it. For pages that show a timed intro or splash screen. Default 0.",
        ge=0,
    )
    crops: bool | None = Field(
        default=None,
        description="Also save pictures: a screenshot of the page and one crop per component. Slower, because the page is then read and photographed one screen at a time. Default false.",
    )
    include_hidden: bool | None = Field(
        default=None,
        validation_alias="includeHidden", serialization_alias="includeHidden",
        description="List hidden elements too. Default true.",
    )


class RunStartResult(BaseModel):
    """
    Returned as soon as the run is accepted. Progress and the result arrive as notifications.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    run_id: str = Field(..., validation_alias="runId", serialization_alias="runId")
    run_folder: str = Field(..., validation_alias="runFolder", serialization_alias="runFolder", description="Absolute path.")


class DiffCompareParams(BaseModel):
    """
    Compares one target of run folder a with one target of run folder b. Reads local files, at most 64 MiB per components.json, and answers when done; it does not start a browser.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    a: str = Field(
        ...,
        description="Path of a run folder made by run.start (side A, the older or reference side).",
    )
    b: str = Field(
        ...,
        description="Path of a run folder (side B, the newer or other side). May be the same folder as a, to compare two targets of one run.",
    )
    target_a: str | None = Field(
        default=None,
        validation_alias="targetA", serialization_alias="targetA",
        description="Target folder name in a, for example chromium-1280x800. May be left out when the run has one target.",
    )
    target_b: str | None = Field(
        default=None,
        validation_alias="targetB", serialization_alias="targetB",
        description="Target folder name in b. As targetA.",
    )


class FindingsAnalyseParams(BaseModel):
    """
    Runs the automated checks over a finished run folder. Reads local files, at most 64 MiB per components.json; writes nothing.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    run: str = Field(..., description="Path of a run folder.")
    target: str | None = Field(
        default=None,
        description="Only this target's findings are returned (every target is still analysed, so a suppression used in another browser is not reported unused). Default: all targets, then the run-level findings.",
    )
    suppressions: str | None = Field(
        default=None,
        description="Path of a suppression file (YAML, at most 256 KiB), as `traceqa findings --suppressions` takes it. Default: none.",
    )


class RunCancelParams(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    run_id: str = Field(..., validation_alias="runId", serialization_alias="runId")


class ScrollInfo(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    y: float
    height: float = Field(..., description="Full scrollable height of the document.")
    viewport_height: float = Field(..., validation_alias="viewportHeight", serialization_alias="viewportHeight")


class PathStep(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    role: str
    name: str | None = Field(
        default=None, description="Accessible name of the ancestor, when it has one."
    )
    tag: str
    repeat_index: int | None = Field(
        default=None,
        validation_alias="repeatIndex", serialization_alias="repeatIndex",
        description="Zero-based position among siblings with the same tag and role. Present only when repeatCount is 2 or more.",
        ge=0,
    )
    repeat_count: int | None = Field(
        default=None,
        validation_alias="repeatCount", serialization_alias="repeatCount",
        description="How many siblings share this tag and role.",
        ge=0,
    )


class Empty(BaseModel):
    """
    No fields.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )


class Viewport(BaseModel):
    """
    Size of the browser window content area, in CSS pixels. Each side is 1 to 10000.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    width: int = Field(..., ge=1, le=10000)
    height: int = Field(..., ge=1, le=10000)


class Rect(BaseModel):
    """
    A rectangle in CSS pixels. Inside a RawElement it is relative to that frame's document. Everywhere else it is relative to the top-level page.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    x: float
    y: float
    w: float
    h: float


class BrowserTarget(BaseModel):
    """
    Which browser to run in. The engine passes this to a driver and never interprets it.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    browser: str = Field(
        ...,
        description="Browser name. Known values: chromium, chrome, msedge, firefox, webkit, safari, cdp, webdriver.",
    )
    cdp_endpoint: str | None = Field(
        default=None,
        validation_alias="cdpEndpoint", serialization_alias="cdpEndpoint",
        description="WebSocket or HTTP endpoint of an already running browser. Used with browser = cdp.",
    )
    webdriver_url: str | None = Field(
        default=None,
        validation_alias="webdriverUrl", serialization_alias="webdriverUrl",
        description="URL of a W3C WebDriver server, local or a cloud grid. Used with browser = webdriver.",
    )
    webdriver_capabilities: dict[str, Any] | None = Field(
        default=None,
        validation_alias="webdriverCapabilities", serialization_alias="webdriverCapabilities",
        description='Capabilities to ask the WebDriver server for, for example { "browserName": "safari" }.',
    )
    headless: bool | None = None


class BrowserInfo(BaseModel):
    """
    What actually ran, as reported by the driver.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    name: str
    version: str


class SameSite(Enum):
    strict = "Strict"
    lax = "Lax"
    none = "None"


class StorageCookie(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    name: str
    value: str
    domain: str = Field(
        ...,
        description="For example example.com, or .example.com for its subdomains too.",
    )
    path: str
    expires: float | None = Field(
        default=None,
        description="Unix time in seconds. -1 or absent for a cookie that lasts until the browser closes.",
    )
    http_only: bool | None = Field(default=None, validation_alias="httpOnly", serialization_alias="httpOnly")
    secure: bool | None = None
    same_site: SameSite | None = Field(default=None, validation_alias="sameSite", serialization_alias="sameSite")


class StorageItem(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    name: str
    value: str


class Warning(BaseModel):
    """
    Something that did not stop the run but that the reader should know.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    code: str = Field(
        ...,
        description="Stable machine-readable code. Known values: cross_origin_frame_skipped, unnamed_interactive_element, scroll_limit_reached, frame_eval_failed, screenshot_failed, browsers_disagree, viewport_not_reached, http_error_status, page_load_incomplete, covered_elements, closed_shadow_roots, elements_removed_while_scrolling, session_not_accepted, signed_in_pictures, collector_tampered, requests_failed.",
    )
    message: str


class ElementState(BaseModel):
    """
    State of one element at the moment it was read.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    visible: bool = Field(
        ...,
        description="False when the element is hidden by CSS (display, visibility, opacity, content-visibility), has no size, is clipped to one pixel or less, or is inside a hidden ancestor.",
    )
    enabled: bool
    in_viewport: bool = Field(..., validation_alias="inViewport", serialization_alias="inViewport")
    checked: bool | None = None
    expanded: bool | None = None
    selected: bool | None = None
    pressed: bool | None = None
    readonly: bool | None = None
    required: bool | None = None
    covered: bool | None = Field(
        default=None,
        description="True when another element lies on top of this one at every point tested, so a click would not reach it, as under a cookie banner or a modal's backdrop. Only tested for visible elements in the viewport; absent means it was not tested.",
    )
    covered_by: str | None = Field(
        default=None,
        validation_alias="coveredBy", serialization_alias="coveredBy",
        description="A selector for the element on top, when covered is true.",
    )


class Confidence(str, Enum):
    """
    How sure we are that an element is interactive. Low means it only looks clickable (for example a div with a pointer cursor).
    """

    high = "high"
    low = "low"


class LocatorStrategy(str, Enum):
    """
    How a locator finds its component: by ARIA role and name, by test id, by visible text, or (last resort) by a CSS selector.
    """

    role = "role"
    testid = "testid"
    text = "text"
    css = "css"


class LocatorScope(BaseModel):
    """
    One ancestor a locator searches inside, outermost first in Locator.scope.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    role: str
    name: str | None = Field(
        default=None, description="The ancestor's accessible name, matched exactly."
    )


class Locator(BaseModel):
    """
    How to find a component again with Playwright-style locators (getByRole, getByTestId, getByText). The counts are over the instances collected in the same file, so uniqueness is proven for one page in one browser; `traceqa codegen` combines the files of all browsers. Never uses nth(): a component that cannot be told apart is reported as not unique, or located by CSS with forced set.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    strategy: LocatorStrategy
    role: str | None = Field(
        default=None, description="ARIA role, for the role strategy."
    )
    name: str | None = Field(
        default=None,
        description="Accessible name (role strategy) or text (text strategy). Matched exactly: whitespace normalised, case sensitive. Absent when the name is a template or changes.",
    )
    exact: bool | None = Field(
        default=None, description="True whenever a name is used."
    )
    test_id: str | None = Field(
        default=None,
        validation_alias="testId", serialization_alias="testId",
        description="Value of data-testid, for the testid strategy.",
    )
    scope: list[LocatorScope] | None = Field(
        default=None,
        description="Ancestors to search inside, outermost first, each a descendant of the one before.",
    )
    frame: str | None = Field(
        default=None,
        description="Label of the frame the component is in, as written in its path: lower case, whitespace normalised. Absent for the main frame.",
    )
    css: str | None = Field(
        default=None, description="CSS selector, only for the css strategy."
    )
    unique: bool = Field(
        ...,
        description="True when the locator matches the component's instances and nothing else collected (matches equals own).",
    )
    matches: int = Field(
        ...,
        description="How many collected instances in this file the locator matches.",
        ge=0,
    )
    own: int = Field(
        ..., description="How many of those are instances of this component.", ge=0
    )
    ambiguous_with: list[str] | None = Field(
        default=None,
        validation_alias="ambiguousWith", serialization_alias="ambiguousWith",
        description="Ids of other components the locator also matches, at most 5.",
        max_length=5,
    )
    forced: bool = Field(
        ...,
        description="True when no readable locator told the component apart and a structural CSS selector was used. Brittle: it breaks when the page layout changes.",
    )
    needs_include_hidden: bool = Field(
        ...,
        validation_alias="needsIncludeHidden", serialization_alias="needsIncludeHidden",
        description="True when every instance is hidden, so a role locator only finds it with includeHidden.",
    )


class RunStatus(str, Enum):
    running = "running"
    completed = "completed"
    failed = "failed"
    cancelled = "cancelled"


class DiffSide(BaseModel):
    """
    One of the two compared pages.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    label: str = Field(..., description="What the person typed for this side.")
    run_id: str = Field(..., validation_alias="runId", serialization_alias="runId")
    target: str | None = Field(
        default=None, description="The target folder name inside the run, when known."
    )
    browser: BrowserInfo
    viewport: Viewport
    url: str
    final_url: str = Field(..., validation_alias="finalUrl", serialization_alias="finalUrl")
    captured_at: str = Field(
        ...,
        validation_alias="capturedAt", serialization_alias="capturedAt",
        description="RFC 3339 timestamp in UTC, copied from the side's components.json.",
    )
    fingerprint_version: int = Field(..., validation_alias="fingerprintVersion", serialization_alias="fingerprintVersion", ge=0)
    component_count: int = Field(..., validation_alias="componentCount", serialization_alias="componentCount", ge=0)


class DiffSummary(BaseModel):
    """
    How many components fall in each group. A paired entry is counted under added or removed, and its pair once more in paired.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    added: int = Field(..., ge=0)
    removed: int = Field(..., ge=0)
    changed: int = Field(..., ge=0)
    unchanged: int = Field(..., ge=0)
    paired: int = Field(
        ...,
        description="Pairs of one removed and one added component that look like the same component after a change.",
        ge=0,
    )


class Kind(str, Enum):
    """
    One of added, removed, changed. An added component is in B only, a removed one in A only.
    """

    added = "added"
    removed = "removed"
    changed = "changed"


class ComponentSummary(BaseModel):
    """
    What a diff compares about one component. The state fields count the instances in that state (covered counts only instances that were tested and found covered).
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    role: str
    name: str
    name_is_template: bool = Field(..., validation_alias="nameIsTemplate", serialization_alias="nameIsTemplate")
    path: str
    instances: int = Field(..., ge=0)
    visible: int = Field(..., ge=0)
    enabled: int = Field(..., ge=0)
    covered: int = Field(..., ge=0)
    checked: int = Field(..., ge=0)
    expanded: int = Field(..., ge=0)
    selected: int = Field(..., ge=0)
    pressed: int = Field(..., ge=0)
    confidence: Confidence


class PairLink(BaseModel):
    """
    Links a removed entry to the added entry that is probably the same component. The id hashes role, name and path, so a change of role or name gives a new id.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    id: str = Field(..., description="The id of the other entry.")
    reason: str = Field(
        ...,
        description="Known values: role_changed (same path and name, other role), name_changed (same path and role, other name or template flag).",
    )
    confidence: Confidence


class DriverHelloParams(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    protocol_version: int = Field(..., validation_alias="protocolVersion", serialization_alias="protocolVersion", ge=0)
    engine_version: str = Field(..., validation_alias="engineVersion", serialization_alias="engineVersion")


class DriverHelloResult(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    driver_name: str = Field(..., validation_alias="driverName", serialization_alias="driverName")
    driver_version: str = Field(..., validation_alias="driverVersion", serialization_alias="driverVersion")
    protocol_version: int = Field(
        ...,
        validation_alias="protocolVersion", serialization_alias="protocolVersion",
        description="The version the driver speaks. The engine refuses to continue when it differs from its own.",
        ge=0,
    )
    browsers: list[str] = Field(..., description="Browser names this driver can open.")
    exclusive_browsers: list[str] | None = Field(
        default=None,
        validation_alias="exclusiveBrowsers", serialization_alias="exclusiveBrowsers",
        description="Browsers of which only one session can exist on the machine at a time, whichever process opens it. The engine never runs two of these at once.",
    )


class Capabilities(BaseModel):
    """
    What this browser session can do. The engine adapts to these instead of knowing browser names.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    clip_screenshot: bool = Field(
        ...,
        validation_alias="clipScreenshot", serialization_alias="clipScreenshot",
        description="Can screenshot one rectangle of the page.",
    )
    full_page_screenshot: bool = Field(
        ...,
        validation_alias="fullPageScreenshot", serialization_alias="fullPageScreenshot",
        description="Can screenshot the whole scrollable page.",
    )
    eval_all_frames: bool = Field(
        ...,
        validation_alias="evalAllFrames", serialization_alias="evalAllFrames",
        description="Can run a script in every frame, not only the top one.",
    )
    cross_origin_frames: bool = Field(
        ...,
        validation_alias="crossOriginFrames", serialization_alias="crossOriginFrames",
        description="Can run a script inside frames from another origin.",
    )
    parallel_sessions: bool = Field(
        ...,
        validation_alias="parallelSessions", serialization_alias="parallelSessions",
        description="Can hold more than one session at a time.",
    )
    set_viewport: bool = Field(
        ..., validation_alias="setViewport", serialization_alias="setViewport", description="Can set an exact viewport size."
    )
    console_events: bool = Field(..., validation_alias="consoleEvents", serialization_alias="consoleEvents")
    network_events: bool = Field(..., validation_alias="networkEvents", serialization_alias="networkEvents")
    failed_requests: bool | None = Field(
        default=None,
        validation_alias="failedRequests", serialization_alias="failedRequests",
        description="Reports requests for scripts, styles, data and frames that failed while the page loaded, in page.eval's requestsFailed. Absent means false.",
    )


class SessionStorageParams(BaseModel):
    """
    Asks for the session's cookies and stored values, to save them after someone has signed in.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    session_id: str = Field(..., validation_alias="sessionId", serialization_alias="sessionId")


class SessionOpenResult(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    session_id: str = Field(..., validation_alias="sessionId", serialization_alias="sessionId")
    browser: BrowserInfo
    capabilities: Capabilities
    viewport: Viewport | None = Field(
        default=None,
        description="The viewport the session really has, when the driver could not give the one asked for. Absent means the one asked for.",
    )


class SessionCloseParams(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    session_id: str = Field(..., validation_alias="sessionId", serialization_alias="sessionId")


class WaitUntil(str, Enum):
    """
    load: the load event fired. settled: load fired and the network and DOM have been quiet for a short time.
    """

    load = "load"
    settled = "settled"


class PageGotoParams(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    session_id: str = Field(..., validation_alias="sessionId", serialization_alias="sessionId")
    url: str
    wait_until: WaitUntil | None = Field(default=None, validation_alias="waitUntil", serialization_alias="waitUntil")
    timeout_ms: int | None = Field(default=None, validation_alias="timeoutMs", serialization_alias="timeoutMs", ge=0)


class RedirectHop(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    url: str
    remote_address: str | None = Field(
        default=None,
        validation_alias="remoteAddress", serialization_alias="remoteAddress",
        description="The IP address the browser connected to for this request, when the browser reports it. Absent when unknown.",
    )


class PageEvalParams(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    session_id: str = Field(..., validation_alias="sessionId", serialization_alias="sessionId")
    script: str = Field(
        ...,
        description="A JavaScript expression that evaluates to a function. The driver calls it with arg and returns its JSON-serialisable result.",
    )
    arg: Any | None = Field(
        default=None, description="Any JSON value, passed to the function."
    )
    all_frames: bool | None = Field(
        default=None,
        validation_alias="allFrames", serialization_alias="allFrames",
        description="Run in every frame the driver can reach. Default false.",
    )
    timeout_ms: int | None = Field(default=None, validation_alias="timeoutMs", serialization_alias="timeoutMs", ge=0)


class FrameInfo(BaseModel):
    """
    One frame of the page. box is where the frame element sits in the top-level page and is absent for the main frame.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    id: str = Field(
        ...,
        description="'main' for the top frame. Otherwise unique within the session.",
    )
    parent_id: str | None = Field(default=None, validation_alias="parentId", serialization_alias="parentId")
    url: str
    name: str | None = None
    cross_origin: bool = Field(
        ...,
        validation_alias="crossOrigin", serialization_alias="crossOrigin",
        description="True when the frame's origin differs from the top page's.",
    )
    box: Rect | None = None


class FrameEvalResult(BaseModel):
    """
    Exactly one of value or error is present.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    frame: FrameInfo
    value: Any | None = Field(default=None, description="What the function returned.")
    error: str | None = Field(
        default=None, description="Why the script could not run in this frame."
    )


class RequestsFailed(BaseModel):
    """
    Only from a driver whose capabilities say failedRequests. Requests for scripts, styles, fetch/XHR data and frames that failed (a network error, or an HTTP status of 400 or more) since the last page.goto began. Cumulative, so the latest answer covers the whole load. Pictures, fonts and media are not counted, and neither are requests cancelled by a navigation.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    count: int = Field(..., ge=0)
    urls: list[str] = Field(
        ...,
        description="The first five failed addresses, with the query and fragment removed by the driver.",
        max_length=5,
    )


class ScrollTo(str, Enum):
    top = "top"
    bottom = "bottom"


class ScrollWait(str, Enum):
    """
    What to wait for after scrolling. animations: the page height is stable and animations that end have ended. height: only that the page height is stable, which is quicker and enough when no picture is taken.
    """

    animations = "animations"
    height = "height"


class PageScrollParams(BaseModel):
    """
    Give exactly one of to or byY.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    session_id: str = Field(..., validation_alias="sessionId", serialization_alias="sessionId")
    to: ScrollTo | None = None
    by_y: float | None = Field(
        default=None,
        validation_alias="byY", serialization_alias="byY",
        description="Pixels to scroll down. Negative scrolls up.",
    )
    wait_for: ScrollWait | None = Field(default=None, validation_alias="waitFor", serialization_alias="waitFor")


class PageScrollResult(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    y: float
    height: float = Field(..., description="Full scrollable height after the scroll.")
    at_end: bool = Field(..., validation_alias="atEnd", serialization_alias="atEnd")


class PageScreenshotParams(BaseModel):
    """
    clip is a rectangle in top-level page coordinates and requires the clipScreenshot capability.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    session_id: str = Field(..., validation_alias="sessionId", serialization_alias="sessionId")
    path: str = Field(
        ..., description="Absolute path of the PNG file the driver must write."
    )
    full_page: bool | None = Field(
        default=None,
        validation_alias="fullPage", serialization_alias="fullPage",
        description="Default false: only the current viewport.",
    )
    clip: Rect | None = None


class PageScreenshotResult(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    path: str
    width: int = Field(..., description="Image width in device pixels.", ge=0)
    height: int = Field(..., ge=0)


class ActionKind(str, Enum):
    click = "click"
    hover = "hover"
    type = "type"
    select = "select"
    press = "press"


class ElementTarget(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    selector: str
    frame_id: str | None = Field(
        default=None, validation_alias="frameId", serialization_alias="frameId", description="Default 'main'."
    )


class PageActParams(BaseModel):
    """
    Reserved for the explorer. Component extraction never sends it.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    session_id: str = Field(..., validation_alias="sessionId", serialization_alias="sessionId")
    action: ActionKind
    target: ElementTarget
    value: str | None = Field(
        default=None, description="Text to type, option to select, or key to press."
    )


class PageActResult(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    ok: bool


class LogLevel(str, Enum):
    debug = "debug"
    info = "info"
    warn = "warn"
    error = "error"


class DriverLogParams(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    level: LogLevel
    message: str


class RpcErrorData(BaseModel):
    """
    The data field of a JSON-RPC error from a driver or the engine.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    kind: str = Field(
        ...,
        description="Known values: timeout, navigation_failed, unsupported, session_not_found, eval_failed, version_mismatch, invalid_params, invalid_request, blocked_url, busy, internal.",
    )
    retryable: bool


class FindingSeverity(str, Enum):
    """
    error: very likely a real problem. warning: probably a problem, check it. info: worth a look.
    """

    error = "error"
    warning = "warning"
    info = "info"


class FindingConfidence(str, Enum):
    """
    How sure the rule is that this is a real problem, apart from its severity.
    """

    high = "high"
    medium = "medium"
    low = "low"


class FindingScope(str, Enum):
    """
    target: one browser at one viewport, written next to its components.json. run: findings about the run as a whole, written in the run folder.
    """

    target = "target"
    run = "run"


class FindingSuppression(BaseModel):
    """
    Why a finding was set aside, from the suppression file.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    reason: str
    expires: str | None = Field(
        default=None, description="YYYY-MM-DD. The entry applies through that day."
    )


class FindingCounts(BaseModel):
    """
    Findings that are not suppressed, by severity.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    error: int = Field(..., ge=0)
    warning: int = Field(..., ge=0)
    info: int = Field(..., ge=0)


class Finding(BaseModel):
    """
    One problem found by one rule in one component.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    id: str = Field(
        ...,
        description="The rule, a colon and the component id, for example unnamed-control:c_9f2a41c07b3e. The same in every browser and run while the component id is. The run-level rule browsers-disagree adds the viewport: browsers-disagree:c_9f2a41c07b3e@1280x800.",
    )
    rule: str = Field(
        ...,
        description="Known values: unnamed-control, weak-name-source, fake-button, small-target, covered-at-load, ambiguous-names, missing-autocomplete, browsers-disagree (run scope only).",
    )
    severity: FindingSeverity
    confidence: FindingConfidence
    component_id: str = Field(..., validation_alias="componentId", serialization_alias="componentId")
    instances: list[str] = Field(
        ..., description="Keys of the instances concerned, at most 20."
    )
    role: str
    name: str
    path: str
    message: str
    evidence: dict[str, Any] = Field(
        ...,
        description="What the rule measured. The fields depend on the rule; see docs/usage.md.",
    )
    wcag: list[str] = Field(
        ...,
        description="WCAG 2.2 success criteria the check relates to, for example 2.5.8.",
    )
    help_url: str | None = Field(default=None, validation_alias="helpUrl", serialization_alias="helpUrl")
    suppressed: FindingSuppression | None = None


class FindingsFile(BaseModel):
    """
    findings.json for one target (scope target) or for the run (scope run).
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    format_version: int = Field(..., validation_alias="formatVersion", serialization_alias="formatVersion", ge=0)
    rules_version: int = Field(
        ...,
        validation_alias="rulesVersion", serialization_alias="rulesVersion",
        description="Raised when a rule is added or changes what it reports.",
        ge=0,
    )
    scope: FindingScope
    run_id: str = Field(..., validation_alias="runId", serialization_alias="runId")
    target: str | None = Field(
        default=None,
        description="Folder name inside the run folder. Absent when the file was computed from a components.json alone, and for scope run.",
    )
    url: str | None = None
    browser: BrowserInfo | None = None
    viewport: Viewport | None = None
    counts: FindingCounts
    findings: list[Finding]
    suppressed: list[Finding] = Field(
        ...,
        description="Findings set aside by the suppression file, each with its reason.",
    )
    warnings: list[Warning] = Field(
        ...,
        description="Known codes: name_source_missing, suppression_unused, suppression_expired.",
    )


class ManifestAbsent(BaseModel):
    """
    A component that must not be found.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    role: str
    name: str


class ManifestComponent(BaseModel):
    """
    One expected component.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    case: str | None = Field(
        default=None, description="Which hard case it covers (free text, for reports)."
    )
    role: str
    name: str | None = Field(
        default=None,
        description="The accessible name. Absent means an empty name unless template is true.",
    )
    template: bool | None = Field(
        default=None, description="True when the instances have different names."
    )
    instances: int | None = Field(
        default=None, description="How many copies exist. Default 1.", ge=0
    )
    confidence: Confidence | None = Field(
        default=None, description="high (default) or low."
    )
    frame: str | None = Field(
        default=None,
        description="Name of the iframe it lives in. Default: the main page.",
    )
    shadow: bool | None = Field(
        default=None, description="True when it is inside shadow DOM."
    )
    hidden: list[str] | None = Field(
        default=None, description="Names of the viewports where it is not visible."
    )
    disabled: int | None = Field(
        default=None, description="How many instances are disabled. Default 0.", ge=0
    )


class Timings(BaseModel):
    """
    Where the time of one target went, in milliseconds. The parts do not add up to durationMs exactly: grouping and writing files are not listed.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    open_ms: int = Field(
        ...,
        validation_alias="openMs", serialization_alias="openMs",
        description="Starting the driver if needed, and opening the browser.",
        ge=0,
    )
    load_ms: int = Field(
        ...,
        validation_alias="loadMs", serialization_alias="loadMs",
        description="Loading the page and waiting for it to settle, including any settle time asked for.",
        ge=0,
    )
    read_ms: int = Field(
        ...,
        validation_alias="readMs", serialization_alias="readMs",
        description="Running the collector, over all the times it ran.",
        ge=0,
    )
    scroll_ms: int = Field(
        ...,
        validation_alias="scrollMs", serialization_alias="scrollMs",
        description="Scrolling and waiting for the page to settle after each scroll.",
        ge=0,
    )
    screenshot_ms: int = Field(
        ..., validation_alias="screenshotMs", serialization_alias="screenshotMs", description="Taking screenshots.", ge=0
    )
    crop_ms: int = Field(
        ...,
        validation_alias="cropMs", serialization_alias="cropMs",
        description="Cutting the crops out of the screenshots.",
        ge=0,
    )
    screens: int | None = Field(
        default=None, description="How many screens the page was read in.", ge=0
    )


class Event(BaseModel):
    """
    One line of events.jsonl. The same object is sent live to the caller.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    seq: int = Field(
        ..., description="Starts at 0 and increases by 1 within a run.", ge=0
    )
    ts: str = Field(..., description="RFC 3339 timestamp in UTC.")
    run_id: str = Field(..., validation_alias="runId", serialization_alias="runId")
    type: str = Field(
        ...,
        description="Known values: run.started, target.started, page.opened, elements.collected, page.scrolled, components.grouped, crop.saved, warning, target.retried, target.cancelled, target.finished, run.finished, error.",
    )
    target: str | None = Field(
        default=None,
        description="The target folder name this event belongs to, when it belongs to one.",
    )
    message: str = Field(..., description="One human-readable sentence.")
    data: dict[str, Any] | None = Field(
        default=None, description="Extra fields that depend on the event type."
    )


class Instance(BaseModel):
    """
    One actual element on the page.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    key: str = Field(
        ...,
        description="The component id, a colon, and the instance's position in document order.",
    )
    name: str
    name_source: str | None = Field(
        default=None,
        validation_alias="nameSource", serialization_alias="nameSource",
        description="Where this instance's name came from. Known values: aria-label, aria-labelledby, label, content, alt, title, placeholder, value. Absent when the name is empty.",
    )
    tag: str
    attrs: dict[str, str]
    selectors: list[str]
    box: Rect
    state: ElementState
    frame_id: str = Field(..., validation_alias="frameId", serialization_alias="frameId")
    in_shadow_dom: bool = Field(..., validation_alias="inShadowDom", serialization_alias="inShadowDom")


class BaselineTarget(BaseModel):
    """
    One saved browser-and-viewport folder.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    dir: str = Field(..., description="Folder name, for example chromium-1280x800.")
    browser: BrowserInfo
    viewport: Viewport
    has_crops: bool = Field(..., validation_alias="hasCrops", serialization_alias="hasCrops")


class FindingsResult(BaseModel):
    """
    One FindingsFile per target, in the run's order, then a run-scope file when there is something for it.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    files: list[FindingsFile]


class RunEventParams(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    run_id: str = Field(..., validation_alias="runId", serialization_alias="runId")
    event: Event


class InteractiveEvidence(BaseModel):
    """
    Why the collector thinks this element can be interacted with.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    confidence: Confidence
    signals: list[str] = Field(
        ...,
        description="Known values: native, role, tabindex, onclick-attr, cursor-pointer, contenteditable.",
    )


class StorageOrigin(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    origin: str = Field(
        ..., description="Scheme, host and port, for example https://app.example.com."
    )
    local_storage: list[StorageItem] = Field(..., validation_alias="localStorage", serialization_alias="localStorage")


class DiffEntry(BaseModel):
    """
    One component that is only on one side, or that is on both and differs. Boxes and crops are not compared.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    kind: Kind = Field(
        ...,
        description="One of added, removed, changed. An added component is in B only, a removed one in A only.",
    )
    id: str = Field(..., description="The component id.")
    a: ComponentSummary | None = Field(
        default=None, description="The component on side A. Absent for added."
    )
    b: ComponentSummary | None = Field(
        default=None, description="The component on side B. Absent for removed."
    )
    fields: list[str] = Field(
        ...,
        description="For changed: the ComponentSummary fields that differ (instances, visible, enabled, covered, checked, expanded, selected, pressed, confidence). Empty otherwise.",
    )
    flags: list[str] = Field(
        ...,
        description="Ready-made facts for gating, in this order when present: became_hidden (some visible instances on A, none on B), became_visible, became_disabled (some enabled instances on A, none on B), became_enabled, became_covered (no covered instance on A, some on B), became_uncovered, count_up, count_down (instance count). Only on changed entries.",
    )
    paired_with: PairLink | None = Field(
        default=None,
        validation_alias="pairedWith", serialization_alias="pairedWith",
        description="Set on a removed or added entry that looks like the other side's entry after a change of role or name.",
    )


class PageGotoResult(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    final_url: str = Field(..., validation_alias="finalUrl", serialization_alias="finalUrl", description="URL after redirects.")
    status: int | None = Field(
        default=None, description="HTTP status of the main document, when known.", ge=0
    )
    title: str
    load_complete: bool | None = Field(
        default=None,
        validation_alias="loadComplete", serialization_alias="loadComplete",
        description="False when the page's content arrived but its load event did not fire in time, for example because one image never arrived. The page is then read as it stands. Absent means true.",
    )
    closed_shadow_roots: int | None = Field(
        default=None,
        validation_alias="closedShadowRoots", serialization_alias="closedShadowRoots",
        description="How many closed shadow roots the page has once loaded. No script can read inside one, so the collector cannot list what they hold. Absent when the driver cannot tell, as only Chromium-based browsers let a driver see them.",
        ge=0,
    )
    redirect_chain: list[RedirectHop] | None = Field(
        default=None,
        validation_alias="redirectChain", serialization_alias="redirectChain",
        description="Every address the main document was requested from on the way to finalUrl, first request first, finalUrl last. Browsers stop following redirects at about 20, so the list is short. Lets the engine check each hop against an operator's address rules. Absent when the driver cannot see the hops (classic WebDriver shows only the final address).",
    )


class PageEvalResult(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    results: list[FrameEvalResult] = Field(
        ..., description="One entry per frame, main frame first."
    )
    requests_failed: RequestsFailed | None = Field(default=None, validation_alias="requestsFailed", serialization_alias="requestsFailed")


class ManifestFile(BaseModel):
    """
    The whole manifest.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    format_version: int | None = Field(
        default=None,
        validation_alias="formatVersion", serialization_alias="formatVersion",
        description="Absent in older files, which are version 1.",
        ge=0,
    )
    viewports: dict[str, Viewport] = Field(
        ...,
        description="Named viewports, for example desktop and mobile. `hidden` lists these names.",
    )
    absent: list[ManifestAbsent] | None = Field(
        default=None,
        description="Components that must NOT be reported, for example content that only exists after a click.",
    )
    warnings: list[str] | None = Field(
        default=None, description="Warning codes the page is expected to raise."
    )
    components: list[ManifestComponent]


class TargetSummary(BaseModel):
    """
    One browser at one viewport.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    dir: str = Field(
        ...,
        description="Folder name inside the run folder, for example firefox-1280x800.",
    )
    target: BrowserTarget
    viewport: Viewport
    browser: BrowserInfo | None = None
    status: RunStatus
    component_count: int = Field(..., validation_alias="componentCount", serialization_alias="componentCount", ge=0)
    instance_count: int = Field(..., validation_alias="instanceCount", serialization_alias="instanceCount", ge=0)
    warning_count: int = Field(..., validation_alias="warningCount", serialization_alias="warningCount", ge=0)
    duration_ms: int = Field(..., validation_alias="durationMs", serialization_alias="durationMs", ge=0)
    timings: Timings | None = None
    error: str | None = Field(
        default=None, description="Why this target failed, when it did."
    )


class Component(BaseModel):
    """
    One kind of interactive element. Repeated copies are instances of the same component.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    id: str = Field(
        ...,
        description="The fingerprint: 'c_' followed by 12 hex characters. See spec/fingerprint.md.",
    )
    role: str
    name: str = Field(
        ...,
        description="Accessible name. When nameIsTemplate is true the instances have different names and this is '*'.",
    )
    name_is_template: bool = Field(..., validation_alias="nameIsTemplate", serialization_alias="nameIsTemplate")
    name_changes: bool | None = Field(
        default=None,
        validation_alias="nameChanges", serialization_alias="nameChanges",
        description="True when the name was seen to change while the page was read, as a countdown's or a cart total's does. The id then leaves the name out, so it stays the same from run to run. The name is the one read last.",
    )
    confidence: Confidence
    signals: list[str]
    path: str = Field(
        ...,
        description="Readable structural path, for example 'list \"products\" > listitem'.",
    )
    crop: str | None = Field(
        default=None,
        description="Path of a screenshot crop of the component, relative to this file. It is taken from the screen that showed the most of the component while the page was scrolled through. Absent when it never appeared.",
    )
    locator: Locator | None = Field(
        default=None,
        description="How to find the component again with a Playwright-style locator. Uniqueness is within this file (one page, one browser); `traceqa codegen` combines the files of every browser.",
    )
    instances: list[Instance]


class BaselineFile(BaseModel):
    """
    baseline.json. Holds no storage state, no run.json and no error text, so the folder is safe to commit.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    format_version: int = Field(..., validation_alias="formatVersion", serialization_alias="formatVersion", ge=0)
    key: str = Field(
        ...,
        description="The folder name: the host of the page (made safe for file names) and 8 hex characters of the SHA-256 of the page address, or the name the person gave.",
    )
    url: str = Field(
        ...,
        description="The page address with secrets replaced, as in components.json.",
    )
    fingerprint_version: int = Field(
        ...,
        validation_alias="fingerprintVersion", serialization_alias="fingerprintVersion",
        description="Ids of a run with another fingerprintVersion cannot be compared; save the baseline again.",
        ge=0,
    )
    promoted_at: str = Field(
        ...,
        validation_alias="promotedAt", serialization_alias="promotedAt",
        description="RFC 3339 timestamp in UTC of the last save or promote.",
    )
    promoted_from: BaselineSource = Field(..., validation_alias="promotedFrom", serialization_alias="promotedFrom")
    targets: list[BaselineTarget] = Field(
        ...,
        description="One entry per saved target folder. Targets pair with a run's by dir.",
    )
    aliases: list[BaselineAlias] = Field(
        ...,
        description="Rename links: a component that was removed from the page under one id and added under another is the same component.",
    )
    promotions: list[BaselinePromotion] = Field(
        ...,
        description="What each save or promote replaced, oldest first. At most the last 20 are kept.",
    )


class RawElement(BaseModel):
    """
    One interactive element exactly as found on the page.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    role: str = Field(
        ...,
        description="ARIA role, explicit or implied by the tag. 'generic' when there is none.",
    )
    name: str = Field(
        ..., description="Accessible name. Empty string when the element has none."
    )
    name_source: str | None = Field(
        default=None,
        validation_alias="nameSource", serialization_alias="nameSource",
        description="Where the name came from. Known values: aria-label, aria-labelledby, label, content, alt, title, placeholder, value.",
    )
    tag: str = Field(..., description="Lower-case tag name.")
    attrs: dict[str, str] = Field(
        ...,
        description="A fixed subset of attributes when present: id, data-testid, type, href, name, placeholder, tabindex, autocomplete, role.",
    )
    text: str | None = Field(
        default=None, description="Visible text, trimmed and capped at 200 characters."
    )
    selectors: list[str] = Field(
        ...,
        description="Ways to find this exact element again, most stable first. Each is a CSS selector inside the element's frame. For an element in shadow DOM the selector has parts joined by ' >>> ': each part is resolved inside the shadow root of the element the previous part matched.",
    )
    box: Rect
    state: ElementState
    in_shadow_dom: bool = Field(..., validation_alias="inShadowDom", serialization_alias="inShadowDom")
    path: list[PathStep] = Field(
        ...,
        description="Meaningful ancestors from the document root down to the parent. Only ancestors that have a non-generic role or that repeat among their siblings are listed.",
    )
    interactive: InteractiveEvidence


class StorageState(BaseModel):
    """
    Cookies and stored values to start a browser with, so it is already signed in or past a site's first screen. The same format as Playwright's storageState, so a file made by Playwright can be used. It holds session keys: treat it as a password. The engine never writes it to a run folder or an event.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    cookies: list[StorageCookie]
    origins: list[StorageOrigin] = Field(
        ..., description="Values in each site's localStorage."
    )


class DiffFile(BaseModel):
    """
    Output of `traceqa diff --format json`. Fully determined by its two inputs: it holds no timestamp of its own and the entries have a fixed order (removed, added, changed; then path, role, name, id).
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    format_version: int = Field(..., validation_alias="formatVersion", serialization_alias="formatVersion", ge=0)
    mode: str = Field(
        ...,
        description="What kind of comparison this is, worked out from the two sides. Known values: cross-browser (same run, same viewport, another browser), cross-viewport (same run, same browser, another viewport), run-to-run (different runs, same browser and viewport), custom.",
    )
    a: DiffSide
    b: DiffSide
    summary: DiffSummary
    entries: list[DiffEntry]
    warnings: list[Warning] = Field(
        ...,
        description="Things the reader should know about the comparison. Known codes: viewports_differ (components hidden by design are expected).",
    )


class SessionOpenParams(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    target: BrowserTarget
    viewport: Viewport
    locale: str | None = Field(
        default=None, description="BCP 47 language tag, for example en-US."
    )
    storage_state: StorageState | None = Field(
        default=None,
        validation_alias="storageState", serialization_alias="storageState",
        description="Cookies and stored values to put in place before the first page is loaded.",
    )


class RunManifest(BaseModel):
    """
    run.json. Written at the start and rewritten at the end.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    format_version: int = Field(..., validation_alias="formatVersion", serialization_alias="formatVersion", ge=0)
    run_id: str = Field(..., validation_alias="runId", serialization_alias="runId")
    feature: str = Field(..., description="Known values: extract.")
    url: str
    status: RunStatus
    started_at: str = Field(
        ..., validation_alias="startedAt", serialization_alias="startedAt", description="RFC 3339 timestamp in UTC."
    )
    finished_at: str | None = Field(default=None, validation_alias="finishedAt", serialization_alias="finishedAt")
    engine_version: str = Field(..., validation_alias="engineVersion", serialization_alias="engineVersion")
    targets: list[TargetSummary]


class ComponentsFile(BaseModel):
    """
    components.json: every interactive component found on one page, in one browser, at one viewport.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    format_version: int = Field(..., validation_alias="formatVersion", serialization_alias="formatVersion", ge=0)
    fingerprint_version: int = Field(
        ...,
        validation_alias="fingerprintVersion", serialization_alias="fingerprintVersion",
        description="Version of the recipe that produced the component ids. Ids from different versions must not be compared.",
        ge=0,
    )
    run_id: str = Field(..., validation_alias="runId", serialization_alias="runId")
    url: str = Field(..., description="URL that was asked for.")
    final_url: str = Field(..., validation_alias="finalUrl", serialization_alias="finalUrl", description="URL after redirects.")
    title: str
    browser: BrowserInfo
    viewport: Viewport = Field(
        ...,
        description="The viewport the page was read at. It differs from the target's when the browser could not give the one asked for, and a viewport_not_reached warning says so.",
    )
    captured_at: str = Field(
        ..., validation_alias="capturedAt", serialization_alias="capturedAt", description="RFC 3339 timestamp in UTC."
    )
    components: list[Component]
    warnings: list[Warning]


class BaselineTargetReport(BaseModel):
    """
    The comparison of one baseline target. Exactly one of diff and error is set.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    dir: str
    diff: DiffFile | None = None
    error: str | None = Field(
        default=None,
        description="Why the target could not be compared, for example that the run has no such target.",
    )


class RunStartParams(BaseModel):
    """
    The engine runs every target at every viewport.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    feature: str = Field(..., description="Known values: extract.")
    url: str
    targets: list[BrowserTarget]
    viewports: list[Viewport]
    out_dir: str | None = Field(
        default=None,
        validation_alias="outDir", serialization_alias="outDir",
        description="Where to create the run folder. Default ./runs.",
    )
    parallel: int | None = Field(
        default=None,
        description="How many browsers to run at the same time. 1 runs them one after another. 0 or absent lets the engine choose from the machine's cores.",
        ge=0,
    )
    options: ExtractOptions | None = None
    storage_state: StorageState | None = Field(
        default=None,
        validation_alias="storageState", serialization_alias="storageState",
        description="Start every browser with these cookies and stored values, for pages behind a sign-in or a first screen. Never written to the run folder.",
    )


class RunDoneParams(BaseModel):
    """
    Sent exactly once per run, after the run folder is complete.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    run_id: str = Field(..., validation_alias="runId", serialization_alias="runId")
    status: RunStatus
    manifest: RunManifest


class CollectorResult(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    collector_version: int = Field(..., validation_alias="collectorVersion", serialization_alias="collectorVersion", ge=0)
    url: str
    title: str
    scroll: ScrollInfo
    elements: list[RawElement] = Field(
        ..., description="Interactive elements in document order."
    )
    warnings: list[Warning]


class BaselineReport(BaseModel):
    """
    Output of `traceqa diff --against ... --format json`: one entry per baseline target, with a DiffFile whose side A is the baseline and side B the run, and the result of the gate.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    format_version: int = Field(..., validation_alias="formatVersion", serialization_alias="formatVersion", ge=0)
    baseline: BaselineRef
    fail_on: list[str] = Field(
        ...,
        validation_alias="failOn", serialization_alias="failOn",
        description="The --fail-on tokens in effect, sorted. Known values: removed, added, changed, disabled, hidden, covered, paired.",
    )
    failed: bool = Field(
        ...,
        description="True when at least one entry matched a token (the exit code is then 1).",
    )
    matched: int = Field(..., description="How many entries matched.", ge=0)
    targets: list[BaselineTargetReport]
