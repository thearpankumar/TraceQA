# Backlog

Work that is designed but not built. Where an item builds on code that already exists, it names that code;
everything else it describes does not exist yet. `LIM-` items are limits of features that shipped, and CUT-8
records a refusal that the code already enforces. Each item says why it matters, how it should work and how to
tell that it is done, so that someone can pick it up without asking.

## 1. The explorer (F6)

### What it is

`traceqa explore <url>` walks a site by interacting with it, and reports what it touched and what broke. It is a
deterministic state-graph walker keyed by component ids, not an agent. Given the same site, the same seed and
the same budgets it makes the same moves and writes the same graph. A language model is used only to invent
form data and, optionally, to judge an outcome (EXP-6). The result is a graph of states and transitions plus a
coverage figure per state: components touched divided by components discovered.

It differs from existing tools in what it records. Autonomous browser agents report "task done" and have no
notion of coverage. Playwright's planner explores once and writes Markdown. The explorer writes a reproducible
graph that can be replayed (EXP-8) and handed to code generation.

### How it fits together

1. The walker (EXP-2) picks a state and a candidate component.
2. The safety policy (EXP-5) decides whether the action is allowed.
3. The driver performs it through `page.act` (EXP-4) and returns a rich result.
4. The engine extracts the page again, incrementally where it can (EXP-9), and computes the new state (EXP-1).
5. The edge is recorded in `graph.json` and `events.jsonl` (EXP-7); oracles (ORA-1 to ORA-8) look at the result.

### EXP-1: State model and state equality

**Why.** The graph is only useful if two visits to the same screen count as one state, and two different screens
do not merge. Hashing the whole DOM treats every ticking clock as a new state. Comparing trees does not scale.
Crawljax, the best-known crawler of this kind, reports that exact DOM comparison is too strict in practice and
that tree edit distance is too slow, which is why it supports configurable comparators.

**Design.** A state is the tuple
`(normalised URL, set of visible component ids, modal marker, frame set)`.

- The URL is normalised: fragment dropped, volatile query parameters removed (a configurable list, with
  defaults such as `utm_*`, `_`, `ts`, `cache`), remaining parameters sorted.
- The component id set is the ids of visible components in `components.json`, with the site chrome factored out.
  Chrome means ids shown on at least a configurable share of the pages (default 0.8), such as the header and
  footer links. Today `site.json` from `traceqa map` lists as `shared` only the ids on every page read, so the
  share has to be added to `map`. Chrome ids are tracked once, globally, and not per state.
- The modal marker is the id of the open dialog (`role=dialog` or `aria-modal`) or empty.
- The frame set is the list of frame ids that are attached.
- Two states are equal when their URLs are equal, their modal markers are equal and the Jaccard similarity of
  the id sets is at least a threshold (`--state-similarity`, default 0.9). A new state is compared to existing
  states with the same URL and modal marker only, so the comparison stays cheap.
- State ids are `s` followed by the order of first discovery, so they are deterministic.

**Acceptance.**
- Unit test: two id sets of 20 ids that differ in 1 id are equal at the default threshold; sets that differ in
  3 ids are not.
- On the buggy shop, loading the home page twice produces one state, with a cookie banner open or closed.
- On the buggy shop, the product list and the cart page produce two states.
- `graph.json` contains no state whose id set is empty, and no two states with equal URL, modal marker and a
  Jaccard similarity at or above the threshold.
- Changing `--state-similarity` to 1.0 on a page with a rotating banner produces more states than at 0.9.

**Size.** M

**Status (branch `backlog/exp-1-screen-identity`).** Built as a module with no command and no output file:
`crates/traceqa-engine/src/state.rs` (`Screens::observe`, `clean_url`, `chrome_ids`, `Similarity`). A look is taken for the screen it
is most alike (the earliest of equals) among those of the same cleaned address and dialog, or becomes the next `s<n>`; the
share is kept in thousandths and compared with whole numbers; a screen keeps the ids of its first look; frames are kept and not
compared; an address that cannot be read, is not http or https, or is over 4096 characters, and a look with no component, are
errors. Tests: 29 unit tests, and 7 on ids the real engine found (`tests/data/screen_ids.json`, made by
`scripts/capture_screen_ids.py` from the Buggy Shop and `testbed/sites/patterns`; the test says when the fingerprint version
has moved on), mutation testing on `state.rs` with nothing missed. Measured on the real ids: the shop read twice is one screen
(35 of 35); the shop with and without its two hidden controls is one (35 of 37, 94.6%); the shop on a phone is another screen at
90% and the same at 85% (31 of 36, 86.1%); no two pattern pages share an id.
Not done: the shop has no cookie banner, so "banner open or closed" is shown with its two hidden controls instead, and "the
product list and the cart are two screens" needs the walk to reach both (EXP-2, EXP-10); `--state-similarity` and `graph.json` do
not exist yet. Known limits, from the module notes: small pages (one change is a big share), addresses with an id in them
(`/product/123`), and lists that grow after the look. Tried on the real pages of two sites (read-only extractions, 9 and 8 looks): the home page read twice was one screen and every other page its own,
with overlaps between different pages of 10% to 44%, so the 90% line was never close. A same-address case with a large shared header is covered by
unit tests only; no real site gave one.

### EXP-2: Walk algorithm and budgets

**Why.** A walker without limits never finishes on a large site, and one that picks actions in random order
cannot be debugged.

**Design.**
- Seed: the start URL, plus the storage state if `--storage-state` is given (as in `extract`).
- Worklist of (state, component) pairs. Breadth-first by default, depth-first with `--strategy dfs`.
- For a state, the candidates are the visible, enabled components not yet tried in that state. They are
  ordered by (component path, component id), so the order depends only on the page.
- One action kind per role: `click` for link, button, tab, menuitem, checkbox, radio, switch and disclosure;
  `type` for textbox and searchbox with generated data (EXP-6); `select` for combobox and listbox; `press`
  (Enter, Space, Escape) as a keyboard fallback when a click is blocked.
- After each action: settle (reusing the drivers' settle logic), extract (EXP-9), compute the state (EXP-1),
  record the edge `(state, component id, action) -> state'`.
- The seed (`--seed`, default 0) only drives generated form data and tie-breaks. It is recorded in `graph.json`.
- Budgets, all with defaults and all recorded in `graph.json`: `--max-states` (50), `--max-actions` (300),
  `--max-depth` (6), `--action-timeout` (10 s), `--timeout` for wall-clock time (10 min) and
  `--max-requests-per-origin` (2000). When a budget is hit the walk stops cleanly and `graph.json` says which
  budget ended it.
- Cancel works as for `extract`: the first Ctrl-C or SIGTERM stops after the current action and closes the
  folder as `cancelled`; a second signal kills the driver. The folder always validates.

**Acceptance.**
- Two runs on the buggy shop with the same seed and budgets give byte-identical `graph.json` (compare with
  `cmp`).
- With `--max-actions 20` the run performs 20 actions or fewer, and `graph.json` names `max-actions` as the
  stop reason. The same holds for each of the other five budgets, one test each.
- Ctrl-C during a walk leaves a folder that passes the run-folder schema check, with status `cancelled`, and the
  command exits with code 130.
- Candidate order: a test that shuffles the extraction order of a fixture page gives the same action sequence.

**Carried forward.** From EXP-5 (safety), which built the rules but nothing that calls them: the `explore` command must (1) refuse to start on an address that `policy::is_local_origin` says is not local, with exit code 2 and no browser opened, unless `--i-understand-this-interacts` is given; (2) take `--allow-writes` (which requires `--allow-origin`), `--allow-origin` and `--deny-name`; (3) open its session with `policy::Rules::driver_policy()` and close the session when `policy::enforced` says the driver did not say the same policy back; (4) ask `policy::Rules::decide` before every action and never act on a `Skip`. Nothing else calls `page.act` yet.
From EXP-1 (screens), which built the rule and nothing that supplies its inputs: the walk must (5) take `--state-similarity` (`state::Similarity::new`, default `DEFAULT_SIMILARITY`); (6) give `state::Screens` the page-wide ids (`state::chrome_ids`, from the ids shown on at least 80% of the pages read). `traceqa map` lists only the ids on every page today, so it needs that share added first, or the walk reads a few pages itself; (7) build a `Look` from each extraction (the ids of the components with a visible instance, the open dialog's id, the frame ids) and treat an `Err` from `observe` as a failed read, not as a screen; (8) measure the known limits on real sites (address ids, small pages, growing lists) and tune the share with those numbers.

**Size.** L

**Status (branch `backlog/exp-1-10`).** Built: `traceqa explore` (`crates/traceqa-cli/src/explore.rs`) and the walk in `crates/traceqa-engine/src/explore/`
(`walk.rs` the worklist, order, budgets and safety; `live.rs` the real browser; `settle.rs` waiting for a page to stop appearing; `runner.rs` the run folder and
the abort watcher; `graph.rs` the file; `data.rs` the typed values; `model.rs` the `Browser` trait the walk is written against), `spec/graph.schema.json`
(`GraphFile`), the fixture `testbed/sites/explore` and `tests/test_explore.py`. The run folder holds `run.json` (feature `explore`, no targets),
`events.jsonl` (progress lines) and `graph.json`. All six budgets exist and are written into `graph.json` with the reason the walk stopped; Ctrl-C leaves
a valid folder (`cancelled`, exit 130) and so does a second one.
Of the carried-forward lines: (1) to (4) are done (a non-local address, or an `--allow-origin` that is not local, exits 2 with no driver started and no folder;
`policy::Rules::with_origins` and `policy::check_start` are new for it; `live.rs` closes the session when the driver does not say the policy back, which no test
has tried with a driver that fails to; `decide` runs before every action and again on the element that is acted on). (5) `--state-similarity` is taken. (6) The ids that are on every page come from a short read
before the walk (up to `--chrome-pages` links the rules allow, default 6, opened and never clicked), not from `traceqa map`; those controls are tried once, on
the first screen that has them. (7) A `Look` is built from every reading; a dialog has no component id, so a screen with one open is named by the dialog's role
and name, taken from the path of the controls inside it; an `Err` from `observe` is a failed read (status `failed`), not a screen. (8) Measured on four real
sites, see below.
Choices made where the item was silent: `action-timeout` as a stop reason means five actions in a row that used up all their time; a click that fails or is
blocked is tried once with a key (`press`, Enter or Space) and, if that fails too, the click's own failure is the one reported; a screen that is opened
again is tried twice before it is given up, and counts as the screen when it is alike by the rule, not when it is the closest of all; typing happens only
with `--allow-writes`; selects are left alone (`no-options`) because their options are not known; the page is read after every load and action until two
readings agree to 90% (at most 6 readings); every action starts from a new browser session.
Changes to earlier items that this needed: the driver counts requests per origin (`requestCounts` in `page.goto` and `page.act`); a `timeout` that the driver
itself reports for `page.act` no longer makes the engine replace the driver (it took the session with it); `extract` got `read_live`, a single reading of the page
a session is on; `state::Screens::matches`. And for PR #41 (EXP-4): the driver waits up to 3 s for the `download` event once a download response has arrived and
listens for downloads on new tabs, and one test that makes eight actions in a row has 5 s per action. Those two fixes are a best reading of the CI logs (WebKit on
Linux and Windows); they could not be reproduced here.
Tests: 63 unit tests in the explore module, 20 real-browser tests in Chromium, Firefox and WebKit (a read-only walk of the fixture; the same bytes twice; each
budget; writes; the refusals; Ctrl-C twice), mutation testing with nothing missed in `policy.rs`, `graph.rs`, `data.rs`, `settle.rs` and `state.rs`, and two
survivors in `walk.rs` (`depth < max_depth` when a page-wide link is marked done: a page-wide link is on a shallow screen first, so no test short of a contrived
site tells them apart).
Tried read-only on four of the five sites in the notes (the fifth, `avqentra.in`, refused secure connections while this was done, for `extract` too). Two
sites: 8 screens in 10 to 13 actions and the same bytes on a second run. The two single-page portfolio sites: 14 actions each, the same bytes on one and nearly
the same on the other. The trial found and fixed four things: the walk acted in a session that a failed page read had closed; a screen was given up after one
reload that looked different; a page was read before it had finished appearing (the same fresh load showed 23, 26 or 32 controls); and the question asked of a
reload ("is it this screen by the rule") was "is this the closest screen".
Not done, and limits: `traceqa explore` cannot run in Safari (the WebDriver driver lists no actions) and does not enforce `--max-requests-per-origin` with a
driver that cannot count; Escape is not used as a fallback; clickable elements with no role (`generic`) are not acted on; `coverage.json` and the named
`explore.*` events are EXP-7; after a second Ctrl-C that cannot wait for the browser the folder is closed but `graph.json` may be missing. Measured: a new session per action means nothing is
cached, so one action cost about 25 to 37 requests on `t3aiworks.com` and took about 18 s on the two 3D portfolio sites (the page read and the settle are most
of it). Not verified: Linux, Windows, Rust 1.88, and the CI fix above.
For other items (not fixed here): EXP-1, on a single-page site whose sections fade in on scroll the same address shows as several screens (30 to 57 controls,
53% to 88% alike), so no share would merge them; a small page whose controls change on every load is split (`t3aiworks.com` home 0.875), and a control whose
name changes has a new id on every visit, so it is `missing` when the walk comes back for it (the settle of `extract` that marks such names is not in the live read);
EXP-3, a screen that is only reached by a click or a scroll (a dialog, an open disclosure, a scrolled page) is listed in `unreachable` after the walk leaves it,
and a walk of a site that is mostly such screens ends early and says `complete`; EXP-9, every action reads the whole page twice and waits for it to settle;
EXP-10, the buggy shop has no cart page and its only open-and-close control is the mobile menu, and read-only mode skips its buttons, so the 80% target needs a
look; EXP-6, a select needs its options, and the typed values are a fixed generator.

### EXP-3: Reset between branches

**Why.** To explore the second branch from a state, the browser must be back in that state. Undoing clicks is
not possible in general.

**Design.**
- Default: open a new browser context with the storage state, navigate to the state's URL, and check that the
  state equals the target (EXP-1). Most states are reachable this way and no history is needed.
- If the check fails (the state was reached through a modal or an in-page step), replay the shortest recorded
  action prefix from the root instead. Prefixes are kept as edge lists in the graph.
- If replay does not reach the state either, mark it `unreachable` in `coverage.json` and move on. Never loop.
- Reset cost is counted in the action budget only when a prefix is replayed.

**Acceptance.**
- On the buggy shop, after exploring the cart page and the product list, a reset returns to the home state
  (equality by EXP-1) in 100 of 100 attempts.
- A fixture where a state is reachable only after opening a dialog is reached by prefix replay, and the dialog
  state appears in `graph.json`.
- A fixture with a state that cannot be re-reached ends with that state in `unreachable` and the walk
  completing.

**Size.** M

### EXP-4: The page.act protocol extension (A3)

**Why.** Today `page.act` is only reserved: the schema's `PageActResult` holds just `ok`, and both drivers
answer it with `unsupported`. The walker and the oracles need to know what the
action did: whether the page navigated, whether a popup, dialog or download appeared, what failed.

**Design.** Extend `PageActResult` in `spec/driver-protocol.schema.json` and regenerate types:
- `url`: the page URL after the action.
- `navigated`: absent, or `{status}` with the main-document HTTP status.
- `popups`: URLs of new pages opened by the action.
- `dialogs`: list of `{type, message, handled}` for alert, confirm, prompt and beforeunload. The policy default
  is to dismiss.
- `downloads`: list of `{url, suggestedName}`; the file is not stored.
- `consoleErrors`: list of `{text, source}` raised during the action.
- `failedRequests`: list of `{url, method, status or failure}`; the URL is redacted like other driver errors.
- `domChanged`: whether a mutation observer, which the driver installs for the action, saw any change during
  settle (no mutation observer exists today).
- `settle`: `{waitedMs, quiet}` where `quiet` is false when the settle limit was reached.
- Capability negotiation in `session.open`: the driver lists the action kinds and event kinds it supports
  (`click`, `hover`, `type`, `select`, `press`; `dialogs`, `popups`, `downloads`, `console`, `network`). The
  existing `consoleEvents` and `networkEvents` capability flags are required fields (false in both drivers
  today), so they stay, deprecated, for older drivers; `failedRequests`, which reports failed requests at load
  through `page.eval`, stays as it is. WebDriver (Safari) reports
  fewer kinds; the engine marks the missing signals as `not_observed` in findings instead of treating them as
  clean.
- The change is additive: a driver that does not send the new fields still validates, and the extraction path
  is untouched. Transcripts are re-recorded.

**Acceptance.**
- `spec/driver-protocol.schema.json` validates a `PageActResult` with every new field, and one with only `ok`.
- A driver test per browser (chromium, firefox, webkit) on a fixture page: a click on a link reports `navigated`
  with status 200; a click that opens `window.open` reports one popup; `alert()` reports one dialog with
  `handled: true`; a click on a failing `fetch` reports one failed request; a download link reports one
  download.
- On the WebDriver driver, `session.open` lists fewer event kinds than chromium and the test asserts the
  difference.
- `uv run scripts/gen_python_protocol.py` and `npm run gen` leave no diff after the change is committed.

**Status (branch `backlog/explorer-page-act-and-safety`).** Built for the Playwright driver and the engine's `Driver::act`:
the schema (all new fields optional, five examples), the five actions, dialogs, popups, downloads, console and failed-request
reports, `domChanged` (shadow roots included) and `settle`, `actionKinds` and `eventKinds` in `session.open`, and the tests in
`web-drivers/playwright-py/tests/test_act.py` against `testbed/sites/act` in Chromium, Firefox and WebKit. The WebDriver driver
lists no actions and no events and still answers `page.act` with `unsupported`; clicking in Safari is not built (optional, and only needed if the explorer must run in Safari).
Tried read-only on three real sites (a Next.js portfolio with a 3D hero, a company site, a learning app; 40, 26 and 28 controls). It found three
bugs, all fixed with tests: scroll-linked effects were credited to the click, a stopped new tab was reported as an error page, and a blocked
same-tab link reported a change. It also showed `blocked_by_policy` doing its job: clicking "Start Learning" tries to `POST /api/auth/guest` on the live
backend, which read-only mode stopped. Limits seen: a click is also a first gesture, so a page that starts music or shows a banner on the first click
reports it on whatever action comes first; a control in a scroll-driven panel that is hidden on a fresh page times out, though extraction (which
leaves the panel on its last state) listed it.
Not done: the engine never calls `page.act` (no safety policy yet, EXP-5); `driver.log` is not used for actions; transcripts were
not re-recorded because the client protocol did not change; `npm run gen` no longer exists (the TypeScript SDK was removed).
Rust readers cannot tell an absent list from an empty one (the generated types use an empty `Vec`), so a reader relies on
`Capabilities.eventKinds` and not on an absent field. Firefox does not report a failed cross-origin `fetch` as a failed request
(it reports an uncaught error), so the failed-request test uses a same-origin request the server drops.

**Size.** L

### EXP-5: Safety policy and read-only mode (A10)

**Why.** Clicking on a real site can delete data, send mail or buy things. A walker that cannot be trusted not
to do that will never be run on anything but toy sites. Labels cannot be the only defence, because labels lie
and are translated.

**Design.** Layers, strongest first:
1. A route handler in the browser context blocks every request whose method is not GET or HEAD while read-only
   mode is on. This is the hard guarantee and does not depend on any label. Blocked requests are recorded as
   `blocked_by_policy` in `events.jsonl`.
2. Read-only mode is the default. It allows only same-origin links, disclosure toggles (`aria-expanded`),
   tabs and menu openers. `--allow-writes` turns it off, and requires `--allow-origin`.
3. A deny-list of destructive names, matched case-insensitively against the accessible name, in every mode:
   delete, remove, pay, buy, purchase, order, submit, send, sign out, log out, cancel subscription,
   unsubscribe, and localised variants (a list per language in the repository, extendable with
   `--deny-name`). A match marks the component `skipped: destructive` and it counts as discovered but not
   touched.
4. Dialogs are dismissed (or cancelled), downloads are discarded, popups are closed.
5. An origin allow-list (`--allow-origin`, default the start origin). Navigation to another origin is blocked.
6. The explorer refuses to start on a non-local origin (anything but `localhost`, `127.0.0.0/8`, `::1`, and
   `*.test`, `*.localhost`) unless `--i-understand-this-interacts` is passed. The refusal exits with code 2 and
   a message that names the flag.
7. Storage-state secrets never enter the run folder, as for `extract`.

**Acceptance.**
- A fixture server that logs the method of every request: a read-only walk of the buggy shop produces zero
  non-GET and non-HEAD requests in the server log, in chromium, firefox and webkit.
- The same fixture with a form button whose label is empty or misleading ("Continue") that posts: still zero
  non-GET requests in read-only mode (layer 1 holds without layer 3).
- "Clear cart" and every checkout-like control on the buggy shop are never activated in read-only mode; they
  appear in `coverage.json` as skipped with reason.
- `traceqa explore https://example.com` exits with code 2 without opening a browser.
- Deny-list unit tests cover at least five languages and the names above.

**Size.** M

**Status (branch `backlog/explorer-page-act-and-safety`).** Built: layer 1 and the layers that belong to the driver, in
`web-drivers/playwright-py/src/traceqa_driver_playwright/safety.py`, and the deny-list and address rules (layers 3 and 5) as
functions in `crates/traceqa-engine/src/policy.rs`. `session.open` takes an `ActPolicy` and says it back; `page.act` is
refused on a session without one (fail closed); in read-only mode (the default) the browser aborts every request that is
not GET or HEAD and no WebSocket is ever connected, and a navigation of the top-level page to another origin is aborted
before it is made. The deny-list is in English, Spanish, French, German, Hindi and Arabic and has not been reviewed by
native speakers. Tests: `web-drivers/playwright-py/tests/test_act.py` in Chromium, Firefox and WebKit against
`testbed/sites/act` (whose server records every request), and unit tests in `policy.rs`.
Measured limits (details in `spec/README.md`): a redirect to another origin cannot be stopped, because the browser follows
it without asking Playwright, so that request is made (a GET in read-only mode) and the driver then notices, reports it and
goes back; Playwright's `service_workers="block"` does not stop registration in any of the three browsers, so pages are not
given `navigator.serviceWorker` instead, which does not cover a dedicated worker the page starts; a GET with a side effect
is stopped only by the name and address lists. Writing a blocked request to `events.jsonl` waits for the explorer.
Not done: the `explore` command, so the refusal to start on a non-local origin exists only as `policy::is_local_origin`
and the flags `--allow-writes`, `--allow-origin`, `--deny-name` and `--i-understand-this-interacts` do not exist yet;
`page.act` still has no caller in the engine; layer 4 (dialogs, downloads, popups) is done in the driver by EXP-4.

### EXP-6: Language model boundary (A7)

**Why.** An explorer whose every step depends on a model is slow, costs money and cannot be replayed. The model
is useful for two things only, and must stay outside the engine core.

**Design.**
- A model is used for (a) form data: given the field role, label, constraints (`type`, `pattern`, `maxlength`,
  `required`) produce a plausible value; (b) the optional judge (ORA-9).
- Without a model the explorer still works: it uses a deterministic generator keyed by (seed, component id)
  (`user@example.test`, `12345`, a fixed sentence). Output with and without a model has the same schema.
- Model calls are made behind a trait in a separate crate (`traceqa-models`, a new crate), not in
  `traceqa-engine`. The engine core depends on the trait only.
- Every model call and answer is written to `events.jsonl`, so a replay uses the recorded values and makes no
  call.
- Cancel semantics cover model calls: a signal aborts an in-flight call within 1 s, and a call counts against the
  wall-clock budget. There is a per-run cap on calls (`--max-model-calls`, default 50).
- Page text sent to a model is redacted like driver errors, and sending is opt-in (`--model <name>`).

**Acceptance.**
- `cargo tree -p traceqa-engine` does not list any HTTP client or model crate.
- With a mock model that sleeps 60 s, Ctrl-C ends the run in under 3 s with a valid folder.
- A run with `--max-model-calls 3` makes at most 3 calls and finishes using the deterministic generator for the
  rest.
- A replay of a run that used a model makes zero model calls (assert on the mock's counter).
- Two runs without a model with the same seed give byte-identical `graph.json`.

**Size.** M

**Built (EXP-6, 28 Oct 2026).** `explore/values.rs` (the `ValueSource` trait, `Values`, the guard around a call, `ModelCall`, `Recorded`), the new crate
`crates/traceqa-models` (`mock` only), and `--model NAME`, `--max-model-calls` (default 50) and `--model-timeout` (default 20 s) on `traceqa explore`.
Without `--model` nothing changes except that a search box now gets one of six words by seed (`data.rs`) and not the fixed word `test`.
How it works: the walk asks `Values` for the text of a field only when it is about to type (the safety rules have let the field through and
`--allow-writes` is on). With a model, the source is asked first, on its own thread; the walk waits in steps of 100 ms, so Ctrl-C and the wall-clock budget end
the wait whatever the source is doing. A call has its own time limit; the run has a cap on calls; three calls in a row without a usable answer switch the
source off for the rest of the run; the same kind of field (role, label, type, placeholder, autocomplete) is asked once per run. An answer that is empty, over
200 characters, has a control character or a line break, or does not fit the field's `type` (`email`, `number`, `tel`, `url`) is thrown away, and the
generator types instead. Fields that hold secrets (type `password`, autocomplete `cc-*` or `*password*`, labels such as card number, CVV, SSN) are never sent.
What is sent is the role, the label and the `type`, placeholder and autocomplete hints, cut to 200 characters with addresses redacted (as driver errors are) and
without the component id. An answer is only text for the field it was asked for: it cannot choose an action, an address or a component, and the safety policy
(EXP-5) decides every action as before. Every call is written to `events.jsonl` as a `model.call` event (`data`: source, key, request, outcome, answer, note,
millis), with outcomes `used`, `rejected`, `failed`, `timed-out`, `stopped`, `over-cap`. `Recorded::from_events` reads those back as a source, so a replay uses the
recorded text and makes no call.
Tests: 18 unit tests in `explore/tests.rs` against an in-memory site and a counting source (the answer is typed, nothing is asked read-only, the cap, a cap of
zero, a source that always fails, one that panics, long and odd answers, answers that do not fit the field type, secrets, redaction, one question per kind of field,
Ctrl-C in a call that sleeps 60 s ends the walk in under 2 s, a call that is too slow, the wall-clock budget, a replay that asks no model, the same bytes), 3 in
`traceqa-models`, `crates/traceqa-engine/tests/no_network_dependencies.rs` (runs `cargo tree -p traceqa-engine` and fails on an HTTP client, a TLS library, an
async runtime or `traceqa-models`), and 9 real-browser tests in `tests/test_explore.py` on the fixture page `/form`, which reports what was typed to the server.

**Not done, and limits (read before building the real connections).**
- **No real model is connected yet.** `--model mock` sends nothing anywhere; it exists so that everything around a model is tested. The next step is to add real
  sources in `traceqa-models`: one for any endpoint that speaks the common chat format (GLM from Z.ai works this way; the address must be configurable so that a
  customer can point it at their own gateway), and one for Amazon Bedrock, which has its own format and sign-in. Keys come from the environment, never from
  arguments, and never go into a file. A source that sends text out of this machine must need a second flag, like `--i-understand-this-interacts` does for a site that
  is not local (a model on localhost does not need it). Whether the Jev judge of the sourcing project can be a source is not decided: its interface was not looked at.
  The judge of the oracles (ORA-9) uses the same boundary and is its own item.
- **A call that is given up on leaves its thread behind.** The walk stops waiting (Ctrl-C, the call's time limit or the wall clock), but a thread cannot be killed, so
  the call goes on in the background until it ends by itself and its answer goes nowhere. For a command that exits when the run ends this costs nothing (checked:
  Ctrl-C with a call that sleeps 60 s ends in under 3 s). If the engine is ever run as a long-lived service, many runs would collect one waiting thread per stuck
  call. The fix then is a source that can be cancelled (an HTTP client with its own timeout) and/or a bounded pool for the calls. The comment on `ask` in
  `values.rs` says the same.
- **What is sent is only redacted for web addresses.** `redact_text` strips query strings from addresses (a `?token=...`) and nothing else. A label
  that holds a name, an email address or a customer reference is sent as it is, up to 200 characters. Fields that hold secrets (passwords, card numbers, CVV, SSN)
  are never sent, but that is a list of words and types, not a promise about every label. Before a real model is connected: decide what else to remove (email
  addresses, long digit runs), say in the first message of a run with `--model` exactly what is sent and where, and keep the second flag for addresses that are not
  local. The mock sends nothing, so none of this applies yet.
- **Field limits are only partly known.** The page script copies `type`, `placeholder`, `autocomplete` and `name`, and the state has `required`, but not `maxlength` or
  `pattern`; those need a change to `COPIED_ATTRS` in `page-script/page-script.js`, which changes every `components.json` and the fingerprint golden files, so it was left alone.
  An answer is cut off by length (200) and checked against `type`, not against `maxlength` or `pattern`.
- **`model.call` events are written when the walk ends**, from what the walk returned, and not one by one while it runs. After a normal end or the first Ctrl-C they are all
  there; after a second Ctrl-C that kills the run only the progress lines (`model mock for "...": used`) are. They go to `events.jsonl` with type `model.call`, which the
  schema allows (`type` is an open string); its list of known values in `spec/run-folder.schema.json` was not extended.
- **Replay needs EXP-8.** `Recorded` and its test exist; the command that replays a run with it does not.
- A `select` is still left alone (`no-options`): its options are not known to the walker. The model could choose among them once they are.
- Not verified: a real model of any kind; Linux, Windows, Rust 1.88.

### EXP-7: Outputs and coverage

**Why.** The graph and the coverage figure are the product. They have to be files with a schema, so that other
tools and the CI gate can read them.

**Design.** New schemas `spec/graph.schema.json` and `spec/coverage.schema.json`, written to the run folder.
- `graph.json`: `schemaVersion`, `startUrl`, `seed`, `budgets` and the stop reason, `states` (id, url, modal,
  component ids, frame ids, `depth`), `edges` (id, from, component id, action, to, `result` with the
  `PageActResult` essentials and any oracle findings), and `stopped` (`complete`, a budget name, `cancelled`).
  Keys are sorted and lists are in discovery order so the file is deterministic.
- `coverage.json`: per state `{discovered, touched, skipped, untouched}` as component id lists and counts; overall
  `touched / discovered`; the `unreachable` states; the list of skipped components with reason (`destructive`,
  `read-only`, `disabled`, `blocked`).
- Every step is an event in `events.jsonl`: `explore.action.start`, `explore.action.end`, `explore.state.new`,
  `explore.policy.block`.
- Hand-off: `traceqa codegen` can take a path through `graph.json` and emit a test (ties into the existing
  page-object output).

**Acceptance.**
- Both files validate against their schemas with the repository's validator (`uv run pytest tests/test_spec_validator.py`
  covers them once they are added).
- `coverage.json` totals equal the sums of its per-state counts.
- On the buggy shop in read-only mode the run touches at least 80% of the discoverable components within 150
  actions. "Discoverable" is the set of component ids that `traceqa extract` finds on every state that a
  read-only action can open; it is listed in `coverage.json` as `discovered`.
- `events.jsonl` has one `explore.action.end` per edge in `graph.json`.

**Carried forward.** From EXP-5: `coverage.json` lists a skipped component with the reason `policy::Skip::reason()` gives (`destructive`, `blocked` or `read-only`), and a request the driver stopped (`failedRequests` entry with the failure `blocked_by_policy`) is written to `events.jsonl`.

**Size.** M

### EXP-8: Replay

**Why.** A finding that cannot be reproduced is ignored. The walk is useful to a developer only if they can
re-run the exact path that exposed a bug.

**Design.**
- `traceqa replay graph.json --path EDGE...` performs the listed edges in order from the seed state, using the
  recorded action, value and component id, and re-resolves the component by id on the current page.
- `--path` also takes a state id to replay the shortest path to it.
- Output: a result per edge (`reproduced` when the target state equals the recorded one by EXP-1, `diverged`
  with the actual state, or `missing` when the component id is not on the page) and exit code 1 if any
  edge is not reproduced.
- `traceqa-replay` today only replays recorded client conversations (a stand-in for `traceqa serve`), so this is
  a new command. It reuses the safety policy, so a replay is also read-only by default.

**Acceptance.**
- Replaying every edge of a buggy-shop graph, edge by edge from the seed, reproduces at least 95% of the
  recorded transitions.
- A replay against a copy of the site with one control removed reports that edge as `missing` and exits 1.
- A replay never sends a non-GET request in read-only mode (server log).

**Size.** M

### EXP-9: Incremental re-extraction (A9)

**Why.** A full extraction takes 30 to 90 seconds on a long page. After each of 300 actions that is hours.

**Design.**
- After an action, first ask the collector for the components under the nodes that changed (the mutation
  observer that EXP-4 adds for `domChanged` gives the roots; settling today uses none), by id, and merge them into the previous component list.
- Fall back to a full extraction when the URL changed, a modal opened or closed, the changed region exceeds
  a share of the page (default 30%), or every Nth action (default 25) to catch drift.
- The merge must give the same id set as a full extraction. A debug flag `--verify-incremental` does both and
  reports differences.

**Acceptance.**
- With `--verify-incremental`, 100 actions on the buggy shop show zero id-set differences between the
  incremental and the full extraction.
- On a long fixture page (at least 2000 components), the median time of an action that opens a disclosure is at
  least 5 times shorter with incremental extraction than with a full one (print both medians).
- `--no-incremental` gives the same `graph.json` byte for byte on the buggy shop.

**Size.** L

### EXP-10: Explorer acceptance on the buggy shop

**Why.** One end-to-end check ties the pieces together and is what a verifier runs.

**Design.** A test that starts the buggy shop on a free port and runs
`traceqa explore <url> --seed 1 --max-actions 150` in read-only mode, in each of the three browsers. The
shop's server logs nothing today, so it gains the request-method log that EXP-5 describes.

**Acceptance.**
- Touches at least 80% of the discoverable components within 150 actions.
- The server log shows zero non-GET and non-HEAD requests.
- Two runs with the same seed give byte-identical `graph.json`.
- Each of the budgets in EXP-2 is respected (counts at or below the limit).
- "Clear cart" and the checkout-like controls are never activated.
- Replay of the recorded edges reproduces at least 95% of the transitions.
- A cancel at action 40 yields a folder that validates.

**Carried forward.** From EXP-5: the buggy shop's `serve.py` records the method of every request (`testbed/sites/act/serve.py` already does, and is the model), and the test asserts that a read-only walk produced only GET and HEAD, in all three browsers. "Clear cart" must appear as skipped, not touched.

**Size.** S (on top of EXP-1 to EXP-9)

## 2. Interaction oracles (F7)

Interaction without checks finds nothing. These oracles run after each explorer action. They are cheap, general
and need no model. Findings use the existing findings model, keyed by component id and action, so they flow into
the same suppression, baseline and gate machinery as the static rules. Third-party noise is handled by an
origin allow-list (`--oracle-origin`, default the start origin). Signals that are flaky by nature are confirmed
by repeat runs (ORA-10).

Every oracle has a planted-bug fixture: a small page where the bug exists, plus an unmodified control page
where the oracle must stay silent. Rule ids are lower case with hyphens, like the existing findings rules
(`covered-at-load`).

### ORA-1: Dead control

**Why.** A button that does nothing is the commonest user-visible bug and the cheapest to detect.

**Design.** After a click, if the URL, the visible component id set, the modal marker and the DOM mutation
observer all report no change, and no request was made, the control is dead. Controls whose only effect is an
attribute change (such as `aria-pressed`) change the DOM and are therefore not dead. Rule id `dead-control`.

**Acceptance.**
- On a fixture with one button that has no handler and one with a handler, exactly one `dead-control` finding
  is reported, for the first.
- On the control page, zero findings.
- On the buggy shop in read-only mode, the finding ids are the same over 5 repeats.

**Size.** S

### ORA-2: Uncaught errors and console errors

**Why.** Script errors triggered by an interaction are real bugs, and often the only visible symptom.

**Design.** Uses `consoleErrors` from EXP-4. Uncaught exceptions are errors; `console.error` is a warning;
messages from non-allow-listed origins are dropped. Rule ids `uncaught-error`, `console-error`.

**Acceptance.** A fixture whose click handler throws gives one `uncaught-error` with the component id; a
fixture that calls `console.error` gives one `console-error`; a third-party script that logs from another origin
gives none. The `not_observed` marker appears on the WebDriver driver. The `consoleEvents` capability exists
today but is false in both drivers, and `not_observed` is a new marker set from it.

**Carried forward.** From EXP-4: a driver says which events it observes in `Capabilities.eventKinds`; the findings of an oracle mark a signal `not_observed` when the session's driver does not list its kind (the WebDriver driver lists none), and must not read an empty list as "nothing happened". Same for ORA-3.

**Size.** S

### ORA-3: Failed requests

**Why.** A click that triggers a 500 or a network failure is a bug the page may hide from the user.

**Design.** Uses `failedRequests` from EXP-4. Status 400 and above on allowed origins, and network failures,
are findings, `failed-request`, with the redacted URL. Optional status allow-list (`--ignore-status 404` for
known probes). What exists today covers the page load only: the Playwright driver's `failedRequests`
capability and the `requests_failed` warning give a count and five URLs since `page.goto`, with no status or
method, so they cannot be pinned on one click; the WebDriver driver reports none.

**Acceptance.** A fixture whose button fetches a URL that returns 500 gives one finding with status 500; one that
returns 404 with `--ignore-status 404` gives none; a request to a third-party origin gives none by default.

**Size.** S

### ORA-4: Navigation to an error page

**Why.** A link that leads to a 404 page is a broken link.

**Design.** Uses `navigated.status` and the threshold of the existing `http_error_status` warning (status 400
and above), which today checks the page load only. Rule id `error-page`. The edge
target state is marked as an error state and is not expanded.

**Acceptance.** A fixture with a link to a 404 page gives one `error-page` finding with status 404 and the
walker does not expand that state (no edges leave it).

**Size.** S

### ORA-5: Focus lost or trapped

**Why.** Keyboard users lose their place when focus vanishes after an action, or cannot leave a dialog.

**Design.** After an action, read the active element. Findings: `focus-lost` (focus moved to the body after an
action that removed the focused component), `focus-trapped` (in an open non-modal region, Tab does not leave the
region in 20 presses). For modal dialogs trapping is correct; the check is the other direction: Escape must
close it (`dialog-not-dismissible`).

**Acceptance.** Fixtures: a button that removes itself and leaves focus on the body gives `focus-lost`; a
non-modal panel with a Tab trap gives `focus-trapped`; a correct modal gives none; a modal that ignores Escape
gives one `dialog-not-dismissible`.

**Size.** M

### ORA-6: Layout shift

**Why.** A click that makes the page jump is a visible quality problem.

**Design.** Sum the movement of components that did not change (matching by id, using the bounding boxes already
in `components.json`) relative to the viewport height. Above a threshold (default 0.1) report `layout-shift`
with the largest movers. It uses boxes before and after, not the browser's layout-shift API, so it works in every
browser.

**Acceptance.** A fixture that injects a banner above the content on click gives `layout-shift` with a score above
0.1; a fixture that opens an overlay without moving content gives none.

**Size.** S

### ORA-7: Newly covered primary controls

**Why.** If an action covers a primary control with something that stays (a toast, a sticky bar), the user
cannot use it.

**Design.** After the action, read the same collector `covered` state that the `covered-at-load` rule uses at
load (and that `diff` compares across runs as `became_covered`), and report controls covered now but not
before. Rule id `newly-covered`.

**Acceptance.** A fixture with a sticky bar that appears over the checkout button gives one `newly-covered`
finding naming that button; a fixture with a toast that disappears within the settle time gives none.

**Size.** S

### ORA-8: State flip not reflected in aria

**Why.** A disclosure that opens but keeps `aria-expanded="false"` misleads screen reader users.

**Design.** For components with `aria-expanded`, `aria-pressed` or `aria-checked` in the collector's state, check
that the value changed after a click that changed what is visible (for `aria-expanded`: the controlled region
changed visibility). Rule id `aria-state-not-updated`.

**Acceptance.** A fixture disclosure that shows its panel but keeps `aria-expanded="false"` gives one finding; a
correct disclosure gives none; a toggle that has no ARIA state gives none (that would be a separate static
rule, not built either).

**Size.** S

### ORA-9: Optional judge

**Why.** Some outcomes are only wrong in meaning ("Added to cart" when nothing was added). A model can flag
them, but only as a suggestion.

**Design.** Off by default. With `--judge <model>`, after an action the engine sends the before/after
component names and visible text changes (redacted) and asks whether the outcome is plausible. The answer is a
finding of severity `info` with a mandatory written reason, stored with the model name. Judge findings never
affect `--fail-on` unless `--judge-gates` is set. Calls obey EXP-6.

**Acceptance.** With a mock model that answers "implausible", the finding carries the written reason and the
model name; without `--judge` there are zero model calls; a finding without a reason is rejected by the schema.

**Size.** S

### ORA-10: Separating flakes by repeats

**Why.** Console and network signals are timing-dependent. One run cannot tell a bug from noise.

**Design.** `traceqa explore --repeat N` (N at least 2) runs the same walk N times with the same seed and tags
each finding with its occurrence count `k`. The report lists findings seen in all N runs as `confirmed` and the
rest as `intermittent`. It extends the group folder and `stability.json` that `extract --repeat` already
writes, from components to findings. Gating by k-of-n is CUT-3.

**Acceptance.** A fixture whose handler throws on a random 50% of clicks produces `intermittent` over
10 repeats; a deterministic bug produces `confirmed` in 10 of 10.

**Size.** M

## 3. Architecture gaps

The explorer covers A3 (EXP-4), A7 (EXP-6), A9 (EXP-9) and A10 (EXP-5). The rest of A4 and A8 are below. A1,
A2, A5, A6 and the first parts of A4 and A8 were built in the feature phase.

### ARC-1: Collector facts beyond the first tier (rest of A4)

**Why.** Many useful rules need facts the collector does not give: `aria-*` attributes beyond state, tab order and
focusability, and styles.

**Design.** Add a collector options tier, like the existing `includeHidden`, so the default output stays small:
- `aria`: copy `aria-describedby`, `aria-labelledby` resolved text, `aria-controls`, `aria-haspopup`,
  `aria-current`, `aria-live` into `attrs`.
- `focus`: whether the element is focusable and its position in sequential tab order (computed without moving
  focus); the `tabindex` value is already in `attrs`.
- `styles`: an optional computed-styles tier: colour, background colour, font size, outline when focused, and
  `cursor`. Off by default because it is large and slow.
- Each tier is a field of `ExtractOptions` and of the CLI (`--collect aria,focus,styles`), and is recorded in
  `run.json`.

**Acceptance.**
- With no option, `components.json` for the buggy shop is byte-identical to today's golden files.
- With `--collect aria,focus`, the output is larger than the default by at most 30% on the buggy shop (print
  both sizes), and every tab-order position is unique within a frame.
- With `--collect styles`, a fixture with white text on a white background has both colours recorded and a
  contrast figure that a rule can use.
- Golden ids (`tests/test_golden_ids.py`) do not change with any tier on.

**Size.** M

### ARC-2: Rate-limited, tested MCP progress notifications (rest of A8)

**Why.** `extract` already sends `notifications/progress` when the request carries a progress token: one per
engine event, so a long page can send many in a burst. The one test (`extract_reports_progress_and_a_summary`
in `crates/traceqa-mcp/src/tests.rs`) checks only the token and increasing values, on a run with no browser.

**Design.** Keep the notifications on the steps the engine already reports (browser started, navigated,
settled, extracted), rate-limited to one per second with the last step always sent, and extend the tests.

**Acceptance.** A test client that sends a progress token receives at least 3 progress notifications during an
extract of the buggy shop, never two within one second, and none when it sends no token; each notification has
an increasing `progress` value.

**Size.** S

### ARC-3: Long-lived MCP sessions (rest of A8)

**Why.** The explorer and multi-step agent work need to keep a browser open between tool calls, instead of
paying the browser start-up every time.

**Design.** `open_session`, `act` and `close_session` tools that wrap the engine's session methods, with an
idle timeout (default 120 s), a cap on open sessions (default 2), the same URL policy as `extract`, and
read-only mode by default (EXP-5). Sessions are closed on client exit and on signals.

**Acceptance.** Ten `get_components` calls on one session take less than half the time of ten independent
calls (print both); the session is gone after the idle timeout (a status call reports it closed); killing the
MCP server leaves no browser process running (checked as in the existing leftover-process tests).

**Size.** M

### ARC-4: The server/discover handshake (rest of A8)

**Why.** The MCP revision dated 2026-07-28 describes a `server/discover` handshake; the server does not speak it,
so newer clients cannot learn its capabilities without initialising.

**Design.** Implement `server/discover` returning name, version, protocol versions and capabilities, without
creating session state. Keep the current `initialize` flow working for older clients.

**Acceptance.** A client that sends `server/discover` as its first message gets a result that lists the tools
capability and the supported protocol versions; the existing MCP tests still pass; a transcript is recorded
for it.

**Size.** S

### ARC-5: Resource subscriptions and list-changed (rest of A8)

**Why.** Run folders appear while a run is going. The MCP resources specification has `resources/subscribe`
and `notifications/resources/list_changed` for exactly this, so an agent could watch a run without polling.

**Design.** Declare `resources: {subscribe: true, listChanged: true}`. Send `list_changed` when a run folder is
created or closed, and `notifications/resources/updated` for a subscribed run resource when its
`components.json` is written.

**Acceptance.** A test client that subscribes to a run, then starts an extraction, receives exactly one `updated`
notification when `components.json` is written and one `list_changed` when the run appears; unsubscribing stops
them.

**Size.** S

## 4. Deferred hygiene

### HYG-1: Ruff ANN rules (deferred in M7)

**Why.** Explicit annotations on public functions make the Python protocol package and drivers readable without an
editor. They were left out when the lint rules were widened because pyright already checks types, and turning
on `ANN` at that point would have meant hundreds of mechanical edits.

**Design.** Add `ANN` to `select` in `pyproject.toml`. Fix public API first (`web-drivers/traceqa-protocol`),
then drivers and scripts. Ignore `ANN401` (explicit `Any`) where the protocol really is dynamic, each with a
one-line justification in the config. Tests may be exempt through `per-file-ignores`.

**Acceptance.**
- `uv run ruff check .` exits 0 with `ANN` selected.
- Every ignore for an `ANN` code in `pyproject.toml` has a comment.
- `uv run pyright` reports no new errors compared with before the change (compare counts).
- CI is green on the three operating systems.

**Size.** M

### HYG-2: A run.start racing the first signal is accepted, then cancelled (X19 residual)

**Why.** `traceqa serve` and `traceqa mcp` refuse new runs once the first signal is seen, and a monitor
cancels every registered run again every 20 ms. A `run.start` that passes the check just before the signal is
still accepted and registered after the sweep; the monitor cancels it within about 20 ms, but the client has
been given a run that never had a chance, and a driver may already be starting.

**Design.** Set the cancel flag first, then take the runs lock and check the flag again before registering a run.
A start that sees the flag set answers with a "shutting down" error and starts nothing. The sweep takes the same
lock after setting the flag, so no run can be added after it.

**Acceptance.** A stress test sends the signal and a `run.start` concurrently, 200 times; every iteration ends
with no run left running, a start answered after the cancel flag was set is always refused (never given a run
id), and the process exits within the documented bound. Run before the fix, at least one of the 200 iterations
gives a run id after the flag was set.

**Size.** S

### HYG-3: Driver tree kill without `ps -A -o pid=,ppid=` (X20 residual)

**Why.** On Unix the engine kills the whole driver process tree by walking `ps`. On busybox (Alpine) `ps` does
not accept `-A -o pid=,ppid=` (to confirm in the image first), so only the driver itself is killed and its
browser child processes can be left running.

**Design.** Fall back, in order: (1) read `/proc/<pid>/stat` for every pid and build the tree from the parent
field; (2) start each driver in its own session so the whole group can be killed with one signal to the
negative pid, through the `setsid` command, because the workspace forbids the `unsafe` a `pre_exec` call needs. Prefer (2) for new starts and keep (1) as the fallback for the process list.

**Acceptance.** A test inside an Alpine container image (`alpine:3`) starts a driver that spawns two children,
cancels the run, and asserts that none of the three processes is left (process count of zero). The test fails on
the current code in the same image.

**Size.** M

### HYG-4: The test suite's run folder check skips the optional files

**Why.** `check_run_folder` in `conftest.py` checks `run.json`, `events.jsonl` and every `components.json`
against `spec/`, but not the files that only some runs write: `axe.json` (`--axe`), `findings.json`
(`traceqa findings`) and the group folders `runs/g_<id>/` (`--repeat`, `traceqa map`). They are validated only
inside the tests that make them (`test_axe.py`, `test_findings.py`, `test_findings_cross_browser.py`,
`test_repeat.py`, `test_map.py`), so a file reached another way (through `serve` or MCP) is not checked.

**Design.** In the target check, validate `axe.json` against `AxeResults` and `findings.json` against
`FindingsFile` when they exist, with the same round-trip rule as `components.json`. Add a `check_group_folder`
for `group.json` (`RunGroup`), `stability.json` (`StabilityFile`) and `site.json` (`SiteFile`), and use it in
the repeat and map tests instead of their own helpers.

**Acceptance.** A copy of a run folder whose `axe.json` lacks `axeVersion` fails `check_run_folder` with a
message naming `axe.json`; the same for a `findings.json` with an unknown field and a `site.json` without
`pages`. Every existing test still passes.

**Size.** S

## 5. Scope cuts of the feature phase

Features that were considered and not built, or built in part. Each says why it was cut and when it would be
built. A scan of the commit messages (`git log --grep 'Not done:'`) found no commit with a `Not done:` line at the
time of writing. Feature commits must add such a line for anything they cut, and this section must list it;
rerun the scan before each release.

### CUT-1: `--format github` for extract (QW6)

**Why.** `diff` and `findings` can write GitHub annotations; `extract` produces data, not verdicts, so there was
nothing to annotate. If extract gains its own warnings gate, annotations make sense.

**Design.** Map extract warnings to workflow command lines (`::warning file=...::message`) with the same
escaping as the existing diff formatter.

**Acceptance.** `traceqa extract <url> --format github` prints one `::warning` line per warning and exits 0; a
fixture with a message containing `%`, a newline and a colon is escaped exactly as in the diff formatter's test.

**Size.** S

### CUT-2: Stability in MCP get_components

**Why.** The `--repeat` statistics exist in run folders, but the MCP tool does not return them, so an agent
cannot tell a flaky component from a stable one.

**Design.** Add `presence_rate` to the detailed response when the run has stability statistics; omit it
otherwise. Flag components below a threshold in concise mode.

**Acceptance.** On a run made with `--repeat 5`, `get_components` in detailed mode returns `presence_rate` for
each component, and concise mode flags every component below 1.0; on a run without repeats the field is absent.

**Size.** S

### CUT-3: Findings gating by k-of-n repeats

**Why.** `--fail-on` gates on one run. With flaky signals (ORA-2, ORA-3) a single run causes false failures.
`extract --fail-on-unstable` already gates on component presence over `--repeat` runs; nothing gates findings
that way.

**Design.** `--fail-on-repeats K/N` runs N times and fails only for findings seen in K or more runs. Builds on
ORA-10 and `--repeat`.

**Acceptance.** For a fixture bug that appears in 50% of runs, `--fail-on-repeats 5/5` passes in at least 90 of
100 trials, and a deterministic bug fails in 100 of 100.

**Size.** M

### CUT-4: SARIF with repo-relative source mapping

**Why.** The SARIF output points at URLs and component ids. GitHub code scanning wants file locations in the
repository, so alerts show only on the Security tab, not in the diff.

**Design.** An optional mapping file (component id or test id to source file and line), supplied by the user or
produced from codegen output, filled into `physicalLocation.artifactLocation` with repo-relative URIs.

**Acceptance.** With a mapping file, the SARIF output validates against the SARIF 2.1.0 schema and every result
with a mapped component has a repo-relative `uri` (no `file://`, no drive letter); results without a mapping are
unchanged.

**Size.** M

### CUT-5: Code generation for Java/Selenium and Cypress, and a live `--verify`

**Why.** Codegen covers TypeScript and Python Playwright page objects only. Other targets multiply the test
matrix. `--verify` (resolve the generated locators on the live page) needs a browser session for each
generation.

**Design.** One emitter module per target over the same locator descriptors. `--verify` opens the page, resolves
each generated locator, and fails if one matches zero or more than one element.

**Acceptance.** For each added language, generated code for the buggy shop compiles (`javac`, or the Cypress
type check) and a test using it passes against the shop. `traceqa codegen <run> --verify` exits 1 on a copy of
the shop with one control removed and names the locator.

**Size.** L

### CUT-6: Python port of the bench scoring onto `traceqa verify`

**Why.** The bench keeps its own scoring in Python. `traceqa verify` now reads the same manifest format, so the
two should share one implementation, but the bench was left alone to keep the phase small and the bench results
comparable.

**Design.** Replace the bench's scoring with a call to `traceqa verify` and map its result to the bench's
table. Keep the old scorer until the new output matches.

**Acceptance.** On the 37 bench cases, scores computed through `traceqa verify` equal the current scores in
every case (a script prints any difference and exits 1 on one), and the bench's old scoring code is removed.

**Size.** M

### CUT-7: axe-core in MCP

**Why.** `--axe` is a CLI option of `extract`. The MCP `extract` tool does not pass it, because axe results are
large and an agent needs a concise form first.

**Design.** Add an `axe` boolean to the MCP extract tool and return only counts by impact plus the ids of the
affected components in concise mode.

**Acceptance.** `extract` with `axe: true` over MCP returns violation counts that equal those of the CLI on the
same page; concise output is at least 5 times smaller than the CLI's axe output on the buggy shop (print both
sizes).

**Size.** S

### CUT-8: Cross-browser pixel comparison (refused)

**Why.** Comparing screenshots between Chromium, Firefox and WebKit shows font rendering, anti-aliasing and
scrollbar differences everywhere. Every page would fail, and the signal would be noise. Playwright's own guidance
is to keep screenshot baselines per browser and platform. The visual comparison (`traceqa visual`) is therefore
limited to the same browser and viewport, and refuses anything else.

**Design.** None. Cross-browser differences are reported structurally, by component, through `diff` and the
cross-browser findings.

**Acceptance.** `traceqa visual` on two runs from different browsers exits non-zero and its message names the
browser mismatch. This is already enforced; the item stays so that nobody re-proposes it without new evidence.

**Size.** S

### CUT-9: GitHub Action wrapper

**Why.** The README (Distribution) lists a GitHub Action as planned. CI users call the binary directly, which
works and is simpler to keep correct.

**Design.** A composite action that downloads the release binary for the platform, checks its checksum and
runs a given command. It needs a stable asset naming scheme, which the release process already has.

**Acceptance.** A workflow in a separate test repository uses the action with `traceqa --version` and prints the
release version; the action fails on a checksum mismatch (tampered asset).

**Size.** M

### CUT-10: Slim baseline format

**Why.** A baseline stores the full component list, which is large for big sites and noisy in review. A slimmer
form (id, role, name, landmark path) would diff well in a pull request.

**Design.** A `--slim` flag for baseline saving that stores only the fields the diff and heal need, plus a
version field; `diff --against` accepts both forms.

**Acceptance.** For the buggy shop a slim baseline is at least 70% smaller than the full one (print both
sizes), and `diff --against` gives the same verdict with either form on the golden diff pairs.

**Size.** M

## 6. Known limits of the shipped features

Features that shipped in the feature phase but stop short of the whole design. The engineer who built each
feature recorded the limit; the item says what it would take to lift it.

### LIM-1: Visual comparison skips components whose name changes

**Why.** `traceqa visual` compares crops only for components the structural diff calls unchanged, and it
skips a component whose accessible name changes between the runs (a clock, a counter, a price). Masking the
changing text would keep the rest of the component under test, but the collector records the component's box,
not the boxes of the text inside it.

**Design.** Have the collector record text boxes (one rectangle per text node of a component, capped per
component) next to the instance box, behind an option so that ordinary runs stay the same size. Visual
comparison then paints those rectangles the same flat colour in both crops before comparing, and reports the
component as compared with a mask.

**Acceptance.** On a fixture page whose only change between two runs is the text of a timestamp inside a card,
`traceqa visual` reports the card as compared and unchanged (not skipped); when the card's background colour
also changes it reports the card as changed. Runs made without the option have `components.json` files of the
same size as today.

**Size.** M

### LIM-2: Old diff pictures stay in the output folder

**Why.** `traceqa visual --out DIR` writes `<DIR>/<target>/<id>.diff.png` for each changed component. On a
second comparison into the same folder it removes the picture of a component that is now unchanged, but not
the picture of a component that is no longer compared at all (removed from the page, now covered or now
skipped). A CI artifact can then show a difference that the current report does not list.

**Design.** Before writing, remove every `*.diff.png` in each target folder of `--out` whose id is not in the
current report's changed list, or write into a fresh folder and replace the old one when the comparison is
done.

**Acceptance.** Compare two runs that differ in components a and b into `DIR`, then compare two runs where a is
gone and b is unchanged into the same `DIR`: afterwards `DIR` holds no `.diff.png` file, and the report lists
none.

**Size.** S

### LIM-3: Shared page chrome with page-specific landmark names is not reported as shared

**Why.** `traceqa map` lists in `site.json` the component ids present on every page it read. The id includes
the landmark path, so a header whose landmark is named after the page (`aria-label="Cart navigation"` on one
page, `"Shop navigation"` on another) gives the same buttons different ids, and they are not listed as shared.
`docs/usage.md` states this.

**Design.** Add a second, looser grouping to `site.json`: components that match by role, name and the landmark
roles of their path (without the landmark names), listed as `similar` with the ids per page. Keep `shared`
exact, so that nothing that relies on it changes.

**Acceptance.** On a fixture site of three pages whose `nav` landmark has a different `aria-label` on each page,
`site.json` lists the nav links under `similar` with one id per page, and `shared` is the same as today.

**Size.** M

### LIM-4: MCP `findings` leaves out axe-core results

**Why.** `traceqa findings` reads `axe.json` when a target has one and reports its violations with the other
findings. The MCP `findings` tool does not, so an agent gets fewer findings than the CLI for the same run folder.

**Design.** Read `axe.json` in the MCP tool the way the CLI does, and return the axe findings in concise form
(rule id, impact, component id, count). Do this together with CUT-7, which lets MCP make such runs.

**Acceptance.** For a run made with `traceqa extract --axe` on the buggy shop, the MCP `findings` tool returns
the same number of axe findings as `traceqa findings --format json` on the same run folder, and every returned
item is marked as untrusted page content like the rest of the tool's output.

**Size.** S

### LIM-5: Concise `get_components` is about half the size of detailed

**Why.** Concise answers exist to save an agent's context. Since the locator hint was added to every row,
concise `get_components` on the buggy shop is about 52% of the detailed answer (5759 against 11058 bytes for
37 rows), where the aim was about a third.

**Design.** Today every concise row carries the locator cut to 120 characters, plus `locator_unique: false`
when it is not unique. Give it only when it is unique and short, and add a flag that says a locator is
available through `find` or the detailed answer otherwise. Measure with the same call the MCP test
makes.

**Acceptance.** The MCP test's measurement of `get_components` with `limit` 37 on `chromium-1280x800` prints a
concise size of at most 40% of the detailed size, and the test's bound (today `small <= 0.6 * large`) is
tightened to that; an agent can still get every
component's locator from the detailed answer.

**Size.** S


## How to read this file

Every item is a heading of the form `### ID: title`, followed by four labelled parts, and sometimes a fifth, **Carried forward**, for work that an earlier item left for this one because it needs something this item builds:

- **Why.** The problem, in a sentence or two.
- **Design.** Notes on how to build it, including the existing code it builds on.
- **Acceptance.** Observable, measurable statements: a number, a command or a file. Each one can be checked by
  someone who did not write the item.
- **Size.** S (a day or two), M (about a week), L (several weeks).

Item ids are stable. `EXP-` items are the explorer, `ORA-` items the interaction oracles, `ARC-` items
architecture gaps, `HYG-` items deferred hygiene, `CUT-` items features that were scoped out of the
feature phase and `LIM-` items known limits of features that did ship. The numbers F6, F7 and A3 to A10 refer to the feature review that produced this list.

Order of work: the safety policy (EXP-5) and the `page.act` extension (EXP-4) come first, because nothing may
click on a real site without them. The walker (EXP-1 to EXP-3) comes next, then outputs and replay (EXP-7,
EXP-8), then the oracles. Hygiene items are independent and can be taken at any time.
