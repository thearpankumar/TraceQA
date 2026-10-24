// Generated from spec/ by scripts/gen-protocol.mjs. Do not edit.

export type RunStatus = ("running" | "completed" | "failed" | "cancelled")
/**
 * How sure we are that an element is interactive. Low means it only looks clickable (for example a div with a pointer cursor).
 */
export type Confidence = ("high" | "low")
/**
 * load: the load event fired. settled: load fired and the network and DOM have been quiet for a short time.
 */
export type WaitUntil = ("load" | "settled")
export type ScrollTo = ("top" | "bottom")
/**
 * What to wait for after scrolling. animations: the page height is stable and animations that end have ended. height: only that the page height is stable, which is quicker and enough when no picture is taken.
 */
export type ScrollWait = ("animations" | "height")
export type ActionKind = ("click" | "hover" | "type" | "select" | "press")
export type LogLevel = ("debug" | "info" | "warn" | "error")

export interface SpecTypes {
EngineHelloParams?: EngineHelloParams
EngineHelloResult?: EngineHelloResult
ExtractOptions?: ExtractOptions
RunStartParams?: RunStartParams
RunStartResult?: RunStartResult
RunCancelParams?: RunCancelParams
RunEventParams?: RunEventParams
RunDoneParams?: RunDoneParams
CollectorResult?: CollectorResult
ScrollInfo?: ScrollInfo
RawElement?: RawElement
PathStep?: PathStep
InteractiveEvidence?: InteractiveEvidence
Empty?: Empty
Viewport?: Viewport
Rect?: Rect
BrowserTarget?: BrowserTarget
BrowserInfo?: BrowserInfo
StorageState?: StorageState1
StorageCookie?: StorageCookie
StorageOrigin?: StorageOrigin
StorageItem?: StorageItem
Warning?: Warning
ElementState?: ElementState
Confidence?: Confidence
RunStatus?: RunStatus
DriverHelloParams?: DriverHelloParams
DriverHelloResult?: DriverHelloResult
Capabilities?: Capabilities
SessionOpenParams?: SessionOpenParams
SessionStorageParams?: SessionStorageParams
SessionOpenResult?: SessionOpenResult
SessionCloseParams?: SessionCloseParams
WaitUntil?: WaitUntil
PageGotoParams?: PageGotoParams
PageGotoResult?: PageGotoResult
RedirectHop?: RedirectHop
PageEvalParams?: PageEvalParams
FrameInfo?: FrameInfo
FrameEvalResult?: FrameEvalResult
PageEvalResult?: PageEvalResult
ScrollTo?: ScrollTo
ScrollWait?: ScrollWait
PageScrollParams?: PageScrollParams
PageScrollResult?: PageScrollResult
PageScreenshotParams?: PageScreenshotParams
PageScreenshotResult?: PageScreenshotResult
ActionKind?: ActionKind
ElementTarget?: ElementTarget
PageActParams?: PageActParams
PageActResult?: PageActResult
LogLevel?: LogLevel
DriverLogParams?: DriverLogParams
RpcErrorData?: RpcErrorData
RunManifest?: RunManifest
TargetSummary?: TargetSummary
Timings?: Timings
Event?: Event
ComponentsFile?: ComponentsFile
Component?: Component
Instance?: Instance
}
export interface EngineHelloParams {
protocolVersion: number
clientName?: string
clientVersion?: string
}
export interface EngineHelloResult {
engineVersion: string
protocolVersion: number
/**
 * Features this engine can run. Known values: extract.
 */
features: string[]
}
export interface ExtractOptions {
/**
 * How many screens to scroll through before giving up on a very long page. 0 reads the page without scrolling.
 */
maxScrollSteps?: number
/**
 * Per page.
 */
timeoutMs?: number
/**
 * Extra time to wait after the page has loaded, before reading it. For pages that show a timed intro or splash screen. Default 0.
 */
settleMs?: number
/**
 * Also save pictures: a screenshot of the page and one crop per component. Slower, because the page is then read and photographed one screen at a time. Default false.
 */
crops?: boolean
/**
 * List hidden elements too. Default true.
 */
includeHidden?: boolean
}
/**
 * The engine runs every target at every viewport.
 */
export interface RunStartParams {
/**
 * Known values: extract.
 */
feature: string
url: string
targets: BrowserTarget[]
viewports: Viewport[]
/**
 * Where to create the run folder. Default ./runs.
 */
outDir?: string
/**
 * How many browsers to run at the same time. 1 runs them one after another. 0 or absent lets the engine choose from the machine's cores.
 */
parallel?: number
options?: ExtractOptions
storageState?: StorageState
}
/**
 * Which browser to run in. The engine passes this to a driver and never interprets it.
 */
export interface BrowserTarget {
/**
 * Browser name. Known values: chromium, chrome, msedge, firefox, webkit, safari, cdp, webdriver.
 */
browser: string
/**
 * WebSocket or HTTP endpoint of an already running browser. Used with browser = cdp.
 */
cdpEndpoint?: string
/**
 * URL of a W3C WebDriver server, local or a cloud grid. Used with browser = webdriver.
 */
webdriverUrl?: string
/**
 * Capabilities to ask the WebDriver server for, for example { "browserName": "safari" }.
 */
webdriverCapabilities?: {
[k: string]: unknown
}
headless?: boolean
}
/**
 * Size of the browser window content area, in CSS pixels.
 */
export interface Viewport {
width: number
height: number
}
/**
 * Start every browser with these cookies and stored values, for pages behind a sign-in or a first screen. Never written to the run folder.
 */
export interface StorageState {
cookies: StorageCookie[]
/**
 * Values in each site's localStorage.
 */
origins: StorageOrigin[]
}
export interface StorageCookie {
name: string
value: string
/**
 * For example example.com, or .example.com for its subdomains too.
 */
domain: string
path: string
/**
 * Unix time in seconds. -1 or absent for a cookie that lasts until the browser closes.
 */
expires?: number
httpOnly?: boolean
secure?: boolean
sameSite?: ("Strict" | "Lax" | "None")
}
export interface StorageOrigin {
/**
 * Scheme, host and port, for example https://app.example.com.
 */
origin: string
localStorage: StorageItem[]
}
export interface StorageItem {
name: string
value: string
}
/**
 * Returned as soon as the run is accepted. Progress and the result arrive as notifications.
 */
export interface RunStartResult {
runId: string
/**
 * Absolute path.
 */
runFolder: string
}
export interface RunCancelParams {
runId: string
}
export interface RunEventParams {
runId: string
event: Event
}
/**
 * One line of events.jsonl. The same object is sent live to the caller.
 */
export interface Event {
/**
 * Starts at 0 and increases by 1 within a run.
 */
seq: number
/**
 * RFC 3339 timestamp in UTC.
 */
ts: string
runId: string
/**
 * Known values: run.started, target.started, page.opened, elements.collected, page.scrolled, components.grouped, crop.saved, warning, target.retried, target.finished, run.finished, error.
 */
type: string
/**
 * The target folder name this event belongs to, when it belongs to one.
 */
target?: string
/**
 * One human-readable sentence.
 */
message: string
/**
 * Extra fields that depend on the event type.
 */
data?: {
[k: string]: unknown
}
}
/**
 * Sent exactly once per run, after the run folder is complete.
 */
export interface RunDoneParams {
runId: string
status: RunStatus
manifest: RunManifest
}
/**
 * run.json. Written at the start and rewritten at the end.
 */
export interface RunManifest {
formatVersion: number
runId: string
/**
 * Known values: extract.
 */
feature: string
url: string
status: RunStatus
/**
 * RFC 3339 timestamp in UTC.
 */
startedAt: string
finishedAt?: string
engineVersion: string
targets: TargetSummary[]
}
/**
 * One browser at one viewport.
 */
export interface TargetSummary {
/**
 * Folder name inside the run folder, for example firefox-1280x800.
 */
dir: string
target: BrowserTarget
viewport: Viewport
browser?: BrowserInfo
status: RunStatus
componentCount: number
instanceCount: number
warningCount: number
durationMs: number
timings?: Timings
/**
 * Why this target failed, when it did.
 */
error?: string
}
/**
 * What actually ran, as reported by the driver.
 */
export interface BrowserInfo {
name: string
version: string
}
/**
 * Where the time of one target went, in milliseconds. The parts do not add up to durationMs exactly: grouping and writing files are not listed.
 */
export interface Timings {
/**
 * Starting the driver if needed, and opening the browser.
 */
openMs: number
/**
 * Loading the page and waiting for it to settle, including any settle time asked for.
 */
loadMs: number
/**
 * Running the collector, over all the times it ran.
 */
readMs: number
/**
 * Scrolling and waiting for the page to settle after each scroll.
 */
scrollMs: number
/**
 * Taking screenshots.
 */
screenshotMs: number
/**
 * Cutting the crops out of the screenshots.
 */
cropMs: number
/**
 * How many screens the page was read in.
 */
screens?: number
}
export interface CollectorResult {
collectorVersion: number
url: string
title: string
scroll: ScrollInfo
/**
 * Interactive elements in document order.
 */
elements: RawElement[]
warnings: Warning[]
}
export interface ScrollInfo {
y: number
/**
 * Full scrollable height of the document.
 */
height: number
viewportHeight: number
}
/**
 * One interactive element exactly as found on the page.
 */
export interface RawElement {
/**
 * ARIA role, explicit or implied by the tag. 'generic' when there is none.
 */
role: string
/**
 * Accessible name. Empty string when the element has none.
 */
name: string
/**
 * Where the name came from. Known values: aria-label, aria-labelledby, label, content, alt, title, placeholder, value.
 */
nameSource?: string
/**
 * Lower-case tag name.
 */
tag: string
/**
 * A fixed subset of attributes when present: id, data-testid, type, href, name, placeholder.
 */
attrs: {
[k: string]: string
}
/**
 * Visible text, trimmed and capped at 200 characters.
 */
text?: string
/**
 * Ways to find this exact element again, most stable first. Each is a CSS selector inside the element's frame. For an element in shadow DOM the selector has parts joined by ' >>> ': each part is resolved inside the shadow root of the element the previous part matched.
 */
selectors: string[]
box: Rect
state: ElementState
inShadowDom: boolean
/**
 * Meaningful ancestors from the document root down to the parent. Only ancestors that have a non-generic role or that repeat among their siblings are listed.
 */
path: PathStep[]
interactive: InteractiveEvidence
}
/**
 * A rectangle in CSS pixels. Inside a RawElement it is relative to that frame's document. Everywhere else it is relative to the top-level page.
 */
export interface Rect {
x: number
y: number
w: number
h: number
}
/**
 * State of one element at the moment it was read.
 */
export interface ElementState {
/**
 * False when the element is hidden by CSS (display, visibility, opacity, content-visibility), has no size, is clipped to one pixel or less, or is inside a hidden ancestor.
 */
visible: boolean
enabled: boolean
inViewport: boolean
checked?: boolean
expanded?: boolean
selected?: boolean
pressed?: boolean
readonly?: boolean
required?: boolean
/**
 * True when another element lies on top of this one at every point tested, so a click would not reach it, as under a cookie banner or a modal's backdrop. Only tested for visible elements in the viewport; absent means it was not tested.
 */
covered?: boolean
/**
 * A selector for the element on top, when covered is true.
 */
coveredBy?: string
}
export interface PathStep {
role: string
/**
 * Accessible name of the ancestor, when it has one.
 */
name?: string
tag: string
/**
 * Zero-based position among siblings with the same tag and role. Present only when repeatCount is 2 or more.
 */
repeatIndex?: number
/**
 * How many siblings share this tag and role.
 */
repeatCount?: number
}
/**
 * Why the collector thinks this element can be interacted with.
 */
export interface InteractiveEvidence {
confidence: Confidence
/**
 * Known values: native, role, tabindex, onclick-attr, cursor-pointer, contenteditable.
 */
signals: string[]
}
/**
 * Something that did not stop the run but that the reader should know.
 */
export interface Warning {
/**
 * Stable machine-readable code. Known values: cross_origin_frame_skipped, unnamed_interactive_element, scroll_limit_reached, frame_eval_failed, screenshot_failed, browsers_disagree, viewport_not_reached, http_error_status, page_load_incomplete, covered_elements, closed_shadow_roots, elements_removed_while_scrolling, session_not_accepted, signed_in_pictures, collector_tampered.
 */
code: string
message: string
}
/**
 * No fields.
 */
export interface Empty {

}
/**
 * Cookies and stored values to start a browser with, so it is already signed in or past a site's first screen. The same format as Playwright's storageState, so a file made by Playwright can be used. It holds session keys: treat it as a password. The engine never writes it to a run folder or an event.
 */
export interface StorageState1 {
cookies: StorageCookie[]
/**
 * Values in each site's localStorage.
 */
origins: StorageOrigin[]
}
export interface DriverHelloParams {
protocolVersion: number
engineVersion: string
}
export interface DriverHelloResult {
driverName: string
driverVersion: string
/**
 * The version the driver speaks. The engine refuses to continue when it differs from its own.
 */
protocolVersion: number
/**
 * Browser names this driver can open.
 */
browsers: string[]
/**
 * Browsers of which only one session can exist on the machine at a time, whichever process opens it. The engine never runs two of these at once.
 */
exclusiveBrowsers?: string[]
}
/**
 * What this browser session can do. The engine adapts to these instead of knowing browser names.
 */
export interface Capabilities {
/**
 * Can screenshot one rectangle of the page.
 */
clipScreenshot: boolean
/**
 * Can screenshot the whole scrollable page.
 */
fullPageScreenshot: boolean
/**
 * Can run a script in every frame, not only the top one.
 */
evalAllFrames: boolean
/**
 * Can run a script inside frames from another origin.
 */
crossOriginFrames: boolean
/**
 * Can hold more than one session at a time.
 */
parallelSessions: boolean
/**
 * Can set an exact viewport size.
 */
setViewport: boolean
consoleEvents: boolean
networkEvents: boolean
}
export interface SessionOpenParams {
target: BrowserTarget
viewport: Viewport
/**
 * BCP 47 language tag, for example en-US.
 */
locale?: string
storageState?: StorageState2
}
/**
 * Cookies and stored values to put in place before the first page is loaded.
 */
export interface StorageState2 {
cookies: StorageCookie[]
/**
 * Values in each site's localStorage.
 */
origins: StorageOrigin[]
}
/**
 * Asks for the session's cookies and stored values, to save them after someone has signed in.
 */
export interface SessionStorageParams {
sessionId: string
}
export interface SessionOpenResult {
sessionId: string
browser: BrowserInfo
capabilities: Capabilities
viewport?: Viewport1
}
/**
 * Size of the browser window content area, in CSS pixels.
 */
export interface Viewport1 {
width: number
height: number
}
export interface SessionCloseParams {
sessionId: string
}
export interface PageGotoParams {
sessionId: string
url: string
waitUntil?: WaitUntil
timeoutMs?: number
}
export interface PageGotoResult {
/**
 * URL after redirects.
 */
finalUrl: string
/**
 * HTTP status of the main document, when known.
 */
status?: number
title: string
/**
 * False when the page's content arrived but its load event did not fire in time, for example because one image never arrived. The page is then read as it stands. Absent means true.
 */
loadComplete?: boolean
/**
 * How many closed shadow roots the page has once loaded. No script can read inside one, so the collector cannot list what they hold. Absent when the driver cannot tell, as only Chromium-based browsers let a driver see them.
 */
closedShadowRoots?: number
/**
 * Every address the main document was requested from on the way to finalUrl, first request first, finalUrl last. Browsers stop following redirects at about 20, so the list is short. Lets the engine check each hop against an operator's address rules. Absent when the driver cannot see the hops (classic WebDriver shows only the final address).
 */
redirectChain?: RedirectHop[]
}
export interface RedirectHop {
url: string
/**
 * The IP address the browser connected to for this request, when the browser reports it. Absent when unknown.
 */
remoteAddress?: string
}
export interface PageEvalParams {
sessionId: string
/**
 * A JavaScript expression that evaluates to a function. The driver calls it with arg and returns its JSON-serialisable result.
 */
script: string
/**
 * Any JSON value, passed to the function.
 */
arg?: unknown
/**
 * Run in every frame the driver can reach. Default false.
 */
allFrames?: boolean
timeoutMs?: number
}
/**
 * One frame of the page. box is where the frame element sits in the top-level page and is absent for the main frame.
 */
export interface FrameInfo {
/**
 * 'main' for the top frame. Otherwise unique within the session.
 */
id: string
parentId?: string
url: string
name?: string
/**
 * True when the frame's origin differs from the top page's.
 */
crossOrigin: boolean
box?: Rect
}
/**
 * Exactly one of value or error is present.
 */
export interface FrameEvalResult {
frame: FrameInfo
/**
 * What the function returned.
 */
value?: unknown
/**
 * Why the script could not run in this frame.
 */
error?: string
}
export interface PageEvalResult {
/**
 * One entry per frame, main frame first.
 */
results: FrameEvalResult[]
}
/**
 * Give exactly one of to or byY.
 */
export interface PageScrollParams {
sessionId: string
to?: ScrollTo
/**
 * Pixels to scroll down. Negative scrolls up.
 */
byY?: number
waitFor?: ScrollWait
}
export interface PageScrollResult {
y: number
/**
 * Full scrollable height after the scroll.
 */
height: number
atEnd: boolean
}
/**
 * clip is a rectangle in top-level page coordinates and requires the clipScreenshot capability.
 */
export interface PageScreenshotParams {
sessionId: string
/**
 * Absolute path of the PNG file the driver must write.
 */
path: string
/**
 * Default false: only the current viewport.
 */
fullPage?: boolean
clip?: Rect
}
export interface PageScreenshotResult {
path: string
/**
 * Image width in device pixels.
 */
width: number
height: number
}
export interface ElementTarget {
selector: string
/**
 * Default 'main'.
 */
frameId?: string
}
/**
 * Reserved for the explorer. Component extraction never sends it.
 */
export interface PageActParams {
sessionId: string
action: ActionKind
target: ElementTarget
/**
 * Text to type, option to select, or key to press.
 */
value?: string
}
export interface PageActResult {
ok: boolean
}
export interface DriverLogParams {
level: LogLevel
message: string
}
/**
 * The data field of a JSON-RPC error from a driver or the engine.
 */
export interface RpcErrorData {
/**
 * Known values: timeout, navigation_failed, unsupported, session_not_found, eval_failed, version_mismatch, invalid_params, invalid_request, blocked_url, busy, internal.
 */
kind: string
retryable: boolean
}
/**
 * components.json: every interactive component found on one page, in one browser, at one viewport.
 */
export interface ComponentsFile {
formatVersion: number
/**
 * Version of the recipe that produced the component ids. Ids from different versions must not be compared.
 */
fingerprintVersion: number
runId: string
/**
 * URL that was asked for.
 */
url: string
/**
 * URL after redirects.
 */
finalUrl: string
title: string
browser: BrowserInfo
viewport: Viewport2
/**
 * RFC 3339 timestamp in UTC.
 */
capturedAt: string
components: Component[]
warnings: Warning[]
}
/**
 * Size of the browser window content area, in CSS pixels.
 */
export interface Viewport2 {
width: number
height: number
}
/**
 * One kind of interactive element. Repeated copies are instances of the same component.
 */
export interface Component {
/**
 * The fingerprint: 'c_' followed by 12 hex characters. See spec/fingerprint.md.
 */
id: string
role: string
/**
 * Accessible name. When nameIsTemplate is true the instances have different names and this is '*'.
 */
name: string
nameIsTemplate: boolean
/**
 * True when the name was seen to change while the page was read, as a countdown's or a cart total's does. The id then leaves the name out, so it stays the same from run to run. The name is the one read last.
 */
nameChanges?: boolean
confidence: Confidence
signals: string[]
/**
 * Readable structural path, for example 'main > list "products" > listitem'.
 */
path: string
/**
 * Path of a screenshot crop of the component, relative to this file. It is taken from the screen that showed the most of the component while the page was scrolled through. Absent when it never appeared.
 */
crop?: string
instances: Instance[]
}
/**
 * One actual element on the page.
 */
export interface Instance {
/**
 * The component id, a colon, and the instance's position in document order.
 */
key: string
name: string
tag: string
attrs: {
[k: string]: string
}
selectors: string[]
box: Rect
state: ElementState
frameId: string
inShadowDom: boolean
}

// Contract versions and method names.

export const CLIENT_PROTOCOL_VERSION = 1;
export const CLIENT_PROTOCOL_METHODS = {
  "engine.hello": {
    "params": "EngineHelloParams",
    "result": "EngineHelloResult"
  },
  "run.start": {
    "params": "RunStartParams",
    "result": "RunStartResult"
  },
  "run.cancel": {
    "params": "RunCancelParams",
    "result": "Empty"
  }
} as const;
export const CLIENT_PROTOCOL_NOTIFICATIONS = {
  "run.event": {
    "params": "RunEventParams"
  },
  "run.done": {
    "params": "RunDoneParams"
  }
} as const;

export const COLLECTOR_VERSION = 1;

export const COMMON_VERSION = 1;

export const DRIVER_PROTOCOL_VERSION = 1;
export const DRIVER_PROTOCOL_METHODS = {
  "driver.hello": {
    "params": "DriverHelloParams",
    "result": "DriverHelloResult"
  },
  "driver.shutdown": {
    "params": "Empty",
    "result": "Empty"
  },
  "session.open": {
    "params": "SessionOpenParams",
    "result": "SessionOpenResult"
  },
  "session.close": {
    "params": "SessionCloseParams",
    "result": "Empty"
  },
  "session.storage": {
    "params": "SessionStorageParams",
    "result": "StorageState"
  },
  "page.goto": {
    "params": "PageGotoParams",
    "result": "PageGotoResult"
  },
  "page.eval": {
    "params": "PageEvalParams",
    "result": "PageEvalResult"
  },
  "page.scroll": {
    "params": "PageScrollParams",
    "result": "PageScrollResult"
  },
  "page.screenshot": {
    "params": "PageScreenshotParams",
    "result": "PageScreenshotResult"
  },
  "page.act": {
    "params": "PageActParams",
    "result": "PageActResult"
  }
} as const;
export const DRIVER_PROTOCOL_NOTIFICATIONS = {
  "driver.log": {
    "params": "DriverLogParams"
  }
} as const;

export const RUN_FOLDER_VERSION = 1;
