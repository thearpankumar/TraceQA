# Backlog

Work that is designed but not built. Nothing here exists in the code yet. Each item says why it matters, how
it should work and how to tell that it is done, so that someone can pick it up without asking.

## How to read this file

Every item is a heading of the form `### ID: title`, followed by four labelled parts:

- **Why.** The problem, in a sentence or two.
- **Design.** Notes on how to build it, including the existing code it builds on.
- **Acceptance.** Observable, measurable statements: a number, a command or a file. Each one can be checked by
  someone who did not write the item.
- **Size.** S (a day or two), M (about a week), L (several weeks).

Item ids are stable. `EXP-` items are the explorer, `ORA-` items the interaction oracles, `ARC-` items
architecture gaps, `HYG-` items deferred hygiene and `CUT-` items features that were scoped out of the
feature phase. The numbers F6, F7 and A3 to A10 refer to the feature review that produced this list.

Order of work: the safety policy (EXP-5) and the `page.act` extension (EXP-4) come first, because nothing may
click on a real site without them. The walker (EXP-1 to EXP-3) comes next, then outputs and replay (EXP-7,
EXP-8), then the oracles. Hygiene items are independent and can be taken at any time.

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
  Chrome means ids that the site map (`traceqa map`, `site.json`) shows on at least a configurable share of
  pages (default 0.8), such as the header and footer links. They are tracked once, globally, and not per state.
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

**Size.** L

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

**Why.** Today `page.act` is reserved and returns only `{ok}`. The walker and the oracles need to know what the
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
- `domChanged`: whether a mutation observer saw any change during settle.
- `settle`: `{waitedMs, quiet}` where `quiet` is false when the settle limit was reached.
- Capability negotiation in `session.open`: the driver lists the action kinds and event kinds it supports
  (`click`, `hover`, `type`, `select`, `press`; `dialogs`, `popups`, `downloads`, `console`, `network`). The
  existing `consoleEvents` and `networkEvents` flags are replaced by this list. WebDriver (Safari) reports
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

### EXP-6: Language model boundary (A7)

**Why.** An explorer whose every step depends on a model is slow, costs money and cannot be replayed. The model
is useful for two things only, and must stay outside the engine core.

**Design.**
- A model is used for (a) form data: given the field role, label, constraints (`type`, `pattern`, `maxlength`,
  `required`) produce a plausible value; (b) the optional judge (ORA-9).
- Without a model the explorer still works: it uses a deterministic generator keyed by (seed, component id)
  (`user@example.test`, `12345`, a fixed sentence). Output with and without a model has the same schema.
- Model calls are made behind a trait in a separate crate (`traceqa-models`, planned in the project plan), not in
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
- The existing recorder and replay crates model recorded conversations; replay of an explorer path reuses the
  safety policy, so a replay is also read-only by default.

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
  observer used for settle gives the roots), by id, and merge them into the previous component list.
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
`traceqa explore <url> --seed 1 --max-actions 150` in read-only mode, in each of the three browsers.

**Acceptance.**
- Touches at least 80% of the discoverable components within 150 actions.
- The server log shows zero non-GET and non-HEAD requests.
- Two runs with the same seed give byte-identical `graph.json`.
- Each of the budgets in EXP-2 is respected (counts at or below the limit).
- "Clear cart" and the checkout-like controls are never activated.
- Replay of the recorded edges reproduces at least 95% of the transitions.
- A cancel at action 40 yields a folder that validates.

**Size.** S (on top of EXP-1 to EXP-9)

## 2. Interaction oracles (F7)

Interaction without checks finds nothing. These oracles run after each explorer action. They are cheap, general
and need no model. Findings use the existing findings model, keyed by component id and action, so they flow into
the same suppression, baseline and gate machinery as the static rules. Third-party noise is handled by an
origin allow-list (`--oracle-origin`, default the start origin). Signals that are flaky by nature are confirmed
by repeat runs (ORA-10).

Every oracle has a planted-bug fixture: a small page where the bug exists, plus an unmodified control page
where the oracle must stay silent.

### ORA-1: Dead control

**Why.** A button that does nothing is the commonest user-visible bug and the cheapest to detect.

**Design.** After a click, if the URL, the visible component id set, the modal marker and the DOM mutation
observer all report no change, and no request was made, the control is dead. Controls whose only effect is an
attribute change (such as `aria-pressed`) change the DOM and are therefore not dead. Rule id `dead_control`.

**Acceptance.**
- On a fixture with one button that has no handler and one with a handler, exactly one `dead_control` finding
  is reported, for the first.
- On the control page, zero findings.
- On the buggy shop in read-only mode, the finding ids are the same over 5 repeats.

**Size.** S

### ORA-2: Uncaught errors and console errors

**Why.** Script errors triggered by an interaction are real bugs, and often the only visible symptom.

**Design.** Uses `consoleErrors` from EXP-4. Uncaught exceptions are errors; `console.error` is a warning;
messages from non-allow-listed origins are dropped. Rule ids `uncaught_error`, `console_error`.

**Acceptance.** A fixture whose click handler throws gives one `uncaught_error` with the component id; a
fixture that calls `console.error` gives one `console_error`; a third-party script that logs from another origin
gives none. The `not_observed` marker appears on the WebDriver driver.

**Size.** S

### ORA-3: Failed requests

**Why.** A click that triggers a 500 or a network failure is a bug the page may hide from the user.

**Design.** Uses `failedRequests` from EXP-4. Status 400 and above on allowed origins, and network failures,
are findings, `failed_request`, with the redacted URL. Optional status allow-list (`--ignore-status 404` for
known probes).

**Acceptance.** A fixture whose button fetches a URL that returns 500 gives one finding with status 500; one that
returns 404 with `--ignore-status 404` gives none; a request to a third-party origin gives none by default.

**Size.** S

### ORA-4: Navigation to an error page

**Why.** A link that leads to a 404 page is a broken link.

**Design.** Uses `navigated.status` and the existing `http_error_status` logic. Rule id `error_page`. The edge
target state is marked as an error state and is not expanded.

**Acceptance.** A fixture with a link to a 404 page gives one `error_page` finding with status 404 and the
walker does not expand that state (no edges leave it).

**Size.** S

### ORA-5: Focus lost or trapped

**Why.** Keyboard users lose their place when focus vanishes after an action, or cannot leave a dialog.

**Design.** After an action, read the active element. Findings: `focus_lost` (focus moved to the body after an
action that removed the focused component), `focus_trapped` (in an open non-modal region, Tab does not leave the
region in 20 presses). For modal dialogs trapping is correct; the check is the other direction: Escape must
close it (`dialog_not_dismissible`).

**Acceptance.** Fixtures: a button that removes itself and leaves focus on the body gives `focus_lost`; a
non-modal panel with a Tab trap gives `focus_trapped`; a correct modal gives none; a modal that ignores Escape
gives one `dialog_not_dismissible`.

**Size.** M

### ORA-6: Layout shift

**Why.** A click that makes the page jump is a visible quality problem.

**Design.** Sum the movement of components that did not change (matching by id, using the bounding boxes already
in `components.json`) relative to the viewport height. Above a threshold (default 0.1) report `layout_shift`
with the largest movers. It uses boxes before and after, not the browser's layout-shift API, so it works in every
browser.

**Acceptance.** A fixture that injects a banner above the content on click gives `layout_shift` with a score above
0.1; a fixture that opens an overlay without moving content gives none.

**Size.** S

### ORA-7: Newly covered primary controls

**Why.** If an action covers a primary control with something that stays (a toast, a sticky bar), the user
cannot use it.

**Design.** After the action, run the existing covered-at-load rule logic on the new state and report
controls covered now but not before. Rule id `newly_covered`.

**Acceptance.** A fixture with a sticky bar that appears over the checkout button gives one `newly_covered`
finding naming that button; a fixture with a toast that disappears within the settle time gives none.

**Size.** S

### ORA-8: State flip not reflected in aria

**Why.** A disclosure that opens but keeps `aria-expanded="false"` misleads screen reader users.

**Design.** For components with `aria-expanded`, `aria-pressed` or `aria-checked` in the collector's state, check
that the value changed after a click that changed what is visible (for `aria-expanded`: the controlled region
changed visibility). Rule id `aria_state_not_updated`.

**Acceptance.** A fixture disclosure that shows its panel but keeps `aria-expanded="false"` gives one finding; a
correct disclosure gives none; a toggle that has no ARIA state gives none (that is a different, static rule).

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
rest as `intermittent`. Gating by k-of-n is CUT-3.

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
- `focus`: `tabindex` value, whether the element is focusable, and its position in sequential tab order
  (computed without moving focus).
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

### ARC-2: MCP progress notifications (rest of A8)

**Why.** An agent that calls `extract` for a long page sees nothing for a minute and may give up.

**Design.** When the request carries a progress token, send `notifications/progress` with the steps the
engine already reports (browser started, navigated, settled, extracted), rate-limited to one per second.

**Acceptance.** A test client that sends a progress token receives at least 3 progress notifications during an
extract of the buggy shop and none when it sends no token; each notification has an increasing `progress` value.

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

**Why.** Explicit annotations on public functions make the Python SDK and protocol packages readable without an
editor. They were left out when the lint rules were widened because pyright already checks types, and turning
on `ANN` at that point would have meant hundreds of mechanical edits.

**Design.** Add `ANN` to `select` in `pyproject.toml`. Fix public API first (`sdks/python/src`, `python/`),
then drivers and scripts. Ignore `ANN401` (explicit `Any`) where the protocol really is dynamic, each with a
one-line justification in the config. Tests may be exempt through `per-file-ignores`.

**Acceptance.**
- `uv run ruff check .` exits 0 with `ANN` selected.
- Every ignore for an `ANN` code in `pyproject.toml` has a comment.
- `uv run pyright` reports no new errors compared with before the change (compare counts).
- CI is green on the three operating systems.

**Size.** M

### HYG-2: A run.start racing the first signal can still be accepted (X19 residual)

**Why.** `traceqa serve` and `traceqa mcp` cancel all runs on a signal. A `run.start` request that arrives at
the same moment as the first signal can be accepted after the cancellation sweep, and then runs unguarded until
the escalation timer ends it.

**Design.** Set the cancel flag first, then take the runs lock and check the flag again before registering a run.
A start that sees the flag set answers with a "shutting down" error and starts nothing. The sweep takes the same
lock after setting the flag, so no run can be added after it.

**Acceptance.** A stress test sends the signal and a `run.start` concurrently, 200 times; every iteration ends
with no run left running, either a refused start or a started run that was then cancelled, and the process
exits within the documented bound. Run before the fix, the test fails in at least one of the 200 iterations.

**Size.** S

### HYG-3: Driver tree kill without `ps -A -o pid=,ppid=` (X20 residual)

**Why.** On Unix the engine kills the whole driver process tree by walking `ps`. On busybox (Alpine) `ps` does
not accept `-A -o pid=,ppid=`, so only the driver itself is killed and its browser child processes can be left
running.

**Design.** Fall back, in order: (1) read `/proc/<pid>/stat` for every pid and build the tree from the parent
field; (2) start each driver in its own session (`setsid`) so the whole group can be killed with one signal to
the negative pid. Prefer (2) for new starts and keep (1) as the fallback for the process list.

**Acceptance.** A test inside an Alpine container image (`alpine:3`) starts a driver that spawns two children,
cancels the run, and asserts that none of the three processes is left (process count of zero). The test fails on
the current code in the same image.

**Size.** M

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
