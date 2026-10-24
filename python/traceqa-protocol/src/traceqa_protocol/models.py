# Generated from spec/ by scripts/gen_python_protocol.py. Do not edit.

from __future__ import annotations

from pydantic import BaseModel, ConfigDict, Field
from typing import Any
from enum import Enum


class EngineHelloParams(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    protocol_version: int = Field(..., alias="protocolVersion", ge=0)
    client_name: str | None = Field(None, alias="clientName")
    client_version: str | None = Field(None, alias="clientVersion")


class EngineHelloResult(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    engine_version: str = Field(..., alias="engineVersion")
    protocol_version: int = Field(..., alias="protocolVersion", ge=0)
    features: list[str] = Field(
        ..., description="Features this engine can run. Known values: extract."
    )


class ExtractOptions(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    max_scroll_steps: int | None = Field(
        None,
        alias="maxScrollSteps",
        description="How many screens to scroll through before giving up on a very long page. 0 reads the page without scrolling.",
        ge=0,
    )
    timeout_ms: int | None = Field(
        None, alias="timeoutMs", description="Per page.", ge=0
    )
    settle_ms: int | None = Field(
        None,
        alias="settleMs",
        description="Extra time to wait after the page has loaded, before reading it. For pages that show a timed intro or splash screen. Default 0.",
        ge=0,
    )
    crops: bool | None = Field(
        None,
        description="Also save pictures: a screenshot of the page and one crop per component. Slower, because the page is then read and photographed one screen at a time. Default false.",
    )
    include_hidden: bool | None = Field(
        None,
        alias="includeHidden",
        description="List hidden elements too. Default true.",
    )


class RunStartResult(BaseModel):
    """
    Returned as soon as the run is accepted. Progress and the result arrive as notifications.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    run_id: str = Field(..., alias="runId")
    run_folder: str = Field(..., alias="runFolder", description="Absolute path.")


class RunCancelParams(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    run_id: str = Field(..., alias="runId")


class ScrollInfo(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    y: float
    height: float = Field(..., description="Full scrollable height of the document.")
    viewport_height: float = Field(..., alias="viewportHeight")


class PathStep(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    role: str
    name: str | None = Field(
        None, description="Accessible name of the ancestor, when it has one."
    )
    tag: str
    repeat_index: int | None = Field(
        None,
        alias="repeatIndex",
        description="Zero-based position among siblings with the same tag and role. Present only when repeatCount is 2 or more.",
        ge=0,
    )
    repeat_count: int | None = Field(
        None,
        alias="repeatCount",
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
    Size of the browser window content area, in CSS pixels.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    width: int = Field(..., ge=0)
    height: int = Field(..., ge=0)


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
        None,
        alias="cdpEndpoint",
        description="WebSocket or HTTP endpoint of an already running browser. Used with browser = cdp.",
    )
    webdriver_url: str | None = Field(
        None,
        alias="webdriverUrl",
        description="URL of a W3C WebDriver server, local or a cloud grid. Used with browser = webdriver.",
    )
    webdriver_capabilities: dict[str, Any] | None = Field(
        None,
        alias="webdriverCapabilities",
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
        None,
        description="Unix time in seconds. -1 or absent for a cookie that lasts until the browser closes.",
    )
    http_only: bool | None = Field(None, alias="httpOnly")
    secure: bool | None = None
    same_site: SameSite | None = Field(None, alias="sameSite")


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
    in_viewport: bool = Field(..., alias="inViewport")
    checked: bool | None = None
    expanded: bool | None = None
    selected: bool | None = None
    pressed: bool | None = None
    readonly: bool | None = None
    required: bool | None = None
    covered: bool | None = Field(
        None,
        description="True when another element lies on top of this one at every point tested, so a click would not reach it, as under a cookie banner or a modal's backdrop. Only tested for visible elements in the viewport; absent means it was not tested.",
    )
    covered_by: str | None = Field(
        None,
        alias="coveredBy",
        description="A selector for the element on top, when covered is true.",
    )


class Confidence(str, Enum):
    """
    How sure we are that an element is interactive. Low means it only looks clickable (for example a div with a pointer cursor).
    """

    high = "high"
    low = "low"


class RunStatus(str, Enum):
    running = "running"
    completed = "completed"
    failed = "failed"
    cancelled = "cancelled"


class DriverHelloParams(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    protocol_version: int = Field(..., alias="protocolVersion", ge=0)
    engine_version: str = Field(..., alias="engineVersion")


class DriverHelloResult(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    driver_name: str = Field(..., alias="driverName")
    driver_version: str = Field(..., alias="driverVersion")
    protocol_version: int = Field(
        ...,
        alias="protocolVersion",
        description="The version the driver speaks. The engine refuses to continue when it differs from its own.",
        ge=0,
    )
    browsers: list[str] = Field(..., description="Browser names this driver can open.")
    exclusive_browsers: list[str] | None = Field(
        None,
        alias="exclusiveBrowsers",
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
        alias="clipScreenshot",
        description="Can screenshot one rectangle of the page.",
    )
    full_page_screenshot: bool = Field(
        ...,
        alias="fullPageScreenshot",
        description="Can screenshot the whole scrollable page.",
    )
    eval_all_frames: bool = Field(
        ...,
        alias="evalAllFrames",
        description="Can run a script in every frame, not only the top one.",
    )
    cross_origin_frames: bool = Field(
        ...,
        alias="crossOriginFrames",
        description="Can run a script inside frames from another origin.",
    )
    parallel_sessions: bool = Field(
        ...,
        alias="parallelSessions",
        description="Can hold more than one session at a time.",
    )
    set_viewport: bool = Field(
        ..., alias="setViewport", description="Can set an exact viewport size."
    )
    console_events: bool = Field(..., alias="consoleEvents")
    network_events: bool = Field(..., alias="networkEvents")
    failed_requests: bool | None = Field(
        None,
        alias="failedRequests",
        description="Reports requests for scripts, styles, data and frames that failed while the page loaded, in page.eval's requestsFailed. Absent means false.",
    )


class SessionStorageParams(BaseModel):
    """
    Asks for the session's cookies and stored values, to save them after someone has signed in.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    session_id: str = Field(..., alias="sessionId")


class SessionOpenResult(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    session_id: str = Field(..., alias="sessionId")
    browser: BrowserInfo
    capabilities: Capabilities
    viewport: Viewport | None = Field(
        None,
        description="The viewport the session really has, when the driver could not give the one asked for. Absent means the one asked for.",
    )


class SessionCloseParams(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    session_id: str = Field(..., alias="sessionId")


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
    session_id: str = Field(..., alias="sessionId")
    url: str
    wait_until: WaitUntil | None = Field(None, alias="waitUntil")
    timeout_ms: int | None = Field(None, alias="timeoutMs", ge=0)


class RedirectHop(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    url: str
    remote_address: str | None = Field(
        None,
        alias="remoteAddress",
        description="The IP address the browser connected to for this request, when the browser reports it. Absent when unknown.",
    )


class PageEvalParams(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    session_id: str = Field(..., alias="sessionId")
    script: str = Field(
        ...,
        description="A JavaScript expression that evaluates to a function. The driver calls it with arg and returns its JSON-serialisable result.",
    )
    arg: Any | None = Field(None, description="Any JSON value, passed to the function.")
    all_frames: bool | None = Field(
        None,
        alias="allFrames",
        description="Run in every frame the driver can reach. Default false.",
    )
    timeout_ms: int | None = Field(None, alias="timeoutMs", ge=0)


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
    parent_id: str | None = Field(None, alias="parentId")
    url: str
    name: str | None = None
    cross_origin: bool = Field(
        ...,
        alias="crossOrigin",
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
    value: Any | None = Field(None, description="What the function returned.")
    error: str | None = Field(
        None, description="Why the script could not run in this frame."
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
    session_id: str = Field(..., alias="sessionId")
    to: ScrollTo | None = None
    by_y: float | None = Field(
        None, alias="byY", description="Pixels to scroll down. Negative scrolls up."
    )
    wait_for: ScrollWait | None = Field(None, alias="waitFor")


class PageScrollResult(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    y: float
    height: float = Field(..., description="Full scrollable height after the scroll.")
    at_end: bool = Field(..., alias="atEnd")


class PageScreenshotParams(BaseModel):
    """
    clip is a rectangle in top-level page coordinates and requires the clipScreenshot capability.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    session_id: str = Field(..., alias="sessionId")
    path: str = Field(
        ..., description="Absolute path of the PNG file the driver must write."
    )
    full_page: bool | None = Field(
        None, alias="fullPage", description="Default false: only the current viewport."
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
    frame_id: str | None = Field(None, alias="frameId", description="Default 'main'.")


class PageActParams(BaseModel):
    """
    Reserved for the explorer. Component extraction never sends it.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    session_id: str = Field(..., alias="sessionId")
    action: ActionKind
    target: ElementTarget
    value: str | None = Field(
        None, description="Text to type, option to select, or key to press."
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


class Timings(BaseModel):
    """
    Where the time of one target went, in milliseconds. The parts do not add up to durationMs exactly: grouping and writing files are not listed.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    open_ms: int = Field(
        ...,
        alias="openMs",
        description="Starting the driver if needed, and opening the browser.",
        ge=0,
    )
    load_ms: int = Field(
        ...,
        alias="loadMs",
        description="Loading the page and waiting for it to settle, including any settle time asked for.",
        ge=0,
    )
    read_ms: int = Field(
        ...,
        alias="readMs",
        description="Running the collector, over all the times it ran.",
        ge=0,
    )
    scroll_ms: int = Field(
        ...,
        alias="scrollMs",
        description="Scrolling and waiting for the page to settle after each scroll.",
        ge=0,
    )
    screenshot_ms: int = Field(
        ..., alias="screenshotMs", description="Taking screenshots.", ge=0
    )
    crop_ms: int = Field(
        ...,
        alias="cropMs",
        description="Cutting the crops out of the screenshots.",
        ge=0,
    )
    screens: int | None = Field(
        None, description="How many screens the page was read in.", ge=0
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
    run_id: str = Field(..., alias="runId")
    type: str = Field(
        ...,
        description="Known values: run.started, target.started, page.opened, elements.collected, page.scrolled, components.grouped, crop.saved, warning, target.retried, target.finished, run.finished, error.",
    )
    target: str | None = Field(
        None,
        description="The target folder name this event belongs to, when it belongs to one.",
    )
    message: str = Field(..., description="One human-readable sentence.")
    data: dict[str, Any] | None = Field(
        None, description="Extra fields that depend on the event type."
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
    tag: str
    attrs: dict[str, str]
    selectors: list[str]
    box: Rect
    state: ElementState
    frame_id: str = Field(..., alias="frameId")
    in_shadow_dom: bool = Field(..., alias="inShadowDom")


class RunEventParams(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    run_id: str = Field(..., alias="runId")
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
    local_storage: list[StorageItem] = Field(..., alias="localStorage")


class PageGotoResult(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    final_url: str = Field(..., alias="finalUrl", description="URL after redirects.")
    status: int | None = Field(
        None, description="HTTP status of the main document, when known.", ge=0
    )
    title: str
    load_complete: bool | None = Field(
        None,
        alias="loadComplete",
        description="False when the page's content arrived but its load event did not fire in time, for example because one image never arrived. The page is then read as it stands. Absent means true.",
    )
    closed_shadow_roots: int | None = Field(
        None,
        alias="closedShadowRoots",
        description="How many closed shadow roots the page has once loaded. No script can read inside one, so the collector cannot list what they hold. Absent when the driver cannot tell, as only Chromium-based browsers let a driver see them.",
        ge=0,
    )
    redirect_chain: list[RedirectHop] | None = Field(
        None,
        alias="redirectChain",
        description="Every address the main document was requested from on the way to finalUrl, first request first, finalUrl last. Browsers stop following redirects at about 20, so the list is short. Lets the engine check each hop against an operator's address rules. Absent when the driver cannot see the hops (classic WebDriver shows only the final address).",
    )


class PageEvalResult(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    results: list[FrameEvalResult] = Field(
        ..., description="One entry per frame, main frame first."
    )
    requests_failed: RequestsFailed | None = Field(None, alias="requestsFailed")


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
    component_count: int = Field(..., alias="componentCount", ge=0)
    instance_count: int = Field(..., alias="instanceCount", ge=0)
    warning_count: int = Field(..., alias="warningCount", ge=0)
    duration_ms: int = Field(..., alias="durationMs", ge=0)
    timings: Timings | None = None
    error: str | None = Field(None, description="Why this target failed, when it did.")


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
    name_is_template: bool = Field(..., alias="nameIsTemplate")
    name_changes: bool | None = Field(
        None,
        alias="nameChanges",
        description="True when the name was seen to change while the page was read, as a countdown's or a cart total's does. The id then leaves the name out, so it stays the same from run to run. The name is the one read last.",
    )
    confidence: Confidence
    signals: list[str]
    path: str = Field(
        ...,
        description="Readable structural path, for example 'list \"products\" > listitem'.",
    )
    crop: str | None = Field(
        None,
        description="Path of a screenshot crop of the component, relative to this file. It is taken from the screen that showed the most of the component while the page was scrolled through. Absent when it never appeared.",
    )
    instances: list[Instance]


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
        None,
        alias="nameSource",
        description="Where the name came from. Known values: aria-label, aria-labelledby, label, content, alt, title, placeholder, value.",
    )
    tag: str = Field(..., description="Lower-case tag name.")
    attrs: dict[str, str] = Field(
        ...,
        description="A fixed subset of attributes when present: id, data-testid, type, href, name, placeholder.",
    )
    text: str | None = Field(
        None, description="Visible text, trimmed and capped at 200 characters."
    )
    selectors: list[str] = Field(
        ...,
        description="Ways to find this exact element again, most stable first. Each is a CSS selector inside the element's frame. For an element in shadow DOM the selector has parts joined by ' >>> ': each part is resolved inside the shadow root of the element the previous part matched.",
    )
    box: Rect
    state: ElementState
    in_shadow_dom: bool = Field(..., alias="inShadowDom")
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


class SessionOpenParams(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    target: BrowserTarget
    viewport: Viewport
    locale: str | None = Field(
        None, description="BCP 47 language tag, for example en-US."
    )
    storage_state: StorageState | None = Field(
        None,
        alias="storageState",
        description="Cookies and stored values to put in place before the first page is loaded.",
    )


class RunManifest(BaseModel):
    """
    run.json. Written at the start and rewritten at the end.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    format_version: int = Field(..., alias="formatVersion", ge=0)
    run_id: str = Field(..., alias="runId")
    feature: str = Field(..., description="Known values: extract.")
    url: str
    status: RunStatus
    started_at: str = Field(
        ..., alias="startedAt", description="RFC 3339 timestamp in UTC."
    )
    finished_at: str | None = Field(None, alias="finishedAt")
    engine_version: str = Field(..., alias="engineVersion")
    targets: list[TargetSummary]


class ComponentsFile(BaseModel):
    """
    components.json: every interactive component found on one page, in one browser, at one viewport.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    format_version: int = Field(..., alias="formatVersion", ge=0)
    fingerprint_version: int = Field(
        ...,
        alias="fingerprintVersion",
        description="Version of the recipe that produced the component ids. Ids from different versions must not be compared.",
        ge=0,
    )
    run_id: str = Field(..., alias="runId")
    url: str = Field(..., description="URL that was asked for.")
    final_url: str = Field(..., alias="finalUrl", description="URL after redirects.")
    title: str
    browser: BrowserInfo
    viewport: Viewport = Field(
        ...,
        description="The viewport the page was read at. It differs from the target's when the browser could not give the one asked for, and a viewport_not_reached warning says so.",
    )
    captured_at: str = Field(
        ..., alias="capturedAt", description="RFC 3339 timestamp in UTC."
    )
    components: list[Component]
    warnings: list[Warning]


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
        None,
        alias="outDir",
        description="Where to create the run folder. Default ./runs.",
    )
    parallel: int | None = Field(
        None,
        description="How many browsers to run at the same time. 1 runs them one after another. 0 or absent lets the engine choose from the machine's cores.",
        ge=0,
    )
    options: ExtractOptions | None = None
    storage_state: StorageState | None = Field(
        None,
        alias="storageState",
        description="Start every browser with these cookies and stored values, for pages behind a sign-in or a first screen. Never written to the run folder.",
    )


class RunDoneParams(BaseModel):
    """
    Sent exactly once per run, after the run folder is complete.
    """

    model_config = ConfigDict(
        populate_by_name=True,
    )
    run_id: str = Field(..., alias="runId")
    status: RunStatus
    manifest: RunManifest


class CollectorResult(BaseModel):
    model_config = ConfigDict(
        populate_by_name=True,
    )
    collector_version: int = Field(..., alias="collectorVersion", ge=0)
    url: str
    title: str
    scroll: ScrollInfo
    elements: list[RawElement] = Field(
        ..., description="Interactive elements in document order."
    )
    warnings: list[Warning]
