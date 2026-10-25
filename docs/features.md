# Features

Everything TraceQA does today, one line each: what it does and how. Options, examples and exact formats are in
[usage.md](usage.md); designed but unbuilt work is in [backlog.md](backlog.md).

## Contents

1. [Reading a page](#1-reading-a-page)
2. [Components and ids](#2-components-and-ids)
3. [Warnings](#3-warnings)
4. [Running browsers](#4-running-browsers)
5. [Run folders](#5-run-folders)
6. [Browsers and drivers](#6-browsers-and-drivers)
7. [Safety and privacy](#7-safety-and-privacy)
8. [Command line](#8-command-line)
9. [Comparing runs](#9-comparing-runs)
10. [Findings](#10-findings)
11. [Tests from a run](#11-tests-from-a-run)
12. [MCP server](#12-mcp-server)
13. [Python SDK](#13-python-sdk)
14. [TypeScript SDK](#14-typescript-sdk)
15. [Client protocol and other languages](#15-client-protocol-and-other-languages)
16. [Contracts](#16-contracts)
17. [Setup, releases and project tooling](#17-setup-releases-and-project-tooling)

## 1. Reading a page

One collector (`collector/collector.js`) runs in every browser and every frame and reports facts only; the engine
does the rest.

- **Native controls**: buttons, links, inputs, selects, text areas, `summary` and image-map areas are listed with high confidence, recognised by their tag.
- **ARIA widgets**: elements with any of 17 interactive roles (button, link, checkbox, radio, switch, tab, the three menu items, option, textbox, searchbox, combobox, listbox, slider, spinbutton, treeitem) are listed with high confidence, from the `role` attribute.
- **Editable regions**: `contenteditable` elements are listed as text boxes with high confidence.
- **Elements that only look clickable**: elements made focusable with `tabindex` (0 or more), an `onclick` attribute or their own `cursor: pointer` are listed with low confidence; never inside a control that is already listed.
- **Confidence**: each element and component is `high` or `low`, low when it was found only by those weak signals.
- **Roles**: computed the way the accessibility tree does, explicit role first and native semantics otherwise (a link needs `href`, input types, landmarks), with `presentation`/`none` ignored where ARIA says so.
- **Accessible names with their source**: from `aria-labelledby`, `aria-label`, `<label>`, value, alt, content, title or placeholder, in that order, recorded as `nameSource`.
- **Name rules**: content text includes image alt and SVG titles, skips `aria-hidden` parts, collapses whitespace and is cut at 200 characters.
- **States**: visible, enabled, in viewport, checked, expanded, selected, pressed, read-only and required, from native properties first and `aria-*` attributes otherwise.
- **Hidden elements**: zero size, `visibility`, opacity 0, `content-visibility`, closed `details`, 1 px clips and `clip-path` insets mark an element hidden; hidden elements are kept and marked unless `--visible-only` is given.
- **Covered elements**: a control with something on top at all five test points is `covered`, with a selector for what covers it, using `elementFromPoint` through open shadow roots.
- **Structure path**: each control records its landmarks, regions, lists and repeated ancestors with their positions, which ids and locators build on.
- **Selectors**: each element gets a `data-testid` selector, a stable `#id` selector and a structural CSS path, skipping ids that look generated.
- **Copied attributes**: `id`, `data-testid`, `type`, `href`, `name`, `placeholder`, `tabindex`, `autocomplete` and `role` are kept for rules and locators and never feed the id.
- **Open shadow DOM**: read through and marked `inShadowDom`, with selectors written `host >>> inner`.
- **Frames**: every reachable frame is read, cross-origin ones included, and its elements are placed in page coordinates, with the frame's label in their path.
- **Tamper-proof reading**: the collector uses the browser's original built-in functions, captured before page scripts run where the driver allows, so a page that overrides DOM functions cannot change what is read.
- **Settling**: waits for the document, the load event, a quiet network (up to 3 s, Playwright browsers) and a stable height with no finishing animation (up to 5 s, 1.5 s after each scroll); `--settle-ms` adds time for splash screens.
- **Partial loads**: a page whose content arrived but whose load event did not is still read, with a warning; one with no content fails with `timeout`.
- **Lazy loading**: scrolls a screen at a time up to `--max-scroll-steps` (default 50) and waits at the bottom for infinite scroll to add more.
- **Two readings**: reads before scrolling and again from the top afterwards, so results do not depend on where scrolling stopped.
- **Changing names**: a control whose name changes between the two readings (a countdown, a counter) keeps one id.
- **Virtualised lists**: rows that a page removes while scrolling are detected and reported as a warning.
- **Failed requests and error pages**: failed scripts, styles, data requests and frames, and an HTTP error status, are counted and reported.
- **Viewport check**: when a browser cannot reach the asked size (Safari), the page is read at the real size and that is reported.

## 2. Components and ids

- **Components with instances**: elements with the same id become one component with numbered instances, so twelve "Add to cart" buttons are one component with twelve instances.
- **Stable ids**: `c_` plus 12 hex digits of a SHA-256 over role, normalised name and landmark path; position, size, classes, state, browser and viewport never count.
- **Same id everywhere**: the path starts at the innermost named ancestor, so ids match across browsers, runs and screen sizes when a responsive layout moves unnamed wrappers.
- **Name normalisation**: names are Unicode-normalised, trimmed, whitespace-collapsed and lower-cased before hashing.
- **Repeated cards and rows**: a slot that repeats with different names in three or more same-shaped cards or table rows becomes one component named `*`; navigation, menus and link lists are never collapsed.
- **Unnamed controls**: keyed by test id, stable id or position among unnamed siblings of the same card, so an icon button keeps one id per card position.
- **Fingerprint version**: every `components.json` records it, and the tools refuse to compare files of different versions.
- **Locators**: each component gets a Playwright locator, tried in order role with exact name, shared test id, role inside named ancestors, exact text, then a CSS selector marked forced; never `nth()`.
- **Locator uniqueness**: each locator is checked against every element the page had, with Playwright's matching rules, and marked unique, multi-match or ambiguous with the other ids.
- **Crops**: with `--crops`, each component gets a picture cut from the scroll-time screen that showed it best, plus a `page.png` overview; off by default, ids are the same either way.

## 3. Warnings

Warnings never fail a run; each is recorded with its target, and the open list can grow.

- **`unnamed_interactive_element`**: a control has no accessible name.
- **`collector_tampered`**: the page replaced built-in functions; names them and says whether the originals were used.
- **`cross_origin_frame_skipped`**: a frame from another origin could not be read.
- **`frame_eval_failed`**: a same-origin frame could not be read.
- **`scroll_limit_reached`**: the page was longer than the scroll limit.
- **`elements_removed_while_scrolling`**: a virtualised list removed rows that had been shown.
- **`requests_failed`**: scripts, styles, data or frames failed to load (Playwright browsers).
- **`http_error_status`**: the page answered with a 4xx or 5xx status.
- **`closed_shadow_roots`**: closed shadow roots hold content that cannot be read (Chromium family).
- **`page_load_incomplete`**: the load event had not fired when time ran out.
- **`covered_elements`**: something (a cookie banner, a modal) lies on top of some controls.
- **`session_not_accepted`**: a run with a saved sign-in still landed on a sign-in page.
- **`signed_in_pictures`**: pictures were taken while signed in and may show personal data.
- **`viewport_not_reached`**: the browser could not take the asked size.
- **`screenshot_failed`**: a screenshot failed; extraction went on without it.
- **`axe_failed`**: axe-core did not run on a frame or its results could not be written.
- **`browsers_disagree`** (run level): browsers found different components at the same size; names the missing ids per browser.

## 4. Running browsers

- **Browsers times viewports**: one run opens every asked browser at every asked size, each as its own target folder.
- **Parallel targets**: browsers run side by side, by default half the cores and at most 4, set with `--parallel`.
- **Retry alone**: a target that timed out while others ran is retried once on its own.
- **Failure isolation**: one browser failing never stops the others; the run is completed, failed or cancelled from its targets.
- **Crash containment**: a panic in one target fails that target, and the run still ends with a valid, closed folder.
- **One Safari at a time**: browsers a driver marks exclusive are never opened twice at once.
- **Clean cancel**: Ctrl-C, Ctrl-Break, SIGTERM, SIGHUP or `run.cancel` stops within about a second, keeps finished targets and marks the rest `cancelled`.
- **Quick quit**: a second signal closes the run folder and kills every driver within about 2 s; a stuck first signal does the same after 7 s, so `docker stop` finds a closed folder.
- **Whole process trees killed**: drivers and everything they started are killed (`taskkill /T` on Windows, a `ps` walk and SIGKILL on Unix).
- **Timings**: each target records how long opening, loading, reading, scrolling, screenshots and crops took; `--timings` prints them.
- **Progress events**: each step is an event, written to `events.jsonl` and streamed live to SDKs and MCP hosts in the same order.

## 5. Run folders

- **`run.json`**: the manifest: run id, address, times, status and one summary per target.
- **`events.jsonl`**: every event of the run, numbered from 0.
- **`components.json`**: one per target: components, instances, locators, warnings and the fingerprint version.
- **`page.png` and `crops/`**: the overview picture and one picture per component, with `--crops`.
- **`axe.json`**: axe-core results placed on component ids, with `--axe`.
- **`findings.json`**: written by `traceqa findings` next to each `components.json`.
- **`heal.json`**: written by `traceqa heal` in the run folder.
- **Group folders**: `runs/g_<id>/` holds `group.json` plus `stability.json` (`--repeat`) or `site.json` (`map`) over ordinary runs.
- **Sortable run ids**: `r_YYYYMMDD_HHMMSS_xxxx`; a folder is never shared between runs.
- **Atomic writes**: every JSON file is written to a temporary file and renamed, so a reader never sees half a file.
- **Safe reading**: readers refuse symbolic links, cap files at 64 MiB and accept only plain target folder names.
- **Lookup by any path**: tools accept a run folder, a target folder, a `components.json` path or a bare run id.

## 6. Browsers and drivers

- **Chromium, Chrome, Edge, Firefox, WebKit**: launched through the Playwright driver; `chrome` and `msedge` are the branded browsers, which `traceqa install` can also download.
- **Running browsers over CDP**: `cdp=ENDPOINT` attaches to a Chromium-family browser that is already running.
- **Real Safari**: `safari` drives Safari on macOS through Apple's `safaridriver`, which the driver starts and stops.
- **WebDriver servers and grids**: `webdriver=URL` opens a session on any W3C WebDriver server, such as chromedriver, geckodriver or a cloud grid.
- **Drivers are separate programs**: they speak a JSON-RPC protocol over standard input and output, so a driver can be written in any language.
- **Chosen by capability**: the engine holds no browser list; it asks each driver which browsers it opens and uses the first that can.
- **Driver discovery**: `--driver`, then `TRACEQA_DRIVERS`, then `PATH`, then `~/.traceqa/env`.
- **Self-healing driver pool**: a driver that times out or loses its pipe is replaced; one that is missing or of the wrong version is not retried.
- **Version handshake**: a driver of another protocol version is refused with `version_mismatch`.
- **Signed-in sessions in every driver**: both drivers start with the cookies and stored values of a state file, and save them back for `traceqa login`.
- **Driver capabilities**: drivers report what they can do (full-page and clip screenshots, frames, failed requests, waiting for promises), and the engine adapts.
- **WebDriver capabilities**: a target can carry `webdriverCapabilities`, passed to the WebDriver server as asked (through `serve` and the SDKs).

## 7. Safety and privacy

- **Read-only on pages**: TraceQA loads and scrolls pages and never clicks, types or submits; `map` only follows links with GET.
- **Address rules**: only `http` and `https` without a user name or password are accepted, parsed the way a browser parses them.
- **Secret redaction**: values of `token`, `api_key`, `password`, signatures and similar query or fragment keys become `REDACTED` in every run file, event, log line and driver error.
- **Sign-in state never stored**: a storage state is never written to a run folder, event, log or error message.
- **Owner-only login file**: `traceqa login` writes the state through a temporary file set to owner-only first (mode 600, or an `icacls` owner-only list on Windows).
- **Metadata addresses blocked**: in the MCP server, cloud metadata and link-local addresses are always refused, however they are written (decimal, hex, octal, IPv6-mapped).
- **Private networks on request**: in the MCP server, `--block-private` also refuses loopback, private, shared, unique-local and multicast addresses, and `--allow-host` exempts named hosts.
- **Redirects checked**: in the MCP server, every redirect hop, the address the browser connected to and the final address are checked too; a refused one fails the target before anything is read.
- **Untrusted page text**: page strings are marked as data for AI hosts and cleaned of control, invisible and bidi characters before they reach Markdown, annotations, SARIF, generated code or MCP answers.
- **Size limits**: request lines and run manifests (8 MiB), driver lines, `components.json` and baseline files (64 MiB), axe-core (8 MiB), names, attributes and answers are capped, so a hostile page or peer cannot exhaust memory.
- **Operator-only settings**: drivers, environment, folders and sign-in states are set by whoever starts TraceQA; an AI using MCP can only name a state file inside `--state-dir`.

## 8. Command line

- **`traceqa extract`**: lists every component of a page in the asked browsers and sizes and writes a run folder.
- **Browser and size options**: `--browser` (several, comma-separated, the same one twice), `--viewport WxH,...` (sides 1 to 10000) and `--headed`.
- **Patience options**: `--timeout-ms`, `--settle-ms` and `--max-scroll-steps` (0 reads without scrolling).
- **Output options**: `--out` folder, `--json` (the manifest alone on stdout), `--crops`, `--visible-only` and `--timings`.
- **Signed-in pages**: `--storage-state FILE` starts every browser signed in.
- **Accessibility scan**: `--axe` (or `--axe-file PATH`) runs axe-core on every page and frame and writes `axe.json`.
- **Stability**: `--repeat N` (1 to 20) reads a page N times and reports how often each component was found; `--fail-on-unstable` exits 1 when one varies.
- **`traceqa map`**: follows same-origin links a few levels deep and lists which components are on which pages and which are shared, in `site.json`.
- **Polite crawling**: depth, page, delay and time limits, robots.txt (RFC 9309, `--no-robots` to ignore), tracking parameters dropped, at most 20 pages per path pattern.
- **Destructive links skipped**: logout, sign-out, delete and unsubscribe paths are never followed, and `--exclude`/`--include` add more rules.
- **`traceqa runs list`**: lists runs and groups, newest first, optionally for one address.
- **`traceqa runs stats`**: shows per address how many runs there are, their statuses and their disk use.
- **`traceqa runs prune`**: deletes old runs by `--keep N` and/or `--older-than AGE`; prints a plan and deletes nothing without `--yes`, and never touches anything that is not a run.
- **`traceqa install`**: installs the drivers into `~/.traceqa/env` with hash-pinned dependencies, pinned to the binary's own commit, and downloads the named browsers (for `safari` it prints the one setting to switch on).
- **Install options**: `--ref` and `--from` choose the driver source, `--with-deps` adds Linux system libraries, `--dry-run` only shows the plan, and `--axe` fetches axe-core.
- **`traceqa doctor`**: shows the build, the home folder, each driver and whether each browser really opens, with the install command for anything missing; `--verbose` shows driver logs.
- **`traceqa login`**: opens a browser window, lets a person sign in and saves the state to a file; a state saved in one browser works in the others, and `--headless` saves once the page has loaded.
- **`traceqa serve`**: speaks the client protocol on standard input and output for SDKs, with `--max-runs` concurrent runs.
- **`traceqa mcp`**: runs the MCP server for AI tools (see [MCP server](#12-mcp-server)).
- **Analysis commands**: `diff`, `baseline`, `findings`, `codegen`, `heal`, `visual` and `verify` work on finished runs without a browser (sections 9 to 11).
- **Exit codes**: 0 success, 1 a target or gate failed, 2 a wrong command or unusable input with nothing run, 130 stopped by a signal.
- **Clean signals everywhere**: `extract`, `map`, `login`, `serve` and `mcp` share one signal handler and leave closed, valid folders.
- **Checks before running**: bad options, viewports, URLs and missing files exit 2 before any folder is made.
- **CI summaries**: with `--format github`, `diff`, `findings` and `verify` also append a Markdown summary to `$GITHUB_STEP_SUMMARY`.
- **Environment variables**: `TRACEQA_HOME`, `TRACEQA_DRIVERS`, `TRACEQA_UV`, `TRACEQA_AXE_FILE` and `TRACEQA_AXE_TARBALL_URL` move folders, drivers, `uv` and axe-core.

## 9. Comparing runs

- **`traceqa diff`**: compares two runs by component id (across browsers, sizes or builds) and lists what was added, removed or changed.
- **What counts as changed**: instance count, visible, enabled, covered, checked, expanded, selected, pressed and confidence; positions and pictures never.
- **Change flags**: each change says `became_hidden`, `became_disabled`, `count_up` and the like, for scripts.
- **Mode detection**: the report says whether it is cross-browser, cross-viewport or run-to-run and warns when sizes differ or little is shared.
- **Any two targets**: runs can be given as folders, `components.json` paths or run ids, and `--target`, `--target-a` and `--target-b` pick targets inside them; `--out` writes the report to a file.
- **Rename pairing**: a removed and an added component at the same place are linked as a role or name change, and never guessed when it is unclear.
- **Deterministic output**: the same inputs always give the same bytes.
- **Output formats**: Markdown (default), JSON (`DiffFile`), GitHub annotations and SARIF 2.1.0, with page text escaped and long tables cut.
- **`traceqa baseline save`**: keeps a run's components (and crops) as a named reference, by default in `.traceqa/baselines`, and refuses to overwrite one.
- **`traceqa baseline promote`**: replaces a baseline with a newer run only with `--yes`, prints the difference otherwise, and records each promotion.
- **`traceqa baseline alias`**: records that a baseline component was renamed to a new id, so diffs treat them as one.
- **`traceqa baseline init`**: writes a `verify` manifest from a run, per viewport.
- **Safe baselines**: written atomically, read without following links, names restricted to plain characters.
- **CI gate**: `diff --against <baseline> --fail-on removed,disabled,...` exits 1 when a listed kind of change appears; an unknown token is an error, so a typo cannot turn the gate off.

## 10. Findings

- **`traceqa findings`**: runs automated checks over a finished run (or one `--target`), prints them and writes `findings.json` unless `--no-write`; not a conformance claim.
- **Stable finding ids**: `<rule>:<componentId>`, the same across browsers and runs while the component is.
- **`unnamed-control`**: a visible control with no accessible name (WCAG 4.1.2).
- **`weak-name-source`**: a name that comes only from a placeholder or a title (WCAG 4.1.2, 3.3.2).
- **`fake-button`**: something clickable by pointer only, with no role or keyboard access (WCAG 4.1.2, 2.1.1).
- **`small-target`**: a target under 24 by 24 CSS px that fails the spacing exception (WCAG 2.5.8).
- **`covered-at-load`**: a control that something covers when the page loads.
- **`ambiguous-names`**: links with the same name and different destinations (WCAG 2.4.4).
- **`missing-autocomplete`**: a password, email or phone field without `autocomplete` (WCAG 1.3.5).
- **`browsers-disagree`**: a component present, visible, enabled or counted differently across browsers at one size, reported once for the run.
- **axe-core findings**: with `extract --axe`, every axe rule violation becomes a finding on the component it belongs to, with WCAG numbers and a statement of what automation cannot check.
- **axe-core is never bundled**: `traceqa install --axe` downloads a pinned version and checks its SHA-512, and it runs through the driver, so a page's content security policy cannot block it.
- **Suppressions**: a YAML file lists findings to ignore, each with a reason and an optional expiry date; expired and unused entries are reported.
- **Severity filter and gate**: `--min-severity` drops lower findings and `--fail-on error|warning|info` exits 1 when one is left.
- **Output formats**: table, JSON, Markdown, GitHub annotations and SARIF 2.1.0.

## 11. Tests from a run

- **`traceqa codegen`**: writes a Playwright page object, TypeScript or Python, with one property per component of a run.
- **Locators that hold in every browser**: a property is unique only when it is unique in every browser that has the component; otherwise it is marked.
- **Honest markers**: repeated components become multi-match locators, and unclear ones are marked `AMBIGUOUS`, `DIFFERS` or `FORCED`, never silently picked with `nth()`.
- **Frames in page objects**: components inside iframes are reached through a frame helper written into the file.
- **Escaped page text**: names, test ids and selectors only appear as escaped string literals, so page text cannot inject code.
- **`locators.json`**: a machine-readable list of every property, its locator and where it is unique, next to the page object.
- **`traceqa heal`**: compares a run with a baseline and sorts each lost component into a mechanical locator fix, a rename proposal, a removal, an ambiguity or a sign-in problem.
- **Rename evidence**: a rename needs the same role and landmark, plus the same test id or a close name and position, and exactly one candidate on each side.
- **Explicit acceptance**: `--accept ID` records the rename as a baseline alias; nothing is accepted automatically.
- **Page-object patching**: `--page-object` rewrites only the lines codegen wrote for that component, shows the diff first and keeps hand edits.
- **Refused in CI**: heal only reports in CI unless `--allow-in-ci` is given.
- **`traceqa visual`**: compares component pictures of two runs, or of a run and a baseline (`--against`), pixel by pixel, to catch style changes the structural diff cannot see; `--fail-on-different` makes it a gate.
- **Same browser and size only**: comparing across browsers or sizes is refused, because font and rendering differences would drown the signal.
- **Gated by structure**: only components that are structurally unchanged are compared; covered, hidden, renamed and name-changing ones are skipped with a reason.
- **Perceptual threshold**: anti-aliased pixels are ignored and thresholds match Playwright's screenshot comparison; differing components get a diff picture.
- **Browser versions recorded**: a different major browser version marks the comparison unreliable.
- **`traceqa verify --manifest`**: checks a run against a YAML manifest of what must exist, must not exist, be enabled or be hidden at each viewport, and which warnings are allowed.
- **Clear failures**: each failure names the check and, for a missing component, the nearest one that was found; typos in the manifest are errors.
- **Verify output formats**: text, JSON, GitHub annotations and SARIF 2.1.0; `--strict` also fails on extra components.

## 12. MCP server

`traceqa mcp` lets AI tools such as Claude Code run TraceQA over the Model Context Protocol (stdio).

- **`extract`**: opens a page in real browsers, headless, and returns a run id with a summary per target (no axe-core and no repeats over MCP).
- **`list_runs`**: lists earlier runs, newest first.
- **`get_components`**: pages through one target's components, filtered by role, name or landmark, or returns one component in full.
- **`describe_page`**: a short outline of a page grouped by landmark, with repeated cards shown once.
- **`find`**: searches every browser of a run at once by role, name, text, landmark or state, saying where each match is found and missing.
- **`diff_runs`**: compares two runs or two targets, with the same core as `traceqa diff`.
- **`findings`**: runs the automated checks on demand, filtered by target, severity or rule, writing nothing (without axe-core results).
- **`get_crop`**: returns a component's picture.
- **Resources**: runs, components and crops are also readable as `traceqa://` resources, for hosts that browse resources.
- **Concise or detailed answers**: `response_format` returns short rows by default and full detail on request, with a locator hint per component.
- **Structured answers**: hosts on MCP 2025-06-18 or later get output schemas and structured content; older hosts get plain text.
- **Narrowing hints**: long answers are paged and say how to narrow the query (by role, name, landmark, target or severity).
- **Progress, cancellation and ping**: `extract` reports progress when the host asks, a cancelled call stops its run, and `ping` is answered.
- **Protocol versions**: answers MCP 2025-11-25, 2025-06-18, 2025-03-26 and 2024-11-05.
- **Protections**: page text marked untrusted and cleaned, hidden elements off unless asked, answers and requests capped, no local paths revealed, remote browsers and sign-in states only when the operator allows them, metadata addresses always refused.
- **Operator options**: `--out`, `--state-dir`, `--allow-remote-browsers`, `--block-private`, `--allow-host`, `--max-concurrent-extracts` and `--suppressions`.

## 13. Python SDK

- **`traceqa.extract(url, ...)`**: one call that starts the engine, extracts and stops it, returning an `ExtractResult`.
- **`Engine`**: one engine process for many calls, used as a context manager; one run at a time per engine.
- **Extract options**: browsers, viewports, output folder, headless, crops, hidden elements, scroll steps, timeouts, settling, parallelism and storage state (a file, a dict or a `StorageState`).
- **Live events**: `on_event` receives each progress event as it happens.
- **`ExtractResult`**: the manifest, the run folder, the status, the targets, and `components()` returning the validated `components.json`.
- **`diff` and `findings`**: compare runs and run the checks through the engine, without a browser.
- **Cancel**: `engine.cancel()` from any thread ends the run as `cancelled` and `extract` returns; Ctrl-C inside `extract` cancels the run and re-raises `KeyboardInterrupt`.
- **Typed errors**: `TraceQAError` with `kind`, and `EngineStartError`, `EngineExitedError` (with `exit_code` and the engine's last stderr lines), `EngineRefusedError` and `EngineProtocolError`.
- **Quiet by default**: prints nothing; `on_log` receives the engine's log lines, which are always drained so the engine never blocks.
- **Finds the engine**: `binary=`, then `TRACEQA_BIN`, then `traceqa` on `PATH`.
- **Engine info and features**: `engine.info` holds the engine's version and features; `diff` and `findings` refuse with kind `unsupported` on an engine that lacks them, and `drivers=` passes driver commands.
- **Validated models**: every message is checked against pydantic models generated from the contracts, from the `traceqa-protocol` package the drivers share.
- **Requirements**: Python 3.10 or later; holds no logic of its own.

## 14. TypeScript SDK

- **`extract(url, options)`**: one call that starts the engine, extracts and always closes it.
- **`Engine.start()`**: one engine for many calls; several extractions can run at once on one engine.
- **Extract options**: the same as Python in camelCase, plus `signal` to cancel with an `AbortSignal`.
- **Live events**: `onEvent` receives each event in order; a handler that throws cancels the run and `extract` rejects with that error.
- **`ExtractResult`**: the manifest, run folder, status, targets and `components()`, validated at run time.
- **Helpers**: `toTarget` turns a browser string such as `cdp=ENDPOINT` into a target, and `BINARY_ENV` names the variable that points at the engine.
- **`diff` and `findings`**: the same queries as Python.
- **Typed errors**: the same classes as Python, with `exitCode` and `stderrTail`, and the cause kept.
- **Quiet by default**: `onLog` receives log lines; stderr is always drained.
- **Generated types**: every contract type comes from the JSON Schemas, under the `protocol` namespace.
- **Requirements**: ESM only (Node 24 can also `require` it), Node 24 or later, no runtime dependencies.

## 15. Client protocol and other languages

- **`traceqa serve`**: JSON-RPC 2.0, one message per line, over standard input and output; stdout is for protocol lines only.
- **Methods**: `engine.hello`, `run.start`, `run.cancel`, `engine.shutdown`, `diff.compare` and `findings.analyse`, with `run.event` and `run.done` notifications.
- **Version and feature handshake**: `engine.hello` refuses another protocol version and lists the features the engine has.
- **Several runs at once**: up to `--max-runs`, with a retryable `busy` answer beyond that.
- **Client crash safety**: if input ends without `engine.shutdown`, every run is cancelled and closed, so no browser is left behind.
- **Robust input**: unreadable, oversized and malformed lines get the standard JSON-RPC errors and the engine keeps serving.
- **Guide for other languages**: [clients.md](clients.md) gives the whole conversation, the rules and how to generate types.
- **Recorded conversations**: six transcripts in `spec/transcripts/` and a stand-in engine, `traceqa-replay`, let any client be tested without a browser.

## 16. Contracts

- **JSON Schema first**: every message and file is defined in `spec/*.schema.json`, with examples checked by the tests.
- **Generated types**: Rust (typify), Python (pydantic) and TypeScript types are generated from the schemas, and CI fails when any is stale.
- **Compatibility rules**: adding an optional field is compatible; anything else bumps the contract version, and readers ignore unknown fields.
- **Documented file formats**: run folders, diffs, findings, baselines, heal and visual reports, site maps and verify manifests each have a schema in `spec/`.
- **Id recipe documented**: [spec/fingerprint.md](../spec/fingerprint.md) defines the id, with shared test vectors for Rust and the collector.

## 17. Setup, releases and project tooling

- **TraceQA home**: drivers, their requirements and axe-core live in `~/.traceqa` (or `$TRACEQA_HOME`), apart from the user's own Python.
- **Release binaries**: a `v*` tag on a `master` commit builds Linux x86-64, macOS arm64 and Windows x86-64 binaries, smoke-tests them and publishes them as `.tar.gz` and `.zip` archives with the README and licence.
- **Verifiable downloads**: each release has `SHA256SUMS.txt` and signed build provenance attestations.
- **Release checks**: every package version must match the tag, and `cargo audit` and `cargo deny` must pass before anything is built.
- **Dependency hygiene**: a weekly audit of Rust, Python and npm dependencies, Dependabot updates with a 7-day cooldown, and every workflow action pinned to a commit.
- **CI on three systems**: every change is built and tested on Linux, macOS and Windows, with lint, type checks and the oldest supported Rust.
- **Demo site**: the Buggy Shop (`fixtures/buggy-shop/`) is a page with a known answer: shadow DOM, frames, lazy lists, hidden controls and more.
- **Bench**: `bench/run.py` extracts the Buggy Shop in every browser and size and scores found, extra, wrong and id stability.
- **Collector case pages**: small pages with expected answers run in Chromium, Firefox and WebKit.
- **Feature fixtures**: small sites with known answers for each feature (`axe`, `findings`, `heal`, `map-site`, `patterns`, `responsive`, `stability`, `trouble`, `visual`), used by the tests.
- **Generators and checks**: scripts regenerate and check the protocol types, the collector's pristine copies, the driver requirements, the transcripts and the version numbers.
- **Mutation testing**: `cargo mutants`, run by hand, checks that the tests catch changes to the id recipe.
