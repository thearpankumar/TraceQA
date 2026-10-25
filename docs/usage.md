# Usage

## Setup

With the `traceqa` binary (needs [uv](https://docs.astral.sh/uv/) for the drivers):

```sh
traceqa install chromium firefox webkit
traceqa doctor
```

From the repository (needs Rust stable and uv):

```sh
cargo build
uv sync
uv run playwright install chromium firefox webkit
```

`cargo build` makes `target/debug/traceqa`; `uv sync` puts the drivers and the Python SDK in `.venv`.

## Drivers

The engine starts browser drivers as separate programs, found in this order:

1. `--driver COMMAND` (repeatable).
2. `TRACEQA_DRIVERS`: command lines separated by `;`.
3. `traceqa-driver-playwright` and `traceqa-driver-webdriver` on `PATH` (inside the repository, `uv run` puts them
   there).
4. `~/.traceqa/env`, where `traceqa install` puts them (moved by `TRACEQA_HOME`).

These inputs, and `TRACEQA_UV` for `traceqa install`, decide which programs run, so they belong to whoever starts
TraceQA. Use absolute paths to pin the drivers. With `traceqa mcp` they go in the operator's server entry; the AI
cannot set any of them.

## `traceqa extract`

```
traceqa extract [OPTIONS] <URL>
```

| Option | Default | Meaning |
|---|---|---|
| `--browser LIST` | `chromium` | Browsers, comma separated. See [Browser values](#browser-values). A name given twice runs twice (`chromium-1280x800-2`). |
| `--viewport LIST` | `1280x800` | Sizes as `WIDTHxHEIGHT`, comma separated; each side 1 to 10000. |
| `--out DIR` | `runs` | Where the run folder is created. |
| `--headed` | off | Show the browser windows. |
| `--crops` | off | Also save `page.png` and one picture per component. Slower; see [Speed](#speed). |
| `--visible-only` | off | Leave out elements that are not visible. |
| `--axe` | off | Also run axe-core and write `axe.json` (needs `traceqa install --axe`). See [Accessibility checks with axe-core](#accessibility-checks-with-axe-core). |
| `--axe-file PATH` | none | Use this `axe.min.js`. Implies `--axe`. |
| `--max-scroll-steps N` | `50` | Screens to scroll through. `0` reads without scrolling. |
| `--timeout-ms MS` | `30000` | Time allowed for a page to load. |
| `--settle-ms MS` | `0` | Extra wait after load, for splash screens and timed intros. |
| `--parallel N` | half the cores, at most 4 | Browsers at the same time. `1` runs them one after another. |
| `--timings` | off | Print where each browser's time went. |
| `--json` | off | Print `run.json` alone on standard output; progress and `Run folder:` go to standard error. |
| `--storage-state FILE` | none | Start signed in. See [Pages behind a sign-in](#pages-behind-a-sign-in). |
| `--repeat N` | `1` | Read the page N times (1 to 20). See [`--repeat`](#--repeat-find-unstable-components). |
| `--fail-on-unstable` | off | With `--repeat`: exit 1 when a component is flagged. |
| `--driver COMMAND` | see [Drivers](#drivers) | Command that starts a driver. |

Every browser runs at every viewport. Options are checked first: a bad value or an unknown browser name exits 2 and
writes nothing (unknown names are not checked when you bring your own driver).

### Browser values

| Value | Opens |
|---|---|
| `chromium`, `chrome`, `msedge`, `firefox`, `webkit` | A browser launched through Playwright. |
| `cdp=ENDPOINT` | A running Chromium-family browser, over the DevTools Protocol (HTTP or WebSocket URL). |
| `safari` | Safari on macOS. Turn on "Allow remote automation" in Safari's Developer settings first. |
| `webdriver=URL` | A session on a W3C WebDriver server or cloud grid. |

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Every target completed. |
| `1` | At least one target failed. |
| `2` | Invalid request; nothing was run. |
| `130` | Stopped by Ctrl-C, Ctrl-Break, SIGTERM or SIGHUP (also for `serve`, `mcp`, `map` and `login`). |

The first signal stops within about a second, closes the browsers and marks unfinished targets `cancelled`; finished
targets keep their results. A second signal quits within about 2 seconds and kills the drivers and everything they
started. If cleanup hangs, the first signal acts as a second after 7 seconds, so `docker stop` (SIGKILL at 10 s)
still finds a closed run folder. Closing a Windows console window cannot be cancelled cleanly; use Ctrl-C.

### Example

```sh
traceqa extract https://example.com --browser chromium,firefox,webkit --viewport 1280x800,390x844 --crops
```

### `--repeat`: find unstable components

```sh
traceqa extract https://example.com --repeat 5 --fail-on-unstable
```

Runs the extraction N times. Each run is an ordinary run folder; `runs/g_<id>/` also holds `group.json` (the runs)
and `stability.json`: per target and component, `runsPresent` and `rate`, and these flags:

| Flag | Meaning |
|---|---|
| `unstable` | Found in some runs, not all. |
| `count_varies` | The number of instances differs. |
| `state_flaps` | `visible` or `enabled` differs. |
| `name_varies` | More than one name was seen. |

It prints the ten least steady components. `--json` prints `stability.json` only. Only completed targets count. Ctrl-C
finishes the group with the runs done so far.

## `traceqa map`

```sh
traceqa map https://shop.example --depth 2
```

Reads a page, follows its same-origin links and says which components are on which page and which are on every page.
It only loads pages (GET) and one `/robots.txt` per origin; it never clicks or submits.

| Option | Default | Meaning |
|---|---|---|
| `--depth N` | `1` | Link levels to follow (at most 5). `0` reads only the page. |
| `--max-pages N` | `50` | Most pages to read (at most 500). |
| `--delay-ms MS` | `500` | Pause between pages. |
| `--max-minutes N` | `10` | Time limit; unread pages are listed as skipped. |
| `--include GLOB`, `--exclude GLOB` | none | Path globs (`*` one folder, `**` across folders), repeatable. |
| `--no-robots` | off | Ignore robots.txt (recorded in `site.json`). |
| `--browser`, `--viewport`, `--storage-state`, `--out`, `--headed`, `--driver` | | As for `extract`. |
| `--json` | off | Print `site.json` only. |

- Paths matching `**/logout**`, `**/signout**`, `**/log-out**`, `**/delete**` and `**/unsubscribe**` are never
  followed, unless `--include` names them. Map only sites you may read.
- Links are taken from `link` components with an `href`, hidden ones included. Tracking parameters (`utm_*`, `fbclid`,
  `gclid`) are dropped; `mailto:`, `tel:`, `javascript:`, `data:` and files are skipped; at most 20 pages per path
  pattern (`/products/{n}`).
- robots.txt follows RFC 9309 for the token `traceqa`; a 5xx answer or no answer stops the crawl.

Each page is an ordinary run. `runs/g_<id>/site.json` lists the pages, each component's pages, `shared` (ids on every
page) and `skipped` links with reasons. A header whose landmark name changes per page gets different ids and is not
shared.

## `traceqa runs`

```sh
traceqa runs list  [--out runs] [--url URL] [--json]
traceqa runs stats [--out runs] [--url URL] [--json]
traceqa runs prune [--out runs] [--keep N] [--older-than AGE] [--url URL] [--dry-run] [--yes]
```

- `list`: runs and groups, newest first. `stats`: per address, run count, statuses, disk use, oldest and newest.
- `prune`: deletes runs beyond the newest `--keep N` per address and/or older than `--older-than` (`12h`, `30d`,
  `2w`); with both, a run must match both. Without `--yes` it prints the plan, deletes nothing and exits 2;
  `--dry-run` prints the plan and exits 0.
- `prune` only touches `r_*` and `g_*` folders in `--out` that hold their own `run.json` or `group.json`, never
  follows links, and keeps a `running` run unless it started over 24 hours ago.

In CI, end with `traceqa runs prune --keep 20 --older-than 14d --yes` so the runs folder does not grow without end.

## `traceqa diff`

Shows what differs between two finished runs: browsers, viewports or builds. No browser starts.

```sh
traceqa diff runs/before runs/after --target chromium-1280x800
traceqa diff runs/RUN runs/RUN --target-a chromium-1280x800 --target-b firefox-1280x800   # cross-browser
```

`A` and `B` are each a run folder, a target folder, a `components.json` or a run id in `--runs-dir`.

| Option | Default | Meaning |
|---|---|---|
| `--target`, `--target-a`, `--target-b` | none | Target folder on both sides, or on one. Needed when a run has several. |
| `--format markdown\|json\|github\|sarif` | `markdown` | `json` is a `DiffFile` (a `BaselineReport` with `--against`). See [the gate](#baselines-and-the-ci-gate) for `github` and `sarif`. |
| `--against NAME\|DIR` | none | Compare a run with a saved baseline (side A). |
| `--baseline-dir DIR` | `.traceqa/baselines` | Where `--against NAME` looks. |
| `--fail-on TOKENS` | none | Exit 1 on `removed`, `added`, `changed`, `disabled`, `hidden`, `covered` or `paired` entries. |
| `--out FILE` | stdout | Write the report to a file. |
| `--runs-dir DIR` | `runs` | Where run ids are looked up. |

- Components are matched by id: only in B is `added`, only in A `removed`, different `changed` (instances, visible,
  enabled, covered, checked, expanded, selected, pressed, confidence). Positions and pictures are not compared.
- Changed entries carry flags such as `became_hidden`, `became_disabled`, `count_up`.
- A renamed or re-roled component is a removed and an added entry linked by `pairedWith`.
- Markdown shows 50 rows per table. The same inputs always give the same output.

Exit codes: `0` the diff ran; `1` `--fail-on` matched; `2` input that cannot be compared (missing path, several
targets without `--target`, different `fingerprintVersion`, unknown token).

## Baselines and the CI gate

A baseline is a saved copy of a run's `components.json` files (and crops) in `.traceqa/baselines/NAME/`, meant to be
committed: it holds no `run.json`, sign-in state or error text. Only `traceqa baseline` changes it.

```sh
traceqa baseline save runs/RUN --name main             # refuses when "main" exists
traceqa diff runs/NEW --against main --fail-on removed,disabled
traceqa baseline promote runs/NEW --name main --yes    # without --yes: shows the diff, exits 2
```

| Command | What it does |
|---|---|
| `baseline save RUN [--name] [--dir] [--runs-dir]` | Makes a baseline (default name: the host plus 8 hex characters of the address). Fails if it exists. |
| `baseline promote RUN [--name] [--dir] [--runs-dir] --yes` | Replaces it and records the promotion. Without `--yes` it prints the difference and exits 2. |
| `baseline alias FROM TO --name NAME [--dir] [--evidence TEXT] [--reason rename\|heal\|manual]` (default `rename`) | Records that baseline component `FROM` is now `TO`. |
| `baseline init RUN --out FILE [--viewport-name 1280x800=desktop]... [--force] [--runs-dir]` | Writes a `verify` manifest from what the run found. Review it before use. |

- `diff --against` compares each baseline target with the run's target of the same name; a missing target or another
  `fingerprintVersion` exits 2.
- `--fail-on`: `disabled`, `hidden` and `covered` match the `became_*` flags; `paired` matches renames found by
  heuristic; a rename recorded as an alias matches nothing.
- `--format github` prints one annotation per entry (at most 50, removed/disabled/hidden/covered as errors) and adds
  the report to `$GITHUB_STEP_SUMMARY`. `--format sarif` writes SARIF 2.1.0 for SARIF viewers, not GitHub code
  scanning. `traceqa findings` has the same two formats.

```yaml
name: components
on: pull_request
permissions:
  contents: read
jobs:
  gate:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      # install TraceQA here
      - run: traceqa extract "$STAGING_URL" --browser chromium --viewport 1280x800
        env:
          STAGING_URL: ${{ vars.STAGING_URL }}
      - run: traceqa diff "$(ls -d runs/* | tail -n 1)" --against main --fail-on removed,disabled,hidden --format github
```

When a change is intended, run `traceqa baseline promote RUN --name main --yes` and commit the baseline.

## `traceqa visual`

Compares the component pictures of two runs pixel by pixel, to catch style changes `diff` cannot see. Both runs need
`--crops`.

```sh
traceqa visual runs/before/RUN runs/after/RUN --target chromium-1280x800
traceqa visual runs/RUN --against main        # the baseline must have crops
```

| Option | Default | Meaning |
|---|---|---|
| `--target NAME` | none | Target folder; needed when a run has several. |
| `--threshold N` | `0.2` | How far two pixels' colours may differ, 0 to 1 (as in Playwright). |
| `--max-diff-ratio R` | `0.001` | A component is the same when at most this share of pixels differ. |
| `--out DIR` | `./visual-diff` | Where `TARGET/ID.diff.png` pictures of differing components go. |
| `--format markdown\|json` | `markdown` | `json` is a `VisualReport` (`spec/visual.schema.json`). |
| `--fail-on-different` | off | Exit 1 when a component differs. |
| `--against`, `--baseline-dir`, `--runs-dir` | as in `diff` | Compare with a baseline. |

- Only the same browser and viewport can be compared; use `traceqa diff` across browsers.
- Only components the structural diff found unchanged are compared; the others are listed with a reason. Different
  browser major versions mark results `unreliable`.

Exit codes: `0` the comparison ran; `1` `--fail-on-different` matched; `2` input that cannot be compared.

## `traceqa verify`

Checks a finished run against a manifest: what must exist, must not exist, and which warnings are allowed
(example: `testbed/sites/buggy-shop/manifest.yaml`; schema: `spec/manifest.schema.json`). `traceqa baseline init` writes a
first one.

```sh
traceqa verify RUN --manifest FILE [--target NAME] [--strict] [--format text|json|github|sarif] [--runs-dir DIR]
```

| Check | Fails when |
|---|---|
| `missing` | A listed component is not found (the message names the nearest one of the same role). |
| `present` | An `absent` entry (role and exact name) is found. |
| `instances`, `confidence`, `disabled` | The count, confidence or disabled count differs (defaults 1, `high`, 0). |
| `hidden`, `visible` | A component listed hidden at this viewport is visible, or an unlisted one is not. |
| `frame`, `shadow` | Its frame, or whether it is in shadow DOM, differs. |
| `warning` | A warning code was raised that `warnings` does not list. |
| `extra` | With `--strict`: a component no entry matched (otherwise a warning). |

Each target is checked at the manifest viewport of the same size; a target at another size is skipped with a note.
Components are matched by role, then exact name (or any name with `template: true`); ids are not used. Unknown
manifest keys are errors. `sarif` output is for SARIF viewers, not GitHub code scanning.

Exit codes: `0` every target passes; `1` a target fails; `2` the manifest or run cannot be used.

```text
$ traceqa verify runs/r_20261013_210455_a1b2 --manifest testbed/sites/buggy-shop/manifest.yaml
chromium-1280x800 (desktop): FAIL, 37 components checked, 1 failures
  FAIL missing at desktop: button 'Open the size guide' x1 (nearest: button 'Open size guide' c_4f1a...)
```

## `traceqa serve`

```
traceqa serve [--max-runs N] [--driver COMMAND]...
```

Speaks the client protocol (JSON-RPC, one message per line) on standard input and output; the SDKs start the engine
this way. See [architecture.md](architecture.md#client-protocol) and [clients.md](clients.md).

| Option | Default | Meaning |
|---|---|---|
| `--max-runs N` | `4` | Runs at once. More get a retryable `busy` error. |
| `--driver COMMAND` | see [Drivers](#drivers) | Command that starts a driver (repeatable). |

- Methods: `engine.hello` (lists `features`), `run.start`, `run.cancel`, `engine.shutdown`, `diff.compare`,
  `findings.analyse`; notifications `run.event` and `run.done`.
- A line over 8 MiB gets error `-32600` and the next line is read. A viewport side outside 1 to 10000 gets `-32602`.
- To close, send `engine.shutdown`, then end the input: running runs finish. If input ends without it, every run is
  cancelled and closed, so no browser is left behind.
- Signals: the first cancels every run and exits 130 once they have closed; a second, or 7 seconds, quits at once.

## `traceqa mcp`

An MCP server for AI tools such as Claude Code, Claude Desktop and Cursor, over standard input and output.

```sh
claude mcp add traceqa -- traceqa mcp
```

Other AI tools take the program `traceqa` with the argument `mcp`.

| Option | Default | Meaning |
|---|---|---|
| `--out DIR` | `runs` | Where runs are written and read. |
| `--state-dir DIR` | off | Folder of saved sign-in states the AI may name (must exist). Without it, `storage_state` is not offered. |
| `--suppressions FILE` | none | Suppression file for the `findings` tool, checked at start and re-read on each call. |
| `--allow-remote-browsers` | off | Allow `cdp=` and `webdriver=` browsers. |
| `--block-private` | off | Also refuse loopback, private, shared, unique-local and multicast addresses. |
| `--allow-host HOST` | none | With `--block-private`, still allow this host. Repeatable. |
| `--max-concurrent-extracts N` | `2` | Extractions at once; more get a `busy` error at once. |
| `--driver COMMAND` | see [Drivers](#drivers) | Command that starts a driver. Repeatable. |

### Tools

| Tool | What it does |
|---|---|
| `extract` | Opens `url` in `browsers` at `viewports` (headless) and returns the run id and a summary per target. Optional: `crops`, `settle_ms`, `max_scroll_steps`, `timeout_ms`, `include_hidden`, `storage_state` (a file name in `--state-dir`). |
| `list_runs` | Earlier runs, newest first. |
| `describe_page` | One target's components grouped by landmark; repeated cards shown once. |
| `find` | Searches every target of a run by `role`, `name`, `query`, `landmark` or `state` (`visible`, `hidden`, `enabled`, `disabled`, `covered`, `checked`, `unchecked`, `expanded`, `collapsed`, `required`), with where each match is found and missing. |
| `get_components` | One target's components, filtered by `role`, `name` or `landmark` and paged; or one in full with `component_id`. Rows carry a Playwright `locator`. |
| `diff_runs` | What changed between two runs or two targets (`run_a`, `run_b`, `target_a`, `target_b`). |
| `findings` | The checks of `traceqa findings`, computed on demand (filters `target`, `min_severity`, `rules`); not written. |
| `get_crop` | The picture of one component, for runs made with `crops`. |

- `response_format`: `concise` (default) or `detailed`. Lists take `limit` (at most 500) and `offset`; a cut list
  says how to narrow it.
- Hosts on MCP 2025-06-18 or later also get output schemas and structured content.
- Resources: `traceqa://runs/{run_id}`, `traceqa://runs/{run_id}/{target}/components` and
  `traceqa://runs/{run_id}/{target}/crops/{component_id}`.
- Progress is reported during `extract` when the host asks; a cancelled call stops its run.

For signed-in pages, save a state into the `--state-dir` folder and let the AI pass its file name:

```sh
traceqa login https://shop.example --save ~/.traceqa/states/shop.state.json
claude mcp add traceqa -- traceqa mcp --state-dir ~/.traceqa/states
```

### Protections

- Cloud metadata and link-local addresses (`169.254.0.0/16`, `100.100.100.200`, `168.63.129.16`, `192.0.0.192`,
  `fd00:ec2::254`, `0.0.0.0/8`, `::`, `fe80::/10`, `metadata.google.internal`, `metadata.goog`) are always refused,
  however they are written, and every address a name resolves to is checked.
- Localhost and private networks work unless `--block-private` is given.
- Redirect hops and the address the browser connected to are checked after loading, so the request has already
  been sent; a refused one fails the target with `blocked_url` and nothing is written. WebDriver browsers report
  only the final address. Subresources and frames (images, scripts, `fetch`, iframes) are not
  covered by the URL policy, so run the server where those requests are harmless.
- Remote browsers are refused unless `--allow-remote-browsers` is given. The AI can only name a regular file inside
  `--state-dir` (letters, digits, `.`, `_`, `-`, starting with a letter or digit; links refused); errors never show
  its path or contents.
- Page text is marked untrusted (`"untrusted_page_content": true`) and cleaned of control, zero-width and bidi
  characters. Hidden elements are left out unless `include_hidden` is set.
- Answers are capped at 100 KB, pictures at 2 MiB, error texts at 500 characters, request lines at 8 MiB; `find`
  reads at most 12 targets, a component shows at most 50 instances, and at most 16 reading calls run at once. No
  answer shows a local path.

Signals: the first cancels running calls and refuses new ones, then exits 130; a second, or 7 seconds, quits at once.
A missing `--state-dir` or unusable `--suppressions` file exits 2.

## `traceqa install`

```
traceqa install [BROWSERS]... [--from FOLDER] [--ref REF] [--with-deps] [--dry-run]
traceqa install --axe [--dry-run]
```

Installs the drivers into `~/.traceqa/env` (or `$TRACEQA_HOME/env`) and downloads the browsers. Needs
[uv](https://docs.astral.sh/uv/). Browsers: `chromium` (default), `chrome`, `msedge`, `firefox`, `webkit`, `safari`
(nothing to download). The driver dependencies are pinned with hashes, and TraceQA's own packages come from the commit
the binary was built from. Run it again to update.

| Option | Meaning |
|---|---|
| `--from FOLDER` | Install the drivers from a TraceQA checkout instead of GitHub. |
| `--ref REF` | Branch, tag or commit to install from. Only a full 40-digit commit id is pinned. |
| `--with-deps` | Also install the browsers' system libraries (Linux, may ask for `sudo`). |
| `--dry-run` | Print what would run, change nothing. |
| `--axe` | Install only axe-core; see [below](#accessibility-checks-with-axe-core). |

## `traceqa doctor`

Shows the build, the TraceQA folder, uv, each driver and whether each browser really opens (a headless window, one to
three seconds each), with the install command for anything missing. `cdp` and `webdriver` are not tried; Safari only on
macOS. `--verbose` shows the drivers' log lines; `--driver COMMAND` checks a given driver. Exit code 0 when at least one browser opens, 1 when none does.

## `traceqa findings`

```
traceqa findings [OPTIONS] <RUN>
```

Automated checks over a finished run. It prints the findings and writes `findings.json` next to each `components.json`.
These are automated checks, not a conformance claim.

| Option | Default | Meaning |
|---|---|---|
| `--target NAME` | all | Only this target. |
| `--suppressions FILE` | `.traceqa/suppressions.yaml` if present | See [Suppressions](#suppressions). |
| `--min-severity LEVEL` | `info` | Leave out findings below `info`, `warning` or `error`. |
| `--fail-on error\|warning\|info\|never` | `never` | Exit 1 when a finding of this severity or worse is left. |
| `--format FORMAT` | `table` | `table`, `json`, `markdown`, `github` or `sarif` (for SARIF viewers, not GitHub code scanning). |
| `--no-write` | off | Only print. |

Exit codes: `0` nothing at or above `--fail-on`; `1` something is; `2` not a run folder, an unknown target or a bad
suppression file. A finding's id is `<rule>:<componentId>`, the same in every browser and run. Only visible instances
are checked.

### Rules

| Rule | Severity | WCAG | Finds |
|---|---|---|---|
| `unnamed-control` | `error` (`warning` when interactivity is unsure) | 4.1.2 | A control with no accessible name. |
| `weak-name-source` | `warning` | 4.1.2, 3.3.2 | A name that comes only from `placeholder` or `title`. Skipped, with warning `name_source_missing`, on files from older engines. |
| `fake-button` | `error` if not keyboard reachable, else `warning` | 4.1.2, 2.1.1 | Something clickable by pointer or `onclick` only, with no native control or role. |
| `small-target` | `warning` | 2.5.8 | A target with a side under 24 CSS px, too close to another target. |
| `covered-at-load` | `warning` | | A visible control covered at every point tested. |
| `ambiguous-names` | `info` | 2.4.4 | Links with the same name and different destinations. |
| `missing-autocomplete` | `info` | 1.3.5 | A `password`, `email` or `tel` input without `autocomplete`. |
| `browsers-disagree` | `warning`, or `info` for counts only | | A component present, visible, enabled or counted differently across browsers at one viewport. Written once for the run; its id ends in `@WxH`. |

With `extract --axe`, each axe rule also becomes findings; see [axe findings](#axe-findings).

### Suppressions

```yaml
# .traceqa/suppressions.yaml
suppressions:
  - id: small-target:c_9f2a41c07b3e        # or: rule + component
    reason: Inline link inside a sentence.
  - rule: fake-button
    component: c_3b33554f14cd
    reason: Legacy tile, replaced in Q1.
    expires: 2027-03-31                     # optional
```

`reason` is required; unknown keys and files over 256 KiB are refused. A suppressed finding moves to `suppressed` and does not count for
`--fail-on`. Expired entries stop applying (warning `suppression_expired`); entries that match nothing get
`suppression_unused`.

## Accessibility checks with axe-core

```sh
traceqa install --axe
traceqa extract https://example.com --axe
traceqa findings runs/<runId>
```

- Off by default. axe-core (MPL-2.0) is never bundled: `traceqa install --axe` downloads version 4.14.0, checks its
  SHA-512 and writes it to `~/.traceqa/axe/4.14.0/`. `TRACEQA_AXE_TARBALL_URL` uses a mirror (the hash is still
  checked). See [third-party-notices.md](third-party-notices.md).
- `--axe-file PATH` or `TRACEQA_AXE_FILE` uses another copy. `--axe` with no copy exits 2 before anything runs.
- Rule tags: `wcag2a`, `wcag2aa`, `wcag21a`, `wcag21aa`, `wcag22aa`, `best-practice`. It runs in every reachable frame,
  and a page's Content-Security-Policy does not block it. If it cannot run, the target gets `axe_failed` and the run
  still completes.
- Available from the CLI and `traceqa serve` (`options.axe` in `run.start`); the SDKs and the MCP server have no axe
  option.
- Automation finds only part of the problems: contrast over images, meaning, alt text, keyboard order, focus and
  closed menus need a person.

### `axe.json`

Per target: `axeVersion`, `tags`, `violations` (rule, impact, tags, help, `helpUrl` and the failing elements, each
placed on a `componentId` when a component contains it), `incomplete` (results left for a person) and `errors`.

### axe findings

`traceqa findings` reports each axe rule per component as `axe.<ruleId>:<componentId>` (or `:page` when no component
contains the element). `critical` and `serious` are `error`, `moderate` is `warning`, `minor` is `info`. `wcag` lists
the success criteria from axe's tags. Suppressions work on these ids too.

## `traceqa codegen`

Writes a Playwright page object from a finished run: one class, one property per component.

```sh
traceqa codegen runs/RUN --lang py --out tests/pages --target chromium
```

| Option | Default | Meaning |
|---|---|---|
| `--lang ts\|py` | required | TypeScript (`<name>.page.ts`, `@playwright/test`) or Python (`<name>_page.py`, sync API). |
| `--out DIR` | `./traceqa-pages` | Where to write. |
| `--target NAME` | every completed target | Folder (`chromium-1280x800`) or browser (`chromium`). Repeatable. |
| `--class-name NAME` | from the page title | The class name. |
| `--include-hidden` | off | Also write components that are hidden everywhere. |

- Locators prefer role with exact name, then a shared `data-testid`, then role inside named ancestors, then exact text,
  and last a CSS selector marked `FORCED`. `nth()` is never written.
- A locator is `unique` only if it is unique in every selected browser; otherwise the property is marked `DIFFERS`,
  `AMBIGUOUS` or multi-match.
- Components in iframes go through a `frameByLabel` / `frame_by_label` helper in the file.
- Each property has a `// traceqa:id c_...` marker, which `traceqa heal` uses. Names are matched exactly, so a page
  object is for the page's language.
- Page text appears only as escaped string literals.
- `locators.json` next to it lists every property, its locator and where it is unique.

Exit codes: `0` written; `2` wrong command or a run made before locators existed (extract again).

## `traceqa heal`

Compares a run with a baseline and sorts what changed into fixes, proposals and failures. It never starts a browser and
never changes the baseline's components.

```sh
traceqa heal runs/RUN --against main --accept c_8a1f03b2c4d5 --page-object tests/pages/shop.page.ts
```

| Option | Default | Meaning |
|---|---|---|
| `--against NAME\|DIR` | required | The baseline. |
| `--baseline-dir DIR` | `.traceqa/baselines` | Where `--against NAME` looks. |
| `--target NAME` | all | One baseline target only. |
| `--page-object PATH` | none | A page object from `traceqa codegen` to patch (`.ts` or `.py`). Repeatable. |
| `--accept ID` | none | Accept the rename proposal for baseline component `ID`. Repeatable. |
| `--allow-in-ci` | off | Allow `--accept` and page-object writes when `CI` or `GITHUB_ACTIONS` is `true`. |
| `--format markdown\|json` | `markdown` | `json` is a `HealReport` (`spec/heal.schema.json`). |
| `--runs-dir DIR` | `runs` | Where run ids are looked up. |

| Outcome | When | Action |
|---|---|---|
| `locator_fix` | Same id, different locator. | fix |
| `rename_proposal` | The id is gone and exactly one run component matches (same role and landmark, and the same `data-testid` or a close name and position). | propose |
| `removed` | The id is gone and nothing matches safely. | fail |
| `ambiguous` | More than one candidate. | refuse |
| `session_not_accepted` | The run shows a sign-in page. Sign in again and extract again. | refuse |

- A rename is never applied by itself: `--accept ID` records it as a baseline alias and patches the page objects.
- In CI, `--accept` and page-object writes exit 2 unless `--allow-in-ci` is given.
- Patching changes only the lines codegen wrote under the component's marker; hand-edited lines are kept and reported.
- The report is also written to `heal.json` in the run folder.

Exit codes: `0` only fixes (or accepted proposals); `1` anything else to decide; `2` unusable input.

## Pages behind a sign-in

A state file holds the cookies and stored values of a signed-in browser (Playwright's `storageState` format).

```sh
traceqa login https://app.example.com --save state.json [--browser NAME] [--headless] [--driver COMMAND]
traceqa extract https://app.example.com/account --storage-state state.json
```

`login` opens a browser window; sign in, then press Enter. TraceQA never sees the password. A state saved in one
browser works in the others (through WebDriver, only the final site's state is saved). `--headless` saves once the page
has loaded. Exit codes: `2` for a bad `--save` path or address, `130` on Ctrl-C (nothing saved).

A state file can also be written by hand:

```json
{ "cookies": [], "origins": [{ "origin": "https://app.example.com", "localStorage": [{ "name": "has_onboarded", "value": "true" }] }] }
```

- Treat it as a password. `login` writes it owner-only (mode `600`; owner-only `icacls` on Windows), and `.gitignore`
  ignores `state.json`, `*.state.json` and `storage-state*.json`. In CI, keep it in secrets.
- TraceQA never writes the state to a run folder, event, log or error.
- A run that still lands on a sign-in page gets `session_not_accepted`: sign in again.
- With `--crops`, pictures of signed-in pages may show personal data (`signed_in_pictures`); keep such run folders
  private.

## Python SDK

```python
import traceqa

result = traceqa.extract("https://example.com", browsers=["chromium", "firefox"])

with traceqa.Engine() as engine:  # several calls on one engine process
    first = engine.extract("https://example.com")
```

| Option | Default | Meaning |
|---|---|---|
| `browsers` | `("chromium",)` | Browser values as on the command line, or `BrowserTarget` objects. |
| `viewports` | `((1280, 800),)` | `(width, height)` pairs, each side 1 to 10000. |
| `out_dir` | `runs` | Where the run folder is created. |
| `headless` | `True` | Hide the browser windows. |
| `crops` | `False` | Also save pictures. |
| `include_hidden` | `True` | Include elements that are not visible. |
| `max_scroll_steps`, `timeout_ms`, `settle_ms` | `50`, `30000`, `0` | As on the command line. |
| `parallel` | from the machine | Browsers at once. |
| `storage_state` | none | A state file path, a dict or a `StorageState`. |
| `on_event` | `None` | Called with each progress event. |
| `on_log`, `binary`, `drivers` | | Also on `Engine`: log-line callback (off by default), the engine path (else `TRACEQA_BIN`, then `PATH`), driver commands. |

- `ExtractResult`: `status`, `targets`, `manifest`, `run_folder` and `components(target_dir)`. A failed browser does
  not raise; check `status`.
- `engine.diff(a, b)` returns a `DiffFile` and `engine.findings(run)` a `FindingsResult`, without a browser.
- `engine.cancel()` from any thread ends the run as `cancelled`; Ctrl-C cancels and re-raises. One extraction at a
  time per `Engine`. `close(timeout=30.0)` stops the engine.
- Errors: `TraceQAError` (with `kind`; a caller's mistake), `EngineStartError`, `EngineExitedError` (`exit_code`,
  `stderr_tail`), `EngineRefusedError` (`kind`, `code`), `EngineProtocolError`.

## TypeScript SDK

Node 24 or later, ESM only, no runtime dependencies (from CommonJS, see [its README](../sdks/typescript/README.md#esm-only)).

```ts
import { extract, Engine } from "traceqa";

const result = await extract("https://example.com", { browsers: ["chromium", "firefox"] });
const components = await result.components("chromium-1280x800");

const engine = await Engine.start();   // several calls on one engine process
try {
  await engine.extract("https://example.org", { browsers: ["webkit"] });
} finally {
  await engine.close();
}
```

- Options are the Python ones in camel case (`outDir`, `includeHidden`, `onEvent`, `onLog`...), plus `signal`, an
  `AbortSignal` that cancels the run. A throwing `onEvent` cancels the run and rejects `extract` with its error.
- One engine can run several extractions at once. `engine.close({ timeoutMs })` (default 30000) stops it.
- `engine.diff(a, b, { targetA, targetB })` and `engine.findings(run, { target, suppressions })` as in Python.
- Errors are the Python classes with camel-case fields (`exitCode`, `stderrTail`). The spec types are exported,
  all under `protocol`.

## Other languages

Any language that can start a process and read and write lines can use the engine. See [clients.md](clients.md).

## Output files

```
runs/<runId>/
  run.json
  events.jsonl
  <browser>-<width>x<height>/
    components.json
    axe.json                  with --axe
    findings.json             after `traceqa findings`
    page.png                  with --crops
    crops/<componentId>.png   with --crops
```

- `run.json`: run id, URL, times, status, and per target its status, counts, duration, error and `timings`
  (`openMs`, `loadMs`, `readMs`, `scrollMs`, `screenshotMs`, `cropMs`, `screens`).
- `events.jsonl`: one event per line, numbered from 0 (`run.*`, `target.*`, `page.*`, `elements.collected`,
  `components.grouped`, `crop.saved`, `warning`, `error`). The same events go live to the SDKs.
- Secret query and fragment values (`token`, `access_token`, `api_key`, `password`, `secret`, `sig`, `x_amz_*` and
  similar) become `REDACTED` in every file. A URL with a user name or password is refused (exit 2).

### `components.json`

| Field | Meaning |
|---|---|
| `id` | `c_` plus 12 hex characters, the same across browsers, viewports and runs. |
| `role`, `name` | ARIA role and accessible name; `name` is `*` when `nameIsTemplate` (repeated cards). |
| `confidence`, `signals` | `high` for real controls, `low` for elements that only look clickable, and why. |
| `path` | Landmarks and regions it sits in, e.g. `list "products" > listitem`. |
| `crop` | Its picture, with `--crops`. |
| `locator` | How to find it with Playwright; see [Locator](#locator). |
| `instances` | Each element: `name`, `nameSource`, `tag`, `attrs`, `selectors`, `box`, `state`, `frameId`, `inShadowDom`. |

### Locator

`strategy` (`role`, `testid`, `text` or `css` when `forced`), `scope` (ancestors to search inside), `frame`,
`unique`, `matches`, `ambiguousWith` and `needsIncludeHidden`. Uniqueness holds for this page in this browser;
`codegen` combines the browsers.

### Warnings

| Code | Meaning |
|---|---|
| `unnamed_interactive_element` | A control has no accessible name. |
| `cross_origin_frame_skipped` | A frame from another origin could not be read. |
| `frame_eval_failed` | A same-origin frame could not be read. |
| `scroll_limit_reached` | The page is longer than the scroll limit. |
| `elements_removed_while_scrolling` | The page removed rows while scrolling (virtual lists); those are not listed. |
| `requests_failed` | Scripts, styles, data or frames failed to load (Playwright browsers). |
| `http_error_status` | The page answered with an HTTP error; it was still read. |
| `page_load_incomplete` | Loading had not finished when time ran out; read as it stood. |
| `closed_shadow_roots` | Closed shadow roots hold content that cannot be read (Chromium only). |
| `covered_elements` | Something (a cookie banner, a modal) covers some controls (`covered: true`). |
| `session_not_accepted` | A run with a state still landed on a sign-in page. |
| `signed_in_pictures` | Pictures were taken while signed in. |
| `collector_tampered` | The page replaced built-in functions; with WebDriver browsers the result may be wrong. |
| `viewport_not_reached` | The browser could not take the asked size; read at its real size. |
| `screenshot_failed` | A screenshot failed; some crops are missing. |
| `axe_failed` | axe-core did not run on a frame. |
| `browsers_disagree` | In `events.jsonl`: browsers found different components at one viewport (names the missing ids). |

## Speed

- Browsers run side by side (`--parallel`); a run takes about as long as its slowest browser.
- Pictures are off by default. `--crops` reads and photographs a screen at a time, which is much slower on long pages.
- `--timings` shows where the time went.

## Pages that are not ready when they load

TraceQA waits for load, a quiet network, a stable height and running animations. For a splash screen held by a timer
(a `browsers_disagree` warning is the sign), add time:

```sh
traceqa extract https://example.com --settle-ms 5000
```

## Demo site and bench

```sh
uv run python testbed/sites/buggy-shop/serve.py     # http://127.0.0.1:4173/
uv run testbed/bench/run.py --browsers chromium --repeats 1 --json scores.json
```

The bench extracts the demo shop and scores it against `testbed/sites/buggy-shop/manifest.yaml`: found, extra, wrong, id
stability and time. It exits non-zero when a score falls short.
