# Features

What you can do with TraceQA, one line each. How to use each one is in [usage.md](usage.md); planned work is in
[backlog.md](backlog.md).

## 1. What the engine reads

- **Every interactive element**: buttons, links, inputs, selects, editable regions and ARIA widgets, plus elements that only look clickable (marked low confidence).
- **Names and states**: each element's accessible name and where it came from, and whether it is disabled, checked, expanded, selected, pressed, required or read-only.
- **Hidden and covered**: hidden elements are marked (or left out with `--visible-only`), and controls under a cookie banner or modal are marked covered.
- **The whole page**: open shadow DOM, same- and cross-origin iframes, and content that loads as the page scrolls.
- **Pages that load slowly**: waits for load, a quiet network and animations, with `--settle-ms` for splash screens.
- **Read-only**: it loads and scrolls pages and never clicks, types or submits.
- **Warnings**: says what it could not read or what looks wrong (closed shadow roots, cross-origin frames, failed requests, an error page, a sign-in page) without failing the run.

## 2. Components and ids

- **Components**: repeated elements become one component with instances, so twelve "Add to cart" buttons are one component.
- **Stable ids**: a component has the same id in every browser, at every screen size and on every run.
- **Repeated cards**: product cards and table rows collapse to one templated component.
- **Changing names**: counters and countdowns keep one id while their text changes.
- **Locators**: each component comes with a Playwright locator and whether it is unique on the page.
- **Pictures**: `--crops` saves a page picture and one picture per component.

## 3. Running

- **Browsers**: Chromium, Chrome, Edge, Firefox, WebKit, real Safari, a running browser over CDP, or any WebDriver server or grid.
- **Many at once**: every browser at every screen size in one run, side by side.
- **Robust runs**: one browser failing never stops the others, and a timed-out browser is retried alone.
- **Clean cancel**: Ctrl-C (or SIGTERM) stops quickly and leaves a valid run folder marked `cancelled`.
- **Signed-in pages**: `traceqa login` saves a sign-in once and `--storage-state` reuses it; the state is never written to a run folder.
- **Run folders**: `run.json`, `events.jsonl` and one `components.json` per browser and size, plus pictures, `axe.json` and `findings.json` when asked for.
- **Secrets redacted**: tokens, passwords and signatures in addresses are replaced by `REDACTED` in every file.
- **Progress**: every step is an event, written to `events.jsonl` and streamed live to the SDKs.

## 4. Command line

- **`traceqa extract`**: reads a page in the asked browsers and sizes and writes a run folder.
- **Stability check**: `extract --repeat N` reads a page N times and flags components that come and go.
- **`traceqa map`**: follows same-origin links and lists which components are on which pages; never follows logout or delete links.
- **`traceqa runs list`**: lists earlier runs, newest first.
- **`traceqa runs stats`**: shows runs and disk use per page.
- **`traceqa runs prune`**: deletes old runs, only with `--yes`.
- **`traceqa install`**: installs the browser drivers and browsers (and axe-core with `--axe`).
- **`traceqa doctor`**: checks the setup and says how to fix what is missing.
- **`traceqa login`**: signs in once in a browser window and saves the state.
- **`traceqa serve`**: the engine for the SDKs and other languages.
- **`traceqa mcp`**: the MCP server for AI tools (section 8).
- **CI output**: `diff`, `findings` and `verify` can print GitHub annotations and SARIF, and exit 1 when a gate fails.

## 5. Comparing runs

- **`traceqa diff`**: compares two runs by component (across browsers, sizes or builds): what was added, removed or changed.
- **`traceqa baseline save`**: keeps a run as a named reference to compare against.
- **`traceqa baseline promote`**: replaces the reference with a newer run, only with `--yes`.
- **`traceqa baseline alias`**: records that a component was renamed.
- **`traceqa baseline init`**: writes a `verify` manifest from a run.
- **CI gate**: `diff --against <baseline> --fail-on removed,disabled,...` fails the build on the changes you choose.

## 6. Findings

`traceqa findings` runs automated checks on a run. They are checks, not a conformance claim.

- **`unnamed-control`**: a control with no accessible name.
- **`weak-name-source`**: a name that comes only from a placeholder or title.
- **`fake-button`**: something clickable by mouse only, with no role or keyboard access.
- **`small-target`**: a target with a side under 24 px, too close to another.
- **`covered-at-load`**: a control covered when the page loads.
- **`ambiguous-names`**: links with the same text and different destinations.
- **`missing-autocomplete`**: a password, email or phone field without `autocomplete`.
- **`browsers-disagree`**: a component that differs between browsers at the same size.
- **axe-core**: with `extract --axe`, axe-core's rules are added, each placed on its component.
- **Suppressions**: a file of accepted findings, each with a reason and an optional expiry date.
- **Gate**: `--fail-on error|warning|info` fails the build on findings.

## 7. Tests from a run

- **`traceqa codegen`**: writes a Playwright page object (TypeScript or Python) with one property per component.
- **`traceqa heal`**: compares a run with a baseline and proposes locator fixes and renames; nothing is applied without `--accept`.
- **`traceqa visual`**: compares component pictures of two runs, in the same browser and size, to catch style changes.
- **`traceqa verify`**: checks a run against a manifest of what must and must not be on the page.

## 8. MCP server

`traceqa mcp` lets AI tools such as Claude Code and Cursor use TraceQA.

- **`extract`**: reads a page in real browsers and returns a run id.
- **`list_runs`**: lists earlier runs.
- **`get_components`**: lists a page's components, filtered and paged, with locators.
- **`describe_page`**: a short outline of a page by landmark.
- **`find`**: searches every browser of a run by role, name, text or state.
- **`diff_runs`**: compares two runs.
- **`findings`**: runs the automated checks.
- **`get_crop`**: returns a component's picture.
- **Safe by default**: page text is marked untrusted, cloud metadata addresses are refused, and remote browsers and sign-in states need the operator's permission.

## 9. Python SDK

- **`traceqa.extract(url, ...)`**: one call that runs an extraction and returns an `ExtractResult`.
- **`Engine`**: one engine for many calls, as a context manager.
- **Options**: browsers, sizes, pictures, hidden elements, timeouts, parallelism and a sign-in state.
- **Live events**: `on_event` receives each step as it happens.
- **Diff and findings**: `engine.diff` and `engine.findings`, without a browser.
- **Cancel**: `engine.cancel()` from any thread.
- **Errors**: `TraceQAError`, `EngineStartError`, `EngineExitedError` (with the engine's last log lines), `EngineRefusedError` and `EngineProtocolError`.

## 10. TypeScript SDK

- **`extract(url, options)`**: one call that runs an extraction and returns an `ExtractResult`.
- **`Engine.start()`**: one engine for many calls, several at once.
- **Options**: as in Python, plus `signal` to cancel with an `AbortSignal`.
- **Live events and logs**: `onEvent` and `onLog`.
- **Diff and findings**: `engine.diff` and `engine.findings`.
- **Typed**: every type comes from the contracts; `toTarget` and `BINARY_ENV` are exported helpers.
- **Requirements**: Node 24 or later, ESM, no runtime dependencies.

## 11. Other languages

- **Any language**: `traceqa serve` speaks a documented JSON-RPC protocol over standard input and output; see [clients.md](clients.md).
