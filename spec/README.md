# spec

The contracts every part of TraceQA agrees on. These JSON Schemas are the
single source of truth: the Rust, Python and TypeScript types are generated
from them, so the sides cannot drift apart.

| File | Contract | Between |
|---|---|---|
| `client-protocol.schema.json` | Client protocol | A client (SDK, MCP server, hosted worker) and the engine |
| `driver-protocol.schema.json` | Driver protocol | The engine and a browser driver |
| `collector.schema.json` | Collector output | The in-page script and the engine |
| `run-folder.schema.json` | Run folder | The engine and anything that reads results |
| `diff.schema.json` | Diff output | `traceqa diff` and anything that reads its report |
| `visual.schema.json` | Visual comparison | `traceqa visual` and anything that reads its report. Pixels are compared only within one browser and viewport. |
| `baseline.schema.json` | Baseline | `traceqa baseline`, `traceqa diff --against` and anything that reads a baseline folder. Written only by `baseline save`, `promote` and `alias`, never by `extract` or `diff`; see [docs/usage.md](../docs/usage.md#baselines-and-the-ci-gate). |
| `heal.schema.json` | Heal report | `traceqa heal` and anything that reads `heal.json`. A query over a diff against a baseline: it proposes, and only `--accept` (refused in CI) writes an alias. See [docs/usage.md](../docs/usage.md#traceqa-heal). |
| `manifest.schema.json` | Manifest | `traceqa baseline init` and anything that compares a run with a list of expected components (the bench). The file is YAML; the schema describes the parsed document. |
| `findings.schema.json` | Findings | `traceqa findings` and anything that reads `findings.json`. Computed from `components.json` on demand, never written by `extract`. The suppression file (YAML, `.traceqa/suppressions.yaml`) is described in [docs/usage.md](../docs/usage.md#suppressions): entries of `id` (or `rule` and `component`), a required `reason` and an optional `expires`. |
| `common.schema.json` | Shared types | Used by the files above |
| `fingerprint.md` | Component id recipe | The engine and anything that compares runs |
| `fingerprint-vectors.json` | Shared test vectors for the recipe | The engine's unit tests and the collector test (`tests/test_fingerprint_vectors.py`) |
| `transcripts/` | Recorded client conversations | Every client is tested against them. See `transcripts/README.md`. |

Every `$ref` must be `#/$defs/Name` or `file.schema.json#/$defs/Name`, and each `$defs` name
must be unique across files: the three generators flatten refs to the last segment.
`scripts/check_spec_refs.py` enforces this in CI.

A `Viewport` side is 1 to 10000 (`common.schema.json`). The engine enforces it with one check,
`Viewport::check`, for every way a viewport arrives; the generated Python and TypeScript types
carry the bounds, and `crates/traceqa-common/build.rs` leaves them out of the Rust types
(typify would make the field `NonZero<u64>`), where a test keeps the schema equal to
`MAX_VIEWPORT_SIDE`.

## How messages travel

Both protocols are JSON-RPC 2.0, one message per line.

- **Client protocol:** the client starts the engine (`traceqa serve`) and writes
  requests to its stdin. The engine writes responses and notifications to its
  stdout. A client that closes on purpose sends `engine.shutdown` first; end of
  input without it means the client died, and the engine cancels its runs. The
  method was added without a version bump: it is additive, and an older engine
  answers `-32601`, which clients ignore.
- **Driver protocol:** the engine starts the driver and is always the caller.
  The driver only sends responses and `driver.log` notifications.

Stderr is free for human-readable logs on both sides.

Limits on what a driver sends:

- A line from the driver may be at most 64 MiB (`MAX_DRIVER_LINE` in the engine).
  The engine reads it with a bounded reader (`jsonrpc::read_line_bounded` and
  `read_line_or_give_up`), so a driver that never sends a newline cannot use up
  the engine's memory. A longer line, or a line that is not UTF-8, makes the
  driver count as lost: the target fails with a short message and a fresh driver
  is started for the next one.
- At most 64 driver messages wait to be read. Past that the engine stops reading
  the driver's pipe, so a driver that floods notifications is slowed down, not
  buffered.
- Error messages that quote a bad line show at most 200 characters of it.
- A client request line longer than 8 MiB (`MAX_REQUEST_LINE`) is skipped and
  answered with code `-32600` and a `null` id; the engine reads the next line.
- Error codes for a line that is not a request, as in JSON-RPC 2.0: text that is
  not JSON (or not UTF-8) is `-32700` with a `null` id. JSON that is not a
  request object (a missing or wrong `jsonrpc`, no `method` or `id`, a `method`
  that is not a string, a number, `null`, or an array including `[]`) is
  `-32600`, with the request's `id` echoed when it is a non-negative whole
  number or a string, and `null` otherwise. Batches are not supported: an array
  is `-32600`. The next line is read as usual.
- At most `--max-runs` runs go at once (default 4). A `run.start` beyond that is
  answered with an error whose `data.kind` is `busy` and `retryable` is true.
- A driver's stderr is inherited, not read by the engine: it is the driver's own
  log channel and is never buffered or stored.

- **Failed requests:** a driver whose capabilities say `failedRequests: true`
  (optional; absent means false) adds `requestsFailed` to every `page.eval` result:
  `count` and the first five `urls`, with query and fragment removed by the driver.
  It is cumulative since the last `page.goto` began and counts only scripts,
  styles, fetch/XHR and frames, never pictures, fonts or navigation-cancelled
  requests. The engine turns a non-zero count into the `requests_failed` warning.
  The WebDriver driver cannot observe the network, so it sends neither.

- **Waiting for a Promise:** a driver whose capabilities say `evalAsync: true` (optional; absent means false) accepts
  `awaitPromise: true` in `page.eval`: the function may return a Promise, and the driver waits for it, up to `timeoutMs`,
  and gives the resolved value. A rejection is that frame's `eval_failed`. Playwright always waits; the WebDriver driver
  uses `/execute/async` for it, because `/execute/sync` cannot wait. The engine needs this for `--axe` (axe-core is asynchronous).

- **Actions:** a driver whose capabilities list `actionKinds` (optional; absent or empty means none) accepts `page.act`:
  one `click`, `hover`, `type`, `select` or `press` on the element `target.selector` names, in `target.frameId`
  (default `main`; only a frame id the engine has already been given by a `page.eval`). A selector written inside shadow
  DOM as `host >>> inner` is accepted. `type`, `select` and `press` need `value`; `select` takes an option's value or
  its label. `timeoutMs` (default 10000) limits the wait for the element and the action itself. The reply says what the
  action did: `url` after it, `navigated` (with the main document's `status` when the driver sees it; a download is not
  a navigation), `popups` (the driver closes them; one the policy stopped is named by the address it was going to), `dialogs` (the driver dismisses them), `downloads` (the driver
  cancels them: no file is stored), `consoleErrors` (`uncaught` or `console`), `failedRequests` (status 400 or more,
  or the browser's reason, at most 50), `domChanged` and `settle`. `domChanged` is judged against what the page changes by
  itself: the driver watches the page at rest for 0.3 s before the action, and counts a change only when some kind of
  change (elements added or removed, text, attributes, or a style that shows or hides) happens more than three times as
  often during the action. A style that only moves or fades is not counted. A page that answers the first click anywhere on it (starting music, a banner) shows that answer in whatever action comes first on it. Measured on a real site: it is quiet at rest,
  and clicking plain text made 28 style changes (parallax, answering the mouse), while opening a menu also added
  elements and changed classes, so counting every change said "changed" for a click that did nothing. Every address and message in it is cut as the
  driver's errors are: no query, no fragment, at most 500 characters. A caller must not read an empty list as "nothing
  happened" for a kind the session does not list in `eventKinds`, and no reader may depend on the difference between an
  absent and an empty list. Errors: `element_not_found` (nothing matches within the time), `timeout` (the element was
  found but the action did not finish, for example a disabled button), `invalid_params` (a selector that matches several
  elements, an unknown frame, a missing `value`, a session opened without a policy), `session_not_found`, `unsupported`.
  A `timeout` that the driver itself reports for `page.act` (a covered control never becomes clickable) is an ordinary
  result: the engine keeps the driver and its sessions. Every other `timeout`, and an engine that gave up waiting for the
  driver, still marks the driver lost and replaces it.
  The WebDriver driver lists no actions yet.

- **Request counts:** in a session opened with a `policy`, `page.goto` and `page.act` replies carry `requestCounts`: how
  many requests the session's browser has sent to each origin since the session was opened, up to and including that
  call. A request the policy stopped never left the browser and is not counted, and neither is one for an address with
  no origin (`data:`, `blob:`). The explorer adds up the counts of its sessions to enforce `--max-requests-per-origin`.
  What the page requests by itself after the last call (a timer, a lazy load) shows up in the next call's counts, so a
  walk can go over its limit by what one step costs. A session without a policy reports none, and so does any driver
  that cannot count (the WebDriver driver); the explorer then does not enforce the limit and says so in its progress.
  An empty map and an absent one mean the same to a Rust reader.

- **Safety policy:** `page.act` only works on a session opened with `session.open`'s `policy` (`ActPolicy`: `allowOrigins`,
  at least one `http` or `https` origin, and `allowWrites`, false when absent). The driver enforces it on the browser's own
  requests, so it does not depend on what a button is called or on anything the page says. Unless `allowWrites`: every
  request whose method is not GET or HEAD is aborted (form posts, `fetch`, XHR, `sendBeacon`), and every WebSocket is never
  connected to its server and is closed. Always: a navigation of the top-level page to an origin that is not in
  `allowOrigins` is aborted before the request is made. A blocked request is reported in the action's `failedRequests` with
  the failure `blocked_by_policy`. The reply to `session.open` says the policy back (`policy`), and a caller that did not
  get the policy it asked for must close the session: a driver that does not know about policies says nothing. A driver
  that cannot keep a policy (the WebDriver driver, or a browser reached over CDP whose context the driver did not create)
  refuses to open a session that asks for one with `unsupported`; a policy no one can read (no origin, an origin that is
  not http or https) is `invalid_params`, before any browser starts.
  **Limits, all measured in Chromium, Firefox and WebKit:**
  - A redirect to another origin cannot be stopped: the browser follows a redirect without asking Playwright, so the
    request to the other site is made. The driver notices where the page ended, reports it as blocked and goes back. This
    is the same limit as the operator's address rules in `docs/usage.md`. A redirect that keeps the method (307, 308) of
    a request that read-only mode let through is a GET, so only a read can reach the other site.
  - Going back is only done when the page really moved. Aborting a navigation leaves Chromium on an error page and leaves
    Firefox and WebKit on the page; the state of the page the action started from (an open menu, typed text) is lost in
    Chromium when the page is restored.
  - Playwright's `service_workers="block"` does not stop a page from registering a service worker in any of the three
    browsers, and a service worker's own requests do not pass through the driver's routes. The driver therefore hides
    `navigator.serviceWorker` from every page. That is not enforced by the browser: a dedicated worker that a page starts
    does not run the driver's script, so a service worker registered from one is not covered.
  - Images, scripts and other subresources from other origins are not stopped (pages would not render); only navigations
    are checked against `allowOrigins`.
  - A GET can still have an effect on a site (`/logout`, `/delete?id=3`). That is why the engine's `policy` module also
    skips components by name and by address.

The method names, with their parameter and result types, are listed in the
`x-traceqa` block at the top of each protocol file.

## Addresses in the run folder

The browser is always sent the address as given, but nothing the engine saves or
reports holds a secret from it. `run.json`, `events.jsonl` and `components.json`
(`url`, `finalUrl`, the `page.opened` event and warnings that name a frame) carry
the address with the value of these parameters replaced by `REDACTED`, in the
query and in the fragment (where OAuth implicit flows put `access_token`). Names
are compared after percent-decoding, in lower case, with `-` read as `_`:

`token`, `access_token`, `id_token`, `refresh_token`, `api_key`, `apikey`,
`secret`, `client_secret`, `password`, `passwd`, `sig`, `signature`,
`x_amz_signature`, `x_amz_credential`, `x_amz_security_token`.

An address without any of them is stored byte for byte as given. An address with a
user name or password (`https://user:pass@host/`) is refused before a run folder is
created. The list is `SECRET_KEYS` in `crates/traceqa-engine/src/url_safety.rs`.

Text that comes from a driver gets the same treatment, because a browser's error
text quotes the address it failed to open (`Page.goto: net::ERR_X at https://h/p?token=...`).
The engine passes every `http://` or `https://` address found in a driver's error
message, in a `driver.log` line it echoes to stderr, and in its own report of a
broken driver, through the same redaction. This is done in the engine, so a driver
written by someone else is covered too, and the key list exists once. The values
of `run.json` target errors, `events.jsonl` and the MCP `error` fields therefore
never hold a secret query value that a driver quoted.

## Redirects and operator address rules

`PageGotoResult.redirectChain` lists every request that led to the page, oldest
first and `finalUrl` last, each with the IP address the browser connected to
(`remoteAddress`) when the browser reports it. It is optional: a driver that
cannot see the hops (classic WebDriver shows only the final address) leaves it
out. The engine checks each hop against the address rules of the MCP server
(`traceqa mcp --block-private`, see `docs/usage.md`). A refused hop fails the
target with the `blocked_url` error kind (not retryable), and nothing from the
page is written.

## The diff

`traceqa diff` joins two `components.json` files on the component id. The id hashes version, role,
name and path, so a change of role or name is a different id: it is reported as one removed and one
added entry that point at each other (`pairedWith`), never as `changed`. Ids of different
`fingerprintVersion`s are never compared. A diff has no timestamp of its own and a fixed entry
order, so the same two inputs give the same bytes.

## Run folder layout

```
runs/<runId>/
  run.json                  RunManifest
  events.jsonl              one Event per line
  <browser>-<width>x<height>/
    components.json         ComponentsFile
    axe.json                AxeResults                (only with `extract --axe`)
    page.png                screenshot of the page    (only with pictures)
    crops/<componentId>.png one crop per component    (only with pictures)
```

`traceqa explore` writes a run folder of its own (`feature` is `explore`): `run.json` with no targets, `events.jsonl`, and
`graph.json` (`GraphFile`, `graph.schema.json`). There is one graph for the walk and not one reading per browser, so
there is no browser folder. `graph.json` is written whole or not at all (written aside and renamed), and is there
whenever the walk got as far as starting: after a cancel too, with `stopped` set to `cancelled`. After a second
Ctrl-C that cannot wait for the browser the folder is closed (`run.json` says `cancelled`), and `graph.json` may be
missing.

```
runs/<runId>/
  run.json                  RunManifest               (feature explore, targets [])
  events.jsonl              one Event per line        (progress lines are type "progress")
  graph.json                GraphFile
```

Given the same site, seed and limits, `graph.json` is byte-identical from run to run: keys are sorted, lists keep the
order things happened in, and it holds no time. Screens are told apart by `traceqa-engine`'s `state` module (the
address without fragment and volatile parameters, the open dialog, and at least 90% of the same component ids once the
ids that are on every page are left out). A dialog has no component id of its own, so a screen with one open names it
by role and name (`dialog "Help"`), taken from the path of the components inside it.

A run group is an index over ordinary runs, not a nested layout. `traceqa extract --repeat N`
writes its N runs as normal sibling folders and one group folder next to them:

```
runs/g_<id>/
  group.json                RunGroup (kind: repeat; later also site)
  stability.json            StabilityFile (repeat groups)
```

A group folder has no `run.json`, so everything that lists runs by `runs/*/run.json` (MCP
`list_runs`, clients, the run-folder checker) skips it and sees the member runs unchanged.
`traceqa runs prune` removes a group folder when none of its runs is left.

Status of a stopped run: a cancel (Ctrl-C, `run.cancel`) turns every target that
was in flight, and every target not yet started, into `cancelled`, also when the
cancel took the target's browser or driver down first. A cancelled target has no
`error`; the driver's words, when there are any, are in a `target.cancelled`
event. The run is `cancelled` if any target is, else `failed` if any failed. A
run that is forced to quit (a second Ctrl-C) is still closed: `run.json` says
`cancelled` with `finishedAt`, and `events.jsonl` ends with `run.finished`.

## Locator

`Locator` (in `common.schema.json`, used by `Component.locator`) describes how to find a
component again with Playwright locators. It is computed per target, so its counts
(`matches`, `own`) and `unique` only cover the elements of that one file; "unique across
browsers" is decided by `traceqa codegen`, which reads the files of every target. It follows
Playwright's matching: a name is `exact` (whitespace squashed, case kept) and hidden elements
are not found unless `needsIncludeHidden`. It never contains an index (`nth`): a component
that cannot be told apart is `unique: false`, or `forced` with a CSS selector. Every string
in it is page text; code that turns it into source must escape it
(`traceqa_common::locator::ts_string` / `py_string` do). The field is optional, so runs
made before it exist stay valid; `traceqa codegen` asks for those to be extracted again.

## Rules for changing a contract

1. **Adding an optional field is compatible.** No version change. Readers must
   ignore fields they do not know.
2. **Anything else is breaking:** removing or renaming a field, making a field
   required, changing a type or a meaning. Raise `x-traceqa.version` in that file.
3. **Open lists can grow.** Fields documented with "Known values" (event types,
   warning codes, browser names) are plain strings. Readers must tolerate values
   they have not seen.
4. **Closed lists are enums** (`Confidence`, `RunStatus`, `WaitUntil`,
   `ActionKind`). Adding a value is breaking.
5. **Type names are unique across all files.** They share one namespace in the
   generated code.
6. **Every change needs an example.** `examples/<TypeName>.<label>.json` is
   checked against `TypeName` by both the Rust and the Python test suites.

## After editing a schema

```
cargo test                               # Rust types regenerate on build
uv run scripts/gen_python_protocol.py    # rewrite the Python types
uv run scripts/record_transcripts.py     # after a client protocol change
uv run pytest
```
