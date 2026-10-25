# Usage

## Setup

### With the `traceqa` binary

Requirement: [uv](https://docs.astral.sh/uv/), which installs the drivers.

```sh
traceqa install chromium firefox webkit
traceqa doctor
```

See [`traceqa install`](#traceqa-install).

### From the repository

Requirements: Rust (stable) and uv.

```sh
cargo build
uv sync
uv run playwright install chromium firefox webkit
```

`cargo build` produces the engine at `target/debug/traceqa`. `uv sync` installs
the browser drivers and the Python SDK into `.venv`.

## Drivers

The engine starts browser drivers as separate programs. It finds them in this
order:

1. `--driver COMMAND`, which may be repeated.
2. The `TRACEQA_DRIVERS` environment variable: command lines separated by `;`.
3. `traceqa-driver-playwright` and `traceqa-driver-webdriver` on `PATH`.
4. The same two drivers where `traceqa install` put them.

Inside the repository, `uv run` puts both drivers on `PATH`:

```sh
uv run cargo run -- extract https://example.com
```

### Which programs TraceQA runs

TraceQA starts programs on your machine: the drivers, and, for `traceqa
install`, `uv`. Which program it starts is decided by the list above and by
these inputs, all of which belong to whoever starts the process:

| Input | What it decides |
|---|---|
| `--driver COMMAND` | The driver command line, used as given. Highest priority. |
| `TRACEQA_DRIVERS` | Driver command lines separated by `;`, used when no `--driver` is given. |
| `PATH` | Where `traceqa-driver-playwright`, `traceqa-driver-webdriver` and `uv` are looked for. |
| `TRACEQA_HOME` | Moves `~/.traceqa`, where the installed drivers are looked for last. |
| `TRACEQA_UV` | The `uv` that `traceqa install` runs, instead of the one on `PATH`. |

Treat each of them as an input that can run code: whoever can set one in the
environment of the `traceqa` process, or put a program earlier on `PATH`, picks
the program that runs. TraceQA does not guard against that, because the process
runs with the rights of the person who started it and could not tell a wanted
change from an unwanted one. It follows that:

- With `traceqa mcp`, these belong in the operator's server entry (the `env`
  block or the command line in the AI tool's settings). The AI cannot set any of
  them: no tool argument reaches the environment, a driver command or a path.
- To pin the drivers, give absolute paths: `--driver /opt/traceqa/env/bin/traceqa-driver-playwright`
  (or the same in `TRACEQA_DRIVERS`). A bare name is searched on `PATH`.
- `traceqa install` puts the drivers in `~/.traceqa/env` and installs only what
  the binary's own release pins (see below).

The order is not changed to prefer `~/.traceqa/env` over `PATH`. Someone who
develops TraceQA keeps a checkout's drivers on `PATH` (with `uv run`); an
installed copy that came first would silently replace the code being worked on.

Arguments are fixed by TraceQA and never taken from a page or from the AI. On
Windows, starting a batch file (`.cmd`, `.bat`) with arguments was unsafe in Rust
before 1.77.2 (CVE-2024-24576, "BatBadBut"), so Rust 1.77.2 or newer is required
there. The oldest version this project declares (`rust-version`, 1.88) is newer,
so a binary built from this source is not affected.

## `traceqa extract`

```
traceqa extract [OPTIONS] <URL>
```

| Option | Default | Meaning |
|---|---|---|
| `--browser LIST` | `chromium` | Browsers to run in, comma separated. See [Browser values](#browser-values). |
| `--viewport LIST` | `1280x800` | Viewport sizes as `WIDTHxHEIGHT`, comma separated. Each side is 1 to 10000. |
| `--out DIR` | `runs` | Where the run folder is created. |
| `--headed` | off | Show the browser windows. |
| `--crops` | off | Also save pictures: a screenshot of the page and one crop per component. Slower on long pages: see [Speed](#speed). |
| `--visible-only` | off | Leave out elements that are not visible. |
| `--axe` | off | Also check every page with axe-core and write `axe.json`. axe-core is not shipped: run `traceqa install --axe` first. See [Accessibility checks with axe-core](#accessibility-checks-with-axe-core). |
| `--axe-file PATH` | none | Use this `axe.min.js` instead of the installed one. Implies `--axe`. |
| `--max-scroll-steps N` | `50` | How many screens to scroll through on a long page. `0` reads the page without scrolling. |
| `--timeout-ms MS` | `30000` | Time allowed for a page to load. At least 1. |
| `--settle-ms MS` | `0` | Extra time to wait after the page has loaded, before reading it. For pages with a timed intro or splash screen. |
| `--parallel N` | chosen from the machine | How many browsers run at the same time. `1` runs them one after another. The default is half the machine's cores, at most 4. At least 1. |
| `--timings` | off | Also print where each browser's time went. |
| `--json` | off | Print the run manifest as JSON instead of a table. Standard output holds that one document and nothing else; progress and the `Run folder:` line go to standard error. |
| `--storage-state FILE` | none | Start every browser with the cookies and stored values in FILE, for pages behind a sign-in. See [Pages behind a sign-in](#pages-behind-a-sign-in). |
| `--repeat N` | `1` | Read the page N times (1 to 20) and report how often each component was found. See [`--repeat`](#--repeat-find-unstable-components). |
| `--fail-on-unstable` | off | With `--repeat`: exit with 1 when any component carries a flag. |
| `--driver COMMAND` | see above | Command that starts a browser driver. |

Every browser is run at every viewport, several at a time. Progress is written
to standard error, each line marked with the browser it is about, and the
result to standard output.

Options are checked before anything runs: a zero `--timeout-ms` or
`--parallel`, a viewport side outside 1 to 10000, or a browser name that the
default drivers do not open (see below) exits with code 2 and leaves no run
folder. When you configure your own driver (`--driver` or `TRACEQA_DRIVERS`),
browser names are not checked up front, because that driver may open others..

If the run folder cannot be created, the message starts `--out PATH:` and the
exit code is 2.

Naming a browser twice (`--browser chromium,chromium`) runs it twice. The
second target's folder ends in `-2` (`chromium-1280x800-2`).

### Browser values

| Value | Opens |
|---|---|
| `chromium`, `chrome`, `msedge`, `firefox`, `webkit` | A browser launched through Playwright. |
| `cdp=ENDPOINT` | A running browser reached over the Chrome DevTools Protocol. `ENDPOINT` is an HTTP or WebSocket URL. |
| `safari` | Safari on macOS. Enable "Allow remote automation" in Safari's Developer settings first. |
| `webdriver=URL` | A session on a W3C WebDriver server or cloud grid. |

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Every target completed. |
| `1` | At least one target failed. |
| `2` | The request was invalid, for example a malformed URL or viewport. Nothing was run. |
| `130` | The run was stopped with Ctrl-C, or on Unix with SIGTERM or SIGHUP. Also the exit code of `traceqa serve` and `traceqa mcp` after such a signal. |

Ctrl-C stops the run within about a second, even while a page is loading or a
target waits for a browser that only one target may use at a time. It closes the
browsers and writes a run
folder that says `cancelled`. Targets that had already finished keep their
results. Every target that was in the middle of a page is `cancelled`, also when
the Ctrl-C ended its browser or driver first: none is reported as `failed`. On
Windows, Ctrl-Break does the same as Ctrl-C.

A second Ctrl-C quits at once, without waiting for the browsers: `run.json` is
written first (status `cancelled`, `finishedAt` set, targets that had not
finished `cancelled`), `events.jsonl` ends with `run.finished`, and the exit
code is 130, within about two seconds. The run folder is valid, as after a
single Ctrl-C. The drivers TraceQA started are killed before it exits, with
everything each of them started (a launcher's real driver, the browsers), so a
driver that ignores signals and its input closing cannot outlive the command,
nor can its children. On Windows that is `taskkill /T`; on Unix TraceQA walks
the parent pids from `ps` and sends SIGKILL, which also reaches a process that
left the driver's session or group.

On Unix, SIGTERM (`timeout`, `docker stop`, a CI timeout) and SIGHUP (a closed
terminal) take the same path as Ctrl-C: the first one cancels the run cleanly,
a second one quits at once. The exit code is 130 for all three, because the
program cannot tell them apart. On Windows, closing the console window ends the
process as soon as the handler returns, so that case cannot be cancelled
cleanly: use Ctrl-C or Ctrl-Break.

A first signal whose cleanup gets stuck (a driver that never answers closing its
session) does not wait for ever: after 7 seconds the command quits as if a
second signal had come, closing the run folder and killing the drivers, and
exits 130. With the 2 seconds that closing may take, that is under 9 seconds,
because `docker stop` sends SIGTERM and then SIGKILL after 10 seconds by
default, and a SIGKILL would leave the run folder at `running`.

### Examples

```sh
# Three browsers, desktop and mobile viewports
traceqa extract https://example.com --browser chromium,firefox,webkit --viewport 1280x800,390x844

# The same, with a picture of every component
traceqa extract https://example.com --browser chromium,firefox,webkit --viewport 1280x800,390x844 --crops

# Attach to a running browser
traceqa extract https://example.com --browser cdp=http://127.0.0.1:9222

# Machine-readable result
traceqa extract https://example.com --json > run.json
```

### `--repeat`: find unstable components

```sh
traceqa extract https://example.com --repeat 5 --fail-on-unstable
```

Runs the whole extract N times, one after another. Every run is an ordinary run folder in `--out`,
so `diff`, `findings`, `codegen`, MCP and the SDKs read them as before. Next to them a group
folder `runs/g_<id>/` holds two files: `group.json` (a `RunGroup`: the member runs, oldest first)
and `stability.json` (a `StabilityFile`). For each target folder (browser and viewport) it lists
every component with `runsPresent` and `rate = runsPresent / runsCompleted`. Only targets that
completed count, so a failed browser does not make every component look missing. A component gets
flags:

| Flag | Meaning |
|---|---|
| `unstable` | Found in some runs but not all (0 < rate < 1). |
| `count_varies` | The number of instances differs between the runs that had it. |
| `state_flaps` | `visible` or `enabled` differs between the runs that had it. |
| `name_varies` | More than one name was seen. Only components that mark a changing name can have this: a different name is otherwise a different component id. |

After the usual per-run summaries it prints `Stability over N runs: K unstable components` and the
ten least steady. A rate keeps timing problems visible that a retry would hide: a component that is
there in 3 of 5 runs is a page problem, however green each single run looked.

- `--json` prints the `stability.json` document only (progress, run folders and the group folder go
  to standard error).
- `--fail-on-unstable` exits with 1 when any component carries a flag.
- Ctrl-C stops after the run in flight (the engine cancels it as for a single run), finishes the
  group with the runs there are, and exits 130.
- `--repeat 1` (the default) is the plain command: one run, no group folder.
- `--repeat N --crops` takes the pictures N times, which is the slowest step; a warning says so.

A group folder holds no `run.json`, so tools that list runs (`list_runs` in MCP) do not see it as
a run.

## `traceqa map`

```sh
traceqa map https://shop.example --depth 2
```

Reads a page, follows its same-origin links a few levels deep, and says which components are on
which page and which are on every page (the header and footer). It never clicks or submits: the only
requests are the browser's own page loads (GET), plus one GET of `/robots.txt` per origin.

> **Warning: logout and delete links.** With `--storage-state` the crawl is signed in, and loading
> `/logout` would sign the saved session out (or `/delete-account` would delete something). Paths
> matching `**/logout*`, `**/signout*`, `**/log-out*`, `**/delete*` and `**/unsubscribe*` are
> therefore never followed; they are listed in `site.json` under `skipped`. Add more with
> `--exclude`, and use `--include` to limit the crawl to the parts you want. Map a site you own or
> have permission to read.

| Option | Meaning |
|---|---|
| `--depth N` | Links to follow from the first page (default 1, most 5). 0 reads only the page. |
| `--max-pages N` | Most pages to read (default 50, most 500). |
| `--delay-ms MS` | Pause between pages (default 500). |
| `--max-minutes N` | Stop after this long (default 10); unread pages are listed as skipped. |
| `--include GLOB`, `--exclude GLOB` | Path globs (`*` one folder, `**` across folders), repeatable. |
| `--no-robots` | Ignore robots.txt. `site.json` says so (`robots.respected: false`). |
| `--browser`, `--viewport`, `--storage-state`, `--out`, `--headed` | As for `extract`. |
| `--json` | Print `site.json` only; progress goes to standard error. |

Links come only from components with the role `link` and an `href`, in every frame and including
hidden ones (a mobile menu). The address is resolved against the page's final address, the fragment
is dropped, tracking parameters (`utm_*`, `fbclid`, `gclid` and similar) are removed, and only the
same scheme, host and port are kept. `mailto:`, `tel:`, `javascript:`, `data:` and files such as
PDFs, archives, images and media are skipped. At most 20 addresses are read for one path pattern
(`/products/1`, `/products/2` count as `/products/{n}`), which stops endless calendars and
paginations.

robots.txt follows RFC 9309 for the product token `traceqa`: the longest matching Allow or Disallow
wins, `*` and `$` work, a 4xx answer allows everything, and a 5xx answer or no answer disallows the
whole crawl.

Each page is an ordinary run in `--out` (so `diff`, `findings`, `codegen` and MCP read them). Next to
them `runs/g_<id>/` holds `group.json` (a `RunGroup` of kind `site`) and `site.json` (a `SiteFile`):
`pages` (url, final url, depth, run id, title, component ids, links out), `components` (id to role,
name and page indexes), `shared` (ids present on every page that was read), `skipped` (link and
reason) and the limits and robots status used. A header whose landmark name changes from page to
page gets different component ids and is not listed as shared. Ctrl-C stops after the page in
flight, finishes the group and exits 130.

## `traceqa runs`

What is in the runs folder, and clearing it out. None of these starts a browser.

```sh
traceqa runs list  [--out runs] [--url URL] [--json]
traceqa runs stats [--out runs] [--url URL] [--json]
traceqa runs prune [--out runs] [--keep N] [--older-than AGE] [--url URL] [--dry-run] [--yes]
```

`list` shows the runs newest first (and the groups). `stats` shows, per page address, the number of
runs, their statuses, disk use, and the oldest and newest start. `--url` limits either to one
address, written as it was asked for.

`prune` deletes a run when it is not among the newest `--keep N` runs of its address and/or started
longer ago than `--older-than` (`12h`, `30d`, `2w`). With both it must satisfy both, so the newest
N are always kept. At least one is required. Like `docker system prune` it lists what it will
remove first, but there is no prompt, because a pipeline has no one to answer: without `--yes` it
prints the plan, deletes nothing and exits with 2. `--dry-run` prints the plan and exits with 0.

It is safe by construction. Only direct children of `--out` whose name starts with `r_` or `g_`
and that hold a `run.json` (or `group.json`) naming that same folder are considered; a symlink or
junction is never followed, and anything else in the folder is left alone and not even listed. A
run that is `running` is kept, unless it started over 24 hours ago: then it is reported as stale
and treated like any other. A group folder is deleted when none of its runs is left.

A pipeline that runs `extract` on every push should end with
`traceqa runs prune --keep 20 --older-than 14d --yes`, or the runs folder grows without end.

## `traceqa diff`

What differs between two finished runs: across browsers, across viewports, or from one run to the
next. It only reads run folders and never starts a browser.

```sh
# Yesterday's run against today's (one browser and viewport each)
traceqa diff runs/r_20261013_210455_a1b2 runs/r_20261014_093000_c3d4

# Chromium against Firefox inside one run
traceqa diff runs/r_20261013_210455_a1b2 runs/r_20261013_210455_a1b2 --target-a chromium-1280x800 --target-b firefox-1280x800

# For a pull request or a CI step summary
traceqa diff runs/before runs/after --target chromium-1280x800 --out diff.md
traceqa diff runs/before runs/after --target chromium-1280x800 --format json > diff.json
```

`A` and `B` are anything `traceqa` can open as a run: a run folder, a target folder, a
`components.json`, or the id of a run in `--runs-dir` (default `runs`). A run with several targets
needs one: `--target NAME` picks the folder of that name on both sides, `--target-a` and `--target-b`
pick one each. A run with a single target needs no flag.

| Flag | Meaning |
|---|---|
| `--format markdown\|json\|github\|sarif` | Markdown (the default) is for people. `json` is the `DiffFile` contract (`spec/diff.schema.json`; with `--against` it is a `BaselineReport`). `github` and `sarif` are described under [Baselines and the CI gate](#baselines-and-the-ci-gate). |
| `--against NAME\|DIR` | Compare one run with a saved baseline instead of two runs. |
| `--baseline-dir DIR` | Where `--against NAME` looks (default `.traceqa/baselines`). |
| `--fail-on TOKENS` | Exit 1 when an entry matches: `removed`, `added`, `changed`, `disabled`, `hidden`, `covered`, `paired`. |
| `--out FILE` | Write the report to a file instead of standard output. |
| `--target`, `--target-a`, `--target-b` | Which target folder of a run to compare. |
| `--runs-dir DIR` | Where bare run ids are looked up. |

**What is compared.** Components are joined on their id. A component only in B is `added`, only in A
is `removed`, and on both sides with a difference is `changed`, with the fields that differ:
`instances`, `visible`, `enabled`, `covered`, `checked`, `expanded`, `selected`, `pressed` (all counts
of instances in that state) and `confidence`. Each changed entry also carries flags for scripts:
`became_hidden`, `became_visible`, `became_disabled`, `became_enabled`, `became_covered`,
`became_uncovered`, `count_up` and `count_down`. Positions, sizes and pictures are not compared: they
move with every layout change.

**Role and name changes.** The id is a hash of role, name and path, so a button that becomes a link, or
is renamed, has a new id. It appears as one removed and one added entry that name each other in
`pairedWith`. At the same path with the same name and another role the pair is `role_changed`
(confidence high). At the same path and role with exactly one removed and one added component it is
`name_changed` (confidence low). Two renames at one path are ambiguous and are not paired.

**Mode.** Recorded in the report: `cross-browser` (same run, same viewport, another browser),
`cross-viewport` (same run, same browser, another viewport; components a site hides on purpose are
expected to differ), `run-to-run` (different runs, same browser and viewport), or `custom`. When the
two sides share fewer than half of the smaller side's components, a note says so.

**Markdown.** Names, paths and roles come from the page, so they are shown in code spans, cut to 80
characters, stripped of invisible and direction-changing characters, and a pipe is shown as
U+2223. Each of the Removed, Added and Changed tables shows 50 rows and then `and N more, use
--format json`, which also keeps the report far below GitHub's 1 MiB limit for a step summary.

**Exit codes.** `0` whenever the diff ran, whether or not it found differences. `2` for input that
cannot be compared: a path that does not exist, a run with several targets and no `--target`, or two
files with different `fingerprintVersion`s (their ids mean different things; extract both again with
the same TraceQA). `1` when `--fail-on` matched an entry. An unknown `--fail-on` token is `2`, so a
typo cannot switch the gate off.

The report is the same bytes every time for the same inputs.

## Baselines and the CI gate

A baseline is a saved copy of a run (its `components.json` files and, if the run had them, its
crops) that later runs are compared with. It lives in `.traceqa/baselines/NAME/` by default, is
meant to be committed, and holds no `run.json`, no storage state and no error text. It is changed
only by `traceqa baseline`, never by `extract` or `diff`, and never silently.

```sh
traceqa extract https://staging.example.com --browser chromium,firefox --viewport 1280x800
traceqa baseline save runs/RUN --name main              # refuses when "main" exists
traceqa diff runs/NEW --against main --fail-on removed,disabled
traceqa baseline promote runs/NEW --name main           # shows the diff and exits 2
traceqa baseline promote runs/NEW --name main --yes     # now it replaces
```

| Command | What it does |
|---|---|
| `baseline save RUN [--name NAME] [--dir DIR]` | Makes a baseline. The default name is the page's host and 8 hex characters of the SHA-256 of its (redacted) address. Fails when it exists. |
| `baseline promote RUN [--name NAME] [--dir DIR] --yes` | Replaces the baseline with the run and records it in `promotions`. Without `--yes` it prints the difference and exits 2: there is no prompt, so CI cannot move the reference by accident. Aliases whose old component is gone are dropped. |
| `baseline alias FROM TO --name NAME [--evidence TEXT]` | Says that baseline component `FROM` is run component `TO` (a rename). |
| `baseline init RUN --out manifest.yaml [--viewport-name 1280x800=desktop]...` | Writes a manifest in the format of `fixtures/buggy-shop/manifest.yaml` (`spec/manifest.schema.json`) from what the run found: role, name or `template: true`, `instances`, `confidence`, `frame`, `shadow`, `hidden` (viewports where no instance is visible) and `disabled`, plus the warning codes seen. It records what was found, not what should be there, so review it. Refuses to overwrite without `--force`. |

`diff --against` compares every target of the baseline with the run's target of the same folder
name (`chromium-1280x800`), with the baseline as side A. A target the run lacks is reported as an
error entry and the exit code is 2. Ids of another `fingerprintVersion` cannot be compared: the
command exits 2 and tells you to re-save with `baseline promote --yes`.

**The gate.** `--fail-on` takes a comma-separated list. An entry matches `removed` or `added` by
its kind, `changed` when it is on both sides and differs, `disabled`, `hidden` and `covered` by the
flags `became_disabled`, `became_hidden` and `became_covered`, and `paired` when a removed or added
entry names the other side's by heuristic (a changed role or name). The exit code is 1 when any entry
matches. A rename that an alias covers (`pairedWith.reason` is `aliased`) is shown as renamed and
matches nothing.

**Formats.**

- `github` prints one workflow command per entry, at most 50 lines, most serious first
  (`::error title=TraceQA removed::button "Add to cart" (c_...) is in the baseline but not in the
  run`). Removed, disabled, hidden and covered are errors; added and changed are warnings; renamed is
  a notice. Page text is cleaned and escaped as the Actions toolkit does, so a name with a newline
  cannot start another command. When `$GITHUB_STEP_SUMMARY` is set the Markdown report is appended
  to it, which has no 50 line limit. `traceqa findings --format github` does the same for findings.
- `sarif` writes SARIF 2.1.0 (`traceqa.diff.removed` and the like as stable rule ids, one result per
  entry, `baselineState` new or absent, a versioned `partialFingerprints` entry). A web component has
  no source file, so a result's location is the page address and a logical location naming the
  component. GitHub code scanning wants repo-relative source files with regions, so this output is for
  SARIF viewers and other tools; use `--format github` for GitHub. Mapping components to source files
  is on the backlog. `traceqa findings --format sarif` is the same for findings.

A workflow (every action is pinned to a full commit SHA; install
TraceQA in the way that suits you before the first `traceqa` step):

```yaml
name: components
on: pull_request
permissions:
  contents: read
jobs:
  gate:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4.4.0
        with:
          persist-credentials: false
      - run: traceqa extract "$STAGING_URL" --browser chromium --viewport 1280x800
        env:
          STAGING_URL: ${{ vars.STAGING_URL }}
      - run: traceqa diff "$(ls -d runs/* | tail -n 1)" --against main --fail-on removed,disabled,hidden --format github
```

When the change is meant, run `traceqa baseline promote RUN --name main --yes` locally and commit
the baseline folder, so the new reference is reviewed like code.

## `traceqa visual`

Compares the component crops of two runs pixel by pixel, to catch a style change that the structural
diff cannot see (a colour, a font, a padding).

```
traceqa extract URL --crops --out runs/before
traceqa extract URL --crops --out runs/after
traceqa visual runs/before/RUN runs/after/RUN --target chromium-1280x800
traceqa visual runs/RUN --against main        # the baseline must have been saved from a run with crops
```

Options: `--target NAME` (the target folder, needed for a run with several), `--threshold 0.2` (how far
two pixels' colours may differ, 0 to 1, as in Playwright's `toHaveScreenshot`), `--max-diff-ratio 0.001`
(a component is the same when at most this share of its pixels differ), `--out DIR` (default
`./visual-diff`; the pictures go to `DIR/TARGET/ID.diff.png`, a red mask over a faded copy of side B,
only for components that differ), `--format markdown|json` (json is the `VisualReport` of
[spec/visual.schema.json](../spec/visual.schema.json)), `--fail-on-different`, `--baseline-dir`, `--runs-dir`.
It never writes into either run folder.

> **Environment warning.** Pixels are compared only within one browser and one viewport. Another
> browser name or viewport exits 2 (use `traceqa diff` across browsers). A different browser
> *version* is recorded in the report; when the major versions differ, `environmentMatch` is false and
> every compared result has `unreliable: true`, because a new browser release changes text rendering.
> Run both sides on the same machine image (as Playwright advises for its own screenshots), and review
> reference updates like code. A different `devicePixelRatio` gives crops of another size, which are
> reported as `size_changed`, not compared.

Only components the structural diff found unchanged are compared. Everything else is listed with a
reason in `results[].reason`: `added`, `removed`, `structurally_changed` (with the fields), `covered`,
`not_visible`, `no_crop` (it never appeared on screen while scrolling), `name_changes` and
`name_is_template` (a name that moves cannot be masked, because the collector does not record where the
text is, so the whole component is skipped), `unsafe_crop` and `crop_unreadable`. A component whose box
differs by more than 1 px, or whose crops differ in size, is `size_changed`. A crop is cut from the screen
that showed most of the component while the page scrolled, so sticky headers and animations can pollute
it; that is why covered components are skipped. Anti-aliased pixels are ignored (Vysniauskas' detector,
as pixelmatch does). Exit codes: 0 when the comparison ran, 1 with `--fail-on-different` and a different
component, 2 for input that cannot be compared (another browser or viewport, a side without crops).

## `traceqa verify`

```sh
traceqa verify RUN --manifest FILE [--target NAME] [--strict] [--format text|json|github|sarif]
```

Checks a finished run against a manifest: the list of components a page should have, in the format
of `fixtures/buggy-shop/manifest.yaml` and `spec/manifest.schema.json`. `traceqa baseline init RUN
--out manifest.yaml` writes a first one from a run; review it, because it records what was found,
not what should be there. A manifest is a constraint set, not a snapshot: it says what must exist,
what must not, and which warnings are allowed. The check is `bench/run.py`'s, so the two agree on
the shop.

| Option | Default | Meaning |
|---|---|---|
| `--manifest FILE` | required | The manifest (YAML). |
| `--target NAME` | all | Only this target, for example `chromium-1280x800`. |
| `--strict` | off | Fail on extras: components found that the manifest does not list. Without it they only warn. |
| `--format` | `text` | `text`, `json` (`VerifyReport` in `spec/manifest.schema.json`), `github` (workflow commands, and a summary in `$GITHUB_STEP_SUMMARY`) or `sarif` (SARIF 2.1.0, for viewers; not GitHub code scanning, see Findings). |
| `--runs-dir DIR` | `runs` | Where a bare run id is looked up. |

Exit codes: 0 every checked target passes, 1 a target fails, 2 the manifest or the run cannot be
used (a typo in the manifest, a repeated viewport name, an unknown `--target`, a target that did
not finish, or no target at a viewport the manifest names).

Each target is checked at the manifest viewport with the same width and height; a target at another
size is skipped with a note. Per target:

| Check | Fails when |
|---|---|
| `missing` | a listed component is not found. The message names the nearest component of the same role, by edit distance. |
| `present` | an `absent` entry (role and exact name) is found. |
| `instances`, `confidence`, `disabled` | the count, the confidence or the number of disabled instances differs (defaults 1, `high`, 0). |
| `hidden`, `visible` | a component listed as hidden at this viewport has a visible instance, or one not listed has none. |
| `frame`, `shadow` | the frame it lives in, or whether it is in shadow DOM, differs. A frame name is compared the way the path writes it (trimmed, lower-cased), never by splitting the path on `>`. |
| `warning` | the extraction raised a warning code the manifest's `warnings` does not list. A listed code that was not raised is only a note, so stale entries get reported. |
| `extra` | (with `--strict`) a component is found that no entry matched. |

Matching follows the bench: the same role, then `template: true` (any name) or the exact name, each
found component used once, in manifest order. Ids are not used, because people write manifests.
Unknown keys anywhere in the manifest are an error, so a typo such as `instnaces` cannot silently
weaken the check; so is a `hidden` list that names a viewport not under `viewports`. Page text in a
message is cleaned and cut like in the other reports.

```text
$ traceqa verify runs/r_20261013_210455_a1b2 --manifest fixtures/buggy-shop/manifest.yaml
chromium-1280x800 (desktop): FAIL, 37 components checked, 1 failures
  FAIL missing at desktop: button 'Open the size guide' x1 (nearest: button 'Open size guide' c_4f1a...)
```

## `traceqa serve`

```
traceqa serve [--max-runs N] [--driver COMMAND]...
```

Speaks the client protocol on standard input and output. SDKs start the engine
this way. See [architecture.md](architecture.md#client-protocol).

Limits: a request line longer than 8 MiB is skipped and answered with a
JSON-RPC error (code `-32600`, id `null`); the next line is read as usual. At
most `--max-runs` runs (default 4) go at once; a `run.start` beyond that gets an
error with kind `busy` and `retryable: true`, and can be sent again when a run
has ended (as soon as its `run.done` arrives). A `run.start` whose viewport has a
side outside 1 to 10000 is answered at once with `-32602` (kind `invalid_params`);
no browser is started and no run folder is made. The command line, the MCP
`extract` tool and both SDKs refuse the same sizes the same way.

Closing: a client that means to close sends `engine.shutdown` (answered with
`{}`), then ends the engine's input. From then on `run.start` is refused with
kind `busy`, runs still going finish, and the engine exits when its input has
ended. If the input ends **without** `engine.shutdown`, the client is taken to
have crashed: every run is cancelled the way `run.cancel` cancels it, each run
folder is finalised with status `cancelled`, and the engine exits. No browser
is left running for a client that is gone. The SDKs send `engine.shutdown` in
`close()`.

Methods: `engine.hello` lists `features` (`extract`, `diff`, `findings`). Two methods
read local files and answer when done, without a browser or a run:
`diff.compare` (`a`, `b` run folders, optionally `targetA`, `targetB`; answers with a
`DiffFile`) and `findings.analyse` (`run`, optionally `target` and `suppressions`, the
path of a suppression file; answers with `{files: [FindingsFile]}`). A request that
names a folder without `run.json`, a target the run does not have, a missing or invalid
suppression file, or a `components.json` over 64 MiB is answered with `-32602` (kind
`invalid_params`) and a message that says what to fix. The SDKs offer them as
`Engine.diff` / `Engine.findings` (Python) and `engine.diff` / `engine.findings`
(TypeScript).

Signals: `serve` stops the way `extract` does. Ctrl-C (Ctrl-Break on Windows),
SIGTERM or SIGHUP cancels every run through the same path as `run.cancel`, stops
accepting `run.start` (refused with kind `busy`, "shutting down"), lets each run
write its last events and `run.done`, and exits **130** once none is going; an
idle engine exits 130 at once. A second signal quits at once: the run folders
are closed as `cancelled`, the drivers and everything they started are killed,
and the engine exits 130 within about two seconds. A first signal whose runs
cannot stop does the same by itself after 7 seconds, so a `docker stop` (SIGKILL
after 10 seconds) still finds closed folders. Standard output carries protocol
lines only; the messages go to standard error. A client that sees the engine
exit 130 without having sent `engine.shutdown` knows it was signalled.

## `traceqa mcp`

```
traceqa mcp [--out DIR] [--state-dir DIR] [--suppressions FILE] [--allow-remote-browsers]
            [--block-private] [--allow-host HOST]... [--max-concurrent-extracts N]
            [--driver COMMAND]...
```

An MCP server: it lets AI tools such as Claude Code, Claude Desktop and Cursor
run TraceQA. The AI tool starts it and talks to it over standard input and
output. Runs are written to and read from `--out` (default `runs`, relative to
the folder the AI tool starts it in).

Add it to Claude Code:

```sh
claude mcp add traceqa -- traceqa mcp
```

Other AI tools take the same command in their MCP settings: the program
`traceqa` with the argument `mcp`.

Signals: Ctrl-C (Ctrl-Break on Windows), SIGTERM or SIGHUP, as sent by `docker
stop`, a closed terminal or a supervisor restart, cancels every running call
(a cancelled call is not answered, as with `notifications/cancelled`), refuses
new `extract` calls ("shutting down"), lets each run folder end `cancelled`
with `run.finished` written, and exits **130**. A second signal, or 7 seconds
without the calls ending, closes the folders at once, kills the drivers and
exits 130 as well. See the signal paragraphs under [Exit codes](#exit-codes).

| Tool | What it does |
|---|---|
| `extract` | Runs an extraction. Arguments: `url`, and optionally `browsers`, `viewports` (`WIDTHxHEIGHT`), `crops`, `settle_ms`, `max_scroll_steps`, `timeout_ms`, `include_hidden`, `storage_state` (the name of a state file in the operator's `--state-dir`; its contents are never shown). Answers with the run id and a summary per target. |
| `list_runs` | The runs in the runs folder, newest first. It does not say where the folder is. |
| `describe_page` | Orient on a page: the components of one run and target grouped by landmark (`banner`, `navigation`, `main`, forms, dialogs, frames), a few hundred tokens for a whole page (the demo shop is under 4 KB). Repeated cards appear once with the name `*` and up to three `example_names`. Arguments: `run_id`, `target`, `max_per_group` (default 25), `response_format`. |
| `find` | Search by `role`, `name`, `query` (a substring of the name, `href`, `placeholder` or `data-testid`; the page's visible text is not stored), `landmark` or `state` (`visible`, `hidden`, `enabled`, `disabled`, `covered`, `checked`, `unchecked`, `expanded`, `collapsed`, `required`) in every target of a run at once (at most 12 targets per call, or one with `target`). Each result has `found_in` (targets and instance counts) and `missing_in`, so one call answers whether a control is in every browser. Paged with `limit` and `offset`. |
| `get_components` | The components of one run and target, one short row each, filtered by `role`, `name` or `landmark` and paged with `limit` and `offset`; or one component in full with `component_id`. |
| `diff_runs` | What changed between two runs, or between two targets of one run: the components only on one side or that differ, one short row each `{change, id, role, name, fields, flags}` (`change` is `added`, `removed` or `changed`; `added` means in `run_b` only; `flags` such as `became_hidden`). `detailed` adds `before` and `after` summaries and `pairedWith`. Arguments: `run_a`, `run_b` (run ids, never paths), `target_a`, `target_b` (needed for a run with several targets), `limit`, `offset`, `response_format`. Computed on demand, as `traceqa diff` does. |
| `findings` | Automated checks over a run, computed on demand as `traceqa findings` does (nothing is written): rows `{id, rule, severity, componentId, name, message, target}`, with counts by severity. `detailed` adds `confidence`, `role`, `path`, `instances`, `evidence`, `wcag`. Arguments: `run_id`, `target`, `min_severity`, `rules`, `limit`, `offset`, `response_format`. Findings are suppressed only by the file the operator names with `--suppressions`; the AI cannot name a file. |
| `get_crop` | The picture of one component, for runs made with `crops`. |

Which tool to call: `extract` to create a run, `describe_page` to orient,
`find` to search across browsers, `get_components` to page through a target or
fetch one id, `diff_runs` to compare two runs or two targets, `findings` for the
automated checks, `get_crop` to see a component.

**Locator hints.** Every `get_components` row has `locator`, the Playwright
(TypeScript) expression that finds the component, for example
`page.getByRole("button", { name: "Add to cart", exact: true })`, as `traceqa
codegen` writes it. `detailed` rows carry the whole locator and `locator_unique`
(whether it matches this component alone on the page). `concise` rows carry the
locator cut at 120 characters, and `locator_unique: false` only when it is not
unique, so an agent does not copy an ambiguous locator without knowing. A
component from an older run has no `locator`. The text comes from the page and
is escaped as a string literal; like every string in an answer it is data.

**Response formats.** `get_components`, `find` and `describe_page` take
`response_format`. `concise` (the default) gives each component as `id`, `role`,
`name` and `instances`, plus `hidden: true` or `template: true` when they apply.
`detailed` adds `visible`, `path`, `selector`, `confidence` and `has_crop`, the
fields `get_components` returned before concise became the default. A concise
answer is about half the size of the detailed one for the same limit (6.0 KB
against 11.6 KB for the 37 components of the demo shop, with the short locator
in every concise row). Answers are compact JSON.
When a list is cut by `limit`, the answer says how to narrow it and carries
`next_offset`: "Showing 100 of 240. Narrow with role (for example button), name
(substring) or landmark, or continue with offset=100."

**Structured content and resources.** With MCP 2025-06-18 or later (the version
the host negotiates in `initialize`), every tool declares an `outputSchema` and
answers with the same JSON as `structuredContent`, cleaned like the text; the
text blocks are unchanged, with the notice first and the JSON last. Older hosts
get neither, and no `resource_link` blocks. `extract` and `get_crop` add
`resource_link` blocks that point at resources, and `get_crop` still returns the
picture itself. The server offers resources (read-only, no subscriptions and no
list-changed notices, `audience: ["assistant"]`):

| URI | Content |
|---|---|
| `traceqa://runs/{run_id}` | The run's manifest as JSON. `resources/list` shows the newest 50 runs. |
| `traceqa://runs/{run_id}/{target}/components` | The target's components file as JSON, cleaned, flagged and capped like a tool answer. |
| `traceqa://runs/{run_id}/{target}/crops/{component_id}` | The component's picture as a base64 `blob` (`image/png`), up to 2 MiB. |

Every segment must be a plain name, files are read only when they sit inside the
runs folder (never through a link), and a run, target or picture that does not
exist is answered with the JSON-RPC error `-32002`.

A mistake in the arguments is answered as a tool error that says what was
wrong, so the AI can correct itself. Progress is reported while an extraction
runs, and the AI tool can cancel it.

**Pages behind a sign-in (operator guide).** The AI cannot make the server open
any file. To let it read pages behind a sign-in, you choose a folder of saved
states with `--state-dir`; without that option `storage_state` is off and is
not even listed in the tool's schema.

```sh
mkdir -p ~/.traceqa/states
traceqa login https://shop.example --save ~/.traceqa/states/shop.state.json
claude mcp add traceqa -- traceqa mcp --state-dir ~/.traceqa/states
```

The AI then passes `storage_state: "shop.state.json"`, a file name and never a
path. A name may hold letters, digits, `.`, `_` and `-`, must start with a
letter or digit, may not end in a dot, may not be a Windows device name (`CON`,
`NUL`, `COM1` and so on), and must be a regular file inside the folder (links
and junctions are refused). `--state-dir` must be an existing folder, or the
server exits with code 2. Errors never show a path, an operating system error
or what the file holds.

**Where the AI may send the browser (operator guide).** What the AI asks for
is limited by options only you, the person who starts the server, can set:

| Option | Effect |
|---|---|
| (default) | `cdp=ENDPOINT` and `webdriver=URL` browsers are refused and not even named in the tool's schema. Cloud metadata and link-local addresses are refused. Localhost and private networks work, so local dev servers can be tested. |
| `--allow-remote-browsers` | The AI may attach to a running browser (`cdp=`) or a WebDriver server (`webdriver=`). They name a machine for this server to connect to, so turn this on only for an AI you trust with that. |
| `--block-private` | Also refuse loopback, private (`10/8`, `172.16/12`, `192.168/16`), shared (`100.64/10`), unique-local (`fc00::/7`) and multicast addresses. |
| `--allow-host HOST` | With `--block-private`, still allow this host (name or address). Repeat for several. |
| `--suppressions FILE` | A suppression file (YAML, see `traceqa findings`) that the `findings` tool honours. Checked at start (exit 2 when it is missing or invalid) and read again on each call, so an edit takes effect at once. Without it nothing is suppressed. The AI cannot name a file; its path is never shown. |

The address is read the way a browser reads it, so `http://2852039166/`,
`http://0xA9FEA9FE/` and `http://[::ffff:a9fe:a9fe]/` are all seen as
`169.254.169.254`. A name is looked up and every address it has is checked. The
following are refused with or without `--block-private`, and `--allow-host`
does not lift them: `169.254.0.0/16` (link-local, which holds the AWS, Azure
and Google metadata service), `100.100.100.200`, `168.63.129.16`,
`192.0.0.192`, `fd00:ec2::254`, `0.0.0.0/8`, `::`, `fe80::/10`, and the names
`metadata.google.internal` and `metadata.goog`. A refused address is answered at
once, before anything is requested.

**Limits on load.** A request line longer than 8 MiB is skipped and answered
with an error (code `-32600`, id `null`); the server then reads the next line.
At most `--max-concurrent-extracts` extractions run at once (default 2). A call
beyond that is answered at once, without starting a browser, with a tool error:
`busy: N extractions are already running. Wait for one to finish, then try
again.` The reading tools (`list_runs`, `get_components`, `describe_page`, `find`, `diff_runs`, `findings`, `get_crop`) have a
separate limit of 16 calls at once, so reading earlier results is never held up
by a long extraction. A slot is free again before the answer to the call that
used it is sent. Cancelling a call works at any time.

What the address rules do not cover, so you can judge the risk:

- **Redirects are checked after the fact (Playwright browsers).** A browser
  shows its hops only once it has followed them, so the request to a refused
  host has already been made when the check fails. The result is then thrown
  away: the target fails with `blocked_url` and no components are written. Each
  hop is checked by its address and by the IP address the browser connected to
  (which defeats a name that answers differently the second time), where the
  browser reports it. WebKit reports no connection address for a hop that
  redirected; its name is looked up instead.
- **WebDriver browsers** report only the final address, so only that is checked.
- **Subresources and frames** (images, scripts, `fetch`, iframes) are not
  checked. A page you let the browser load can make the browser request other
  hosts. Run the server where that is harmless (no access to metadata or internal
  services) if the AI should not be able to reach them.
- A name that resolves to different addresses for the check and for the browser
  (DNS rebinding) can pass the first check. The connection-address check is the
  safety net.

**Page text is data, not instructions.** Names, text, selectors, titles,
warnings and urls in a result come from a web page, and a page can try to
talk to the AI. So every result built from run data starts with a text block
saying this, and its JSON carries `"untrusted_page_content": true`. Read the
JSON from the last text block. Before the AI sees them, control characters
(a newline and a tab stay), zero-width and bidi format characters and the
invisible Unicode Tags block are removed from every string and key. This also
drops the joiners inside some emoji and Indic or Arabic text. Hidden and
off-screen elements, where injected text hides, are left out unless
`extract` gets `include_hidden: true`.

Answers are kept small: `limit` is at most 500 (the answer says
`limit_applied`), one component lookup shows at most 50 instances
(`instances_total`, `instances_shown`), no answer is longer than 100 KB (when
cut it says `truncated: true` with a note that names how to narrow it), `get_crop` refuses pictures over
2 MiB (and only pictures that really sit inside the runs folder, never
through a link), and error texts are cut to 500 characters. No answer, error or
progress message shows a local path: the runs folder is never named, and other
absolute paths in engine errors are replaced by `<path>`. The tools carry MCP
annotations: `extract` opens pages (open world, not read-only); the other
seven are read-only.

## `traceqa install`

```
traceqa install [BROWSERS]... [--from FOLDER] [--ref REF] [--with-deps] [--dry-run]
traceqa install --axe [--dry-run]
```

Installs the browser drivers and browsers, so the binary is all a user needs
to download. Needs [uv](https://docs.astral.sh/uv/); without it, the command
says how to install it (with `winget`, `brew` or `pipx`, never a script piped
into a shell; uv's installation page also has a versioned installer with a
published checksum).

| Step | What happens |
|---|---|
| 1 | Creates a Python environment in `~/.traceqa/env` (or `$TRACEQA_HOME/env`), away from the user's other Python projects. |
| 2 | Installs the drivers' third-party dependencies from `~/.traceqa/driver-requirements.txt`, which is built into the binary and pins every package to the version and sha256 hash in `uv.lock` (`uv pip install --require-hashes --no-deps`). |
| 3 | Installs TraceQA's own three packages from GitHub without dependencies, at the commit the binary was built from, or from a checkout with `--from`. From a checkout the versions come from its `uv.lock` and step 2 does not apply. |
| 4 | Downloads the browsers asked for. Default `chromium`. Known values: `chromium`, `chrome`, `msedge`, `firefox`, `webkit`, `safari` (nothing to download). |

| Option | Meaning |
|---|---|
| `--from FOLDER` | Install the drivers from a TraceQA checkout instead of GitHub. |
| `--ref REF` | Branch, tag or commit on GitHub. Default: the commit the binary was released from (shown by `traceqa doctor`), so the binary and its drivers match and cannot be changed afterwards. Only a full 40-digit commit id is pinned: any other value is used but reported as not pinned. |
| `--with-deps` | Also install the system libraries the browsers need. Linux only, may ask for `sudo`. |
| `--dry-run` | Print the commands that would run, and write nothing. |
| `--axe` | Install only axe-core, for `extract --axe`; drivers and browsers are left alone. See [Accessibility checks with axe-core](#accessibility-checks-with-axe-core). |

A binary that was not built from a release commit (a debug build, a tree with
uncommitted changes, a build without git) has no commit to install from. It
uses its release tag, such as `v0.1.0`, and warns that tags can be moved.
Release builds get the commit from `TRACEQA_RELEASE_COMMIT`, or from `git` when
the tree is clean.

What the pinning does not cover: the build tool of the three TraceQA packages
(`hatchling`) is fetched without a hash when they are built, and Playwright
downloads the browsers from its own servers; neither is pinned by TraceQA.

Running it again updates the drivers. The engine finds the installed drivers
by itself; see [Drivers](#drivers).

The requirements file is made from `uv.lock` by
`scripts/gen_driver_requirements.py`; a test fails when they differ.

## `traceqa doctor`

Checks the setup and says what is missing: the commit the binary was built from
(or `not a release build`), TraceQA's folder, uv, and for each
driver where it was found, whether it starts, and, for each browser it lists,
whether that browser really opens (doctor starts a headless window and closes
it, one to three seconds per browser). A browser that is not installed is named,
with the command that installs it. `cdp` and `webdriver` need an address and
are not tried; Safari is tried on macOS only. Honours `PLAYWRIGHT_BROWSERS_PATH`.
Exits with 0 when at least one browser opens, and with 1 when none does.

The drivers' own log lines (`[driver info] opened chromium ...`) are hidden, so they do
not interleave with the results. `--verbose` shows them, on standard error. The
results and the exit code are the same either way.

```
TraceQA 0.1.0
Commit:  not a release build
Home:    /Users/me/.traceqa (drivers installed here)
uv:      /opt/homebrew/bin/uv

Drivers:
  /Users/me/.traceqa/env/bin/traceqa-driver-playwright
    installed: /Users/me/.traceqa/env/bin/traceqa-driver-playwright
    works: traceqa-driver-playwright 0.1.0, opens chromium, chrome, msedge, firefox, webkit, cdp
    chromium: ok
    chrome: ok
    msedge: not installed -> msedge is not installed. Run: traceqa install msedge
    firefox: ok
    webkit: ok
  ...

Ready. Browsers that work: chrome, chromium, firefox, webkit
Not installed: msedge. Run: traceqa install msedge
```

## `traceqa findings`

```
traceqa findings [OPTIONS] <RUN>
```

Automated checks over a finished run: controls with no name, buttons that are not
buttons, targets too small to hit, and a few more. It reads each target's
`components.json`, prints the findings, and writes `findings.json` next to it.
`extract` does not write `findings.json`, so a run folder is the same with or
without this command.

**These are automated checks, not a conformance claim.** Automated tools find
only part of the accessibility problems a page has (Deque puts it at about 57%
of the issues in an audit). A page with no findings is not accessible, and each
finding says how sure its rule is.

| Option | Default | Meaning |
|---|---|---|
| `<RUN>` | | A run folder made by `traceqa extract`. |
| `--target NAME` | all | Only this target, for example `firefox-1280x800`. |
| `--suppressions FILE` | `.traceqa/suppressions.yaml` if it exists | Findings the team has decided to live with. See [Suppressions](#suppressions). |
| `--min-severity LEVEL` | `info` | Leave out findings below `info`, `warning` or `error`. |
| `--fail-on LEVEL` | `never` | Exit with 1 when a finding of this severity or worse is left: `error`, `warning`, `info` or `never`. |
| `--format FORMAT` | `table` | `table`, `json` (an array of `FindingsFile`, one per target), `markdown`, `github` (workflow commands, and the Markdown appended to `$GITHUB_STEP_SUMMARY`) or `sarif` (SARIF 2.1.0 for SARIF viewers; see [Baselines and the CI gate](#baselines-and-the-ci-gate)). |
| `--no-write` | off | Only print. |

Exit codes: 0 when nothing is left at or above `--fail-on`, 1 when something is,
2 when the command is wrong: not a run folder, an unknown `--target`, a suppression
file that cannot be used. The findings are computed from `components.json` every
time. Running the command twice writes byte-identical files (an unchanged file is
not rewritten); there are no timestamps in them.

A finding's `id` is the rule, a colon and the component id, such as
`unnamed-control:c_9f2a41c07b3e`. It is the same in every browser and run for as
long as the component id is. A finding names one component and lists up to 20 of
its instances. Messages quote page text, with control and bidirectional
characters removed.

### Rules

| Rule | Severity | Confidence | WCAG | Evidence | How to fix |
|---|---|---|---|---|---|
| `unnamed-control` | `error` (`warning` for a component the engine is not sure is interactive) | `high` (`low`) | 4.1.2 | `unnamedInstances`, `tag`, `signals` | Give it text, an `aria-label` or a `<label>`. |
| `weak-name-source` | `warning` | `medium` | 4.1.2, 3.3.2 | `nameSource` (`placeholder` or `title`), `weakInstances` | Add a visible `<label>`. A placeholder disappears when typing; a title does not show on touch screens. |
| `fake-button` | `error` when it cannot be reached with the keyboard, `warning` when it can be focused but has no role | `low` | 4.1.2, 2.1.1 | `signals`, `keyboardReachable`, `tabindex` (the attribute, when present) | Use a `<button>`. If you must keep the `<div>`, add `role="button"`, `tabindex="0"` and key handlers. |
| `small-target` | `warning` | `medium`; `low` for links and native checkboxes and radios | 2.5.8 | `size` (`w`, `h`), `minSize` (24), `spacingOk` (false), `undersizedInstances` | Make the target at least 24x24 CSS pixels, or space it so a 24 px circle on it touches no other target. |
| `covered-at-load` | `warning` | `medium` | none | `coveredBy` (selector), `coveredInstances` | Check that the cover is meant to be there (a dialog, a banner). If not, fix the stacking. |
| `ambiguous-names` | `info` | `medium` | 2.4.4 | `name`, `destinationCount`, `destinations` (up to 10), `components` (up to 10) | Make the link text say where it goes, or add an `aria-label` that does. |
| `missing-autocomplete` | `info` | `low` | 1.3.5 | `inputType`, `instances` | Add `autocomplete` (`current-password`, `email`, `tel`, ...). |
| `browsers-disagree` (run scope) | `warning` when the component is missing in some browsers, or visible or enabled in some and not others; `info` when only the instance count differs | `high` (missing), `medium` | none | `viewport`, `differs` (`presence`, `visible`, `enabled`, `instances`), `browsers` (per browser: `present`, `instances`, `visible`, `enabled`), `presentIn`, `missingIn`, `similar` | Check which browser is right. A page read before it settled does this too: try `--settle-ms`. |

Only visible instances are checked, and only these rules are shipped: a rule that
is often wrong costs more trust than it earns.

- `unnamed-control` checks components with an interactive role.
- `weak-name-source` needs `nameSource` on each instance. A `components.json` from
  an older engine has none; the file then has the warning `name_source_missing`
  and the rule does not run. A label with the same text as the placeholder is not
  reported, because the name then comes from the label.
- `fake-button` looks at elements the engine found only by a pointer cursor or an
  `onclick` attribute, with no native control and no interactive role. A focusable
  element with neither (a `tabindex` alone) is not reported. `tabindex="-1"` is
  not keyboard reachable.
- `small-target` follows WCAG 2.5.8 for what a page can show. A target under 24 px
  on a side passes the *spacing* exception when a 24 px circle centred on it
  touches no other target and no circle of another undersized target. A box that
  contains the target (its container) is not a neighbour. Disabled controls are
  skipped. The *inline*, *user-agent control*, *equivalent* and *essential*
  exceptions cannot be seen in the data, so links and native checkboxes and radios
  get confidence `low`.
- `covered-at-load` uses the state the collector recorded: visible, on screen and
  covered at every point tested.
- `ambiguous-names` looks at links whose name is not templated. Links with the same
  name (compared lower-case, with spaces collapsed) and different `href` values are
  ambiguous. Links without an `href` count by component. The finding is attached to
  the lowest component id of the group. Two links with the same name and the same
  destination are fine.
- `missing-autocomplete` checks `<input>` of type `password`, `email` and `tel`.
- `browsers-disagree` needs two or more browsers at one viewport and is the only rule
  that looks across targets. Its findings are written to the run-level
  `findings.json` (`scope: "run"`), with the id `browsers-disagree:<componentId>@<width>x<height>`
  (the size is part of the id because a component can disagree at one size and not
  at another). No browser is the reference: the evidence lists every browser's values.
  It compares the same component id (see `traceqa diff`) by presence, by whether
  none, some or all instances are visible or enabled, and by the number of instances.
  A control whose role differs between browsers has a different id in each, so it
  shows as missing in one; `similar` then names the component of that browser at the
  same path that looks like it (`role_changed` or `name_changed`).

### axe-core rules

A run made with `traceqa extract --axe` also has `axe.json` in each target folder, and `traceqa findings` then adds one finding per axe rule and component. See [Accessibility checks with axe-core](#accessibility-checks-with-axe-core) below for how it works and what it cannot cover.

### Suppressions

```yaml
# .traceqa/suppressions.yaml
suppressions:
  - id: small-target:c_9f2a41c07b3e        # or: rule + component
    reason: Inline link inside a sentence (the 2.5.8 inline exception).
  - rule: fake-button
    component: c_3b33554f14cd
    reason: Legacy tile, replaced in Q1.
    expires: 2027-03-31                     # optional, YYYY-MM-DD, applies through that day
```

`reason` is required. Unknown keys are refused, and so is a file over 256 KiB.
A suppressed finding moves from `findings` to `suppressed` in `findings.json`,
carrying its `reason`, and does not count towards `--fail-on`. An entry past its
`expires` day is ignored and the finding comes back, with a `suppression_expired`
warning. An entry that matches no finding in any target gets a
`suppression_unused` warning but does not fail the run, so the file does not rot.
When a run has such warnings they are written to a run-level `findings.json` in the
run folder itself (`scope: "run"`).

## Accessibility checks with axe-core

```sh
traceqa install --axe
traceqa extract https://example.com --axe
traceqa findings runs/<runId>
```

[axe-core](https://github.com/dequelabs/axe-core) is Deque's open source accessibility rule engine. `--axe` runs it on every page
(and in every frame the driver can reach) after the components have been found, and places each failing element on a component, so the
result has the same ids as everything else in the run. It adds one file per target, `axe.json`; nothing changes in `components.json`
except a warning when axe could not run. A run without `--axe` never reads an axe file.

**What it is not.** Automated rules find on average about 57% of the WCAG issues in an audit (Deque). A page with no axe findings is not
accessible, and nothing here is a conformance claim. Contrast over images and gradients, the meaning of text and alt text, keyboard
order, focus visibility, time limits, caption quality and everything inside menus and dialogs that are closed when the page loads need a
person. axe also leaves results it cannot decide as "incomplete" (needs review): `findings.json` counts them in `needsReview` and
carries this note in `coverage`. Rules run: the tags `wcag2a`, `wcag2aa`, `wcag21a`, `wcag21aa`, `wcag22aa` and `best-practice`.

**Where axe comes from.** axe-core is licensed under the Mozilla Public License 2.0 (a file-level copyleft licence). TraceQA does not
include it in the repository, the binary or the release archives. `traceqa install --axe` downloads the pinned npm tarball
(`axe-core` 4.14.0, from `registry.npmjs.org`) at your request, checks its SHA-512 against the value built into `traceqa`, and writes
only `axe.min.js`, `LICENSE` and, when present, `LICENSE-3RD-PARTY.txt` to `~/.traceqa/axe/4.14.0/` (or `$TRACEQA_HOME/axe/4.14.0/`),
unmodified. It prints the licence, the source location and that the file is unmodified. A download that does not match the hash is
rejected and nothing is written. `--dry-run` prints the address and the hash and writes nothing. `TRACEQA_AXE_TARBALL_URL` moves the
download to a mirror; the hash is still checked. See [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md).

`--axe` without an installed file (and without `--axe-file PATH` or `TRACEQA_AXE_FILE`) exits with code 2 and the install hint, before
any run folder exists. The SDKs and `traceqa serve` take `options.axe: true` and the same rule applies. The MCP server has no axe
option in this version: it would let a model trigger a large script injection.

**How it runs.** axe's source is sent through the driver's `page.eval` as part of an ordinary function, once per frame (with
`awaitPromise`, because `axe.run` is asynchronous; a driver advertises that as the capability `evalAsync`, and both shipped drivers
do). It is never run through the page's own `eval`, so a page with a Content-Security-Policy such as `script-src 'self'` still works.
In the page only what is needed leaves it: the rule, impact, tags, help text and, for each failing element, axe's selector and its
box. The page's HTML is dropped. A target where axe cannot run (a driver without `evalAsync`, a timeout, a frame that refuses
scripts) gets the warning `axe_failed` and the run still completes.

**Placing on components.** The element's box is measured the way the collector measures (position plus scroll, moved into
top-level page coordinates for a frame), and then matched in Rust, in the element's own frame: an instance with the same box and tag is
the element itself; otherwise the smallest instance whose box contains the element's centre is its nearest enclosing component
(`viaAncestor: true`). An element in no component gets no `componentId` and its finding is about the page.

### `axe.json`

`AxeResults` in `spec/run-folder.schema.json`: `axeVersion`, `tags` (the rule tags that ran), `violations`, `incomplete` (how many
results axe left for a person) and `errors` (frames where axe did not run, and rules whose list of elements was cut at 200). Each
violation has `rule`, `impact`, `tags`, `help`, `helpUrl` and `nodes`; each node has `frameId`, `target` (axe's selector in its frame),
`box` (page coordinates) and `componentId` / `viaAncestor` when a component contains it.

### axe findings

`traceqa findings` turns each rule and component into a finding with the rule `axe.<ruleId>` (for example `axe.color-contrast`) and the
id `axe.<ruleId>:<componentId>`, or `axe.<ruleId>:page` when no component contains the elements (its `componentId` is `page`).
Severity follows axe's impact: `critical` and `serious` are `error`, `moderate` is `warning`, `minor` is `info`. `wcag` lists the success
criteria in axe's tags (`wcag143` is `1.4.3`), `helpUrl` is axe's page for the rule and `evidence` has `source: "axe-core"`,
`axeVersion`, `impact`, the number of elements and up to five selectors. Confidence is `medium`. Suppressions work on these ids like
any other. axe is not tamper-proof: a hostile page that replaces built-in functions can make it report wrongly (see
`collector_tampered`), as it can any in-page script.

## `traceqa codegen`

Writes a Playwright page object from a finished run: one class, one property per
component, in TypeScript or Python.

```
traceqa codegen runs/r_20261013_210455_a1b2 --lang ts
traceqa codegen runs/r_20261013_210455_a1b2 --lang py --out tests/pages --target chromium
```

| Option | Meaning |
|---|---|
| `RUN` | A run folder made by `traceqa extract`. |
| `--lang ts\|py` | TypeScript (`<name>.page.ts`, `@playwright/test`) or Python (`<name>_page.py`, the sync API). |
| `--out DIR` | Where to write. Default `./traceqa-pages`. |
| `--target NAME` | Use only this target, by folder (`chromium-1280x800`) or browser (`chromium`). Repeat for several. Default: every completed target. |
| `--class-name NAME` | The class. Default: made from the page title, for example `BuggyShopPage`; `AppPage` when the title has no ASCII letters. |
| `--include-hidden` | Also write components that are hidden in every target. They are found with `includeHidden`. Default: left out, and listed under `leftOutAsHidden` in `locators.json`. |

It reads the `locator` of each component in every selected target (see
[Output files](#output-files)) and writes only what holds in all of them. A component is
`unique` in the page object only when it is unique, with the same descriptor, in every
browser that has it. When the browsers disagree the descriptor with the most
ancestors is used and the property says `DIFFERS`.

What a property looks like (TypeScript; the Python class is the same with `@property`):

```ts
  // traceqa:id c_5e19c7...
  // role: "button", name: "Add to cart"
  /** A multi-match locator: the page had 12 of these. Use all(), count() or filter(). */
  get addToCartButton(): Locator {
    return this.page.getByRole("button", { name: "Add to cart", exact: true });
  }
```

* **Order of preference:** role and exact name; the `data-testid` every instance shares;
  role (and name) inside the nearest named ancestors that tell it apart; the exact visible
  text for elements with no role; and last a CSS selector, marked `FORCED`. Names are
  always `exact: true` because Playwright's default is a case-insensitive substring.
* **`nth()` is never written.** A component that appears several times is a multi-match
  locator, and its docstring says so. One that cannot be told apart from another component
  says `AMBIGUOUS` (with the other ids) or is `FORCED` to CSS, never silently.
* **Frames:** a component in an iframe goes through `frameByLabel(page, label)` /
  `frame_by_label(page, label)`, written at the top of the file. The label is the iframe's
  name, else the path of its address, lower case, as in the component's path.
* **Markers:** every property has a `traceqa:id c_...` comment directly above it, so a tool
  can find and rewrite it later. Property names are ASCII (`addToCartButton` /
  `add_to_cart_button`); two components with the same name both get the first six hex
  digits of their id (`homeLink_0f4b2a`). Names are stable from run to run.
* **Locale:** names are matched exactly, so a page object is for the language it was read
  in. The file says so in its header.
* **Page content that loads on scroll or after a click** is not there until the test makes
  it so; the counts in the run were taken after the extraction scrolled the page.
* **Safety:** names, test ids, selectors and frame labels are page text. They are written
  only as escaped string literals (quotes, backslashes, line breaks, U+2028 and U+2029 and
  invisible or bidirectional characters become `\uXXXX`), never raw, and a comment holds
  them only in that form. Class names and file names are ASCII, and a run folder that names
  a target outside itself is refused.

`locators.json` is written next to the page object. It maps each component id to its
`property`, `role`, `name`, the merged `locator`, `presentIn` and `uniqueIn` (targets),
`unique`, `forced`, `differsAcrossTargets` and `matchesAtMost`.

Exit code 0 on success, 2 when the command is wrong, the run cannot be read, or the run was
made by a `traceqa` from before locators existed (extract the page again).

## `traceqa heal`

Sorts what a run lost against a baseline into what can be fixed mechanically, what a person has to
decide, and what is a failure. It is a query over a diff: it reads the run and the baseline, never
starts a browser, and never changes the baseline's components (only `traceqa baseline promote`
moves the reference).

```sh
traceqa heal runs/r_20261025_064332_b084 --against checkout
traceqa heal runs/r_20261025_064332_b084 --against checkout --page-object tests/pages/shop.page.ts
traceqa heal runs/r_20261025_064332_b084 --against checkout --accept c_8a1f03b2c4d5 --page-object tests/pages/shop.page.ts
```

| Option | Meaning |
|---|---|
| `RUN` | A run folder, a target folder, or the id of a run in `--runs-dir`. |
| `--against NAME\|DIR` | The baseline: its name in `--baseline-dir`, or a baseline folder. Required. |
| `--baseline-dir DIR` | Where `--against NAME` looks (default `.traceqa/baselines`). |
| `--target NAME` | Look at one target folder of the baseline only. |
| `--page-object PATH` | A page object made by [`traceqa codegen`](#traceqa-codegen) (`.ts` or `.py`) to patch. Repeat for several. The unified diff is printed (to standard error) before the file is written. |
| `--accept ID` | Accept the rename proposal for the baseline component `ID`. Repeat for several. |
| `--allow-in-ci` | Allow `--accept` and `--page-object` when `CI` or `GITHUB_ACTIONS` is `true`. |
| `--format markdown\|json` | Markdown (default) for people; `json` is the `HealReport` contract (`spec/heal.schema.json`). |
| `--runs-dir DIR` | Where bare run ids are looked up. |

The report is also written to `heal.json` in the run folder.

### The five outcomes

Every component of the baseline that the run does not have as it was gets one item. Components
that are unchanged are only counted.

| `kind` | When | `action` |
|---|---|---|
| `locator_fix` | The same id is in the run, and its locator is different (it needed a landmark, or lost its test id). The id is the same, so the component is the same; only the way to find it changed. | `fix` |
| `rename_proposal` | The id is gone, and exactly one component of the run is a match, with no other removed component competing for it (a 1:1 match). | `propose` |
| `removed` | The id is gone and nothing in the run is a safe match. | `fail` |
| `ambiguous` | More than one component could be the match, on either side. Nothing is proposed; the candidates are listed. | `refuse` |
| `session_not_accepted` | The run carries a `session_not_accepted` warning: it shows a sign-in page, not the page of the baseline. This is checked first and nothing else is healed. Sign in again (`traceqa login`) and extract again. | `refuse` |

**What makes a match.** The role must be equal and so must the landmark (the first landmark step
of the path, such as `main` or `navigation "contents"`). Then either both sides carry the same
`data-testid` (confidence `high`), or the names are alike (normalised: Unicode NFC, lower case,
whitespace collapsed; Levenshtein ratio of at least 0.8) and the centre of the box moved by no
more than 10 percent of the viewport on each axis (confidence `medium`). A different role is never
a match, and a similar name far away is not one either. The 1:1 rule is never relaxed to get more
heals: a rename that is really a removal plus an unrelated addition stays `removed` or `ambiguous`.

**Exit codes.** `0` only when every item is `none` or `fix` (or a proposal was accepted in this
command). `1` when anything is a proposal, a removal, ambiguous or refused. `2` for input that
cannot be used: a baseline target the run lacks, ids of another fingerprint version, an `--accept`
for an id that has no proposal, or a refused write (below).

### Accepting a proposal, and CI

A rename is never applied by itself. `--accept ID` is the person's decision: it records the alias
`{from, to, reason: "heal", evidence, addedAt}` in the baseline (the same as `traceqa baseline
alias`, which `traceqa diff --against` then honours: the pair is accepted and the gates ignore it)
and patches the page objects named with `--page-object`.

`--accept` and any write to a page object are **refused with exit 2 when `CI=true` or
`GITHUB_ACTIONS=true`**, unless `--allow-in-ci` is given. In CI, heal can only report. A
`--page-object` without `--accept` writes the mechanical `locator_fix` items only; renames are
shown in the report and patched only after `--accept`.

### Patching a page object

Only the `return` line of a property is ever changed, and only under a `traceqa:id c_...` marker
that names the component. The line must still hold exactly the expression that codegen wrote for
the baseline: a line a person edited is kept and reported on standard error. When the targets
(browsers) of the baseline need different new locators for one component, it is not patched. The
patch is idempotent: running it again changes nothing. An accepted rename also moves the marker to
the new id. Everything else in the file, comments and line endings included, stays byte for byte.

## Pages behind a sign-in

Extraction starts every browser empty, so a page behind a sign-in, an
onboarding step or a "continue as a guest" screen shows that screen instead of
itself, at the same address. A state file gets past it: the cookies and stored
values a browser holds once someone is in. It is Playwright's `storageState`
format, so a file made by Playwright tests works too.

### `traceqa login`

```
traceqa login URL --save FILE [--browser NAME]
```

A browser window opens on the page. Sign in there as usual, with a password, a
code or another service, then come back and press Enter. The browser's state is
saved to FILE, readable only by you. TraceQA never sees the password. Then:

```
traceqa extract URL --storage-state FILE
```

A state saved in one browser works in the others. Through WebDriver, as for
Safari, only the cookies and stored values of the site the window ends on are
saved. `--headless` saves as soon as the page has loaded, for scripts and tests
where the page signs in by itself.

Ctrl-C (Ctrl-Break on Windows), or SIGTERM or SIGHUP on Unix, while the command
waits stops it with exit code 130: the page is closed, the driver is shut down
(within about 10 seconds), and the hidden temporary file is removed, so nothing
is saved; an existing FILE is left as it was. A second signal quits at once and
kills the driver.

How FILE is written:

- `--save` is checked before the browser opens. If FILE is a directory, or its
  folder does not exist, the command stops at once with exit code 2, so you do
  not sign in for nothing. A bad address exits with 2 as well: it must be an
  `http` or `https` address, and must not hold a user name or password
  (`https://user:pass@host/`). Sign in in the window instead.
- The state goes to a hidden temporary file next to FILE, which is made
  readable by its owner alone before anything is written to it, and is then
  moved over FILE. FILE is never readable by others, not even briefly. A
  failure leaves no temporary file behind. An existing FILE is replaced, and
  ends up owner-only even if it was not before. A link is replaced by the new
  file, not written through.
- On Linux and macOS the mode is `600`. On Windows the file's access list is
  set to the owner alone (`OWNER RIGHTS`, with inherited entries removed) by
  running `icacls` from the Windows folder. That also takes away the access
  that SYSTEM and administrators would otherwise have: an administrator can
  still take ownership of the file. If the access list cannot be set, nothing
  is saved and the command fails.

### A state file by hand

When a site only checks a stored value, the file can be written by hand:

```json
{
  "cookies": [],
  "origins": [
    { "origin": "https://app.example.com", "localStorage": [{ "name": "has_onboarded", "value": "true" }] }
  ]
}
```

### Keeping it safe

| Rule | Why |
|---|---|
| Treat a state file as a password | It opens the account until the session expires. |
| Keep it out of version control | The repository's `.gitignore` already ignores `state.json`, `*.state.json` and `storage-state*.json`. |
| Use a test account with made-up data | A signed-in page can show personal data, and its names and pictures end up in the run folder. |
| Prefer a staging copy of the site | Extraction only reads, but later features will click. |
| In CI, keep the file in the CI system's secrets | Write it to disk in the job, and do not upload it. |

TraceQA never writes the state to a run folder, an event, a log or an error
message. When a run that was given a state still lands on what looks like a
sign-in page, its address or a password field, the target has a
`session_not_accepted` warning: sign in again to make a new file.

### Pictures of signed-in pages

Pictures are opt-in (`--crops`), but when a run has both a state and `--crops`,
the pictures show the signed-in page as it was: `page.png` and `crops/` in each
target's folder can contain names, e-mail addresses or balances. Each such target
gets a `signed_in_pictures` warning as a reminder. Keep the run folder private
(the repository's `.gitignore` already ignores `/runs/`), and delete it when you
are done. Leave out `--crops` when you only need the components.

## Python SDK

```python
import traceqa

result = traceqa.extract("https://example.com", browsers=["chromium", "firefox"])
```

`traceqa.extract(url, **options)` starts an engine, runs one extraction and
stops the engine. To make several calls on one engine process:

```python
with traceqa.Engine() as engine:
    first = engine.extract("https://example.com")
    second = engine.extract("https://example.org", browsers=["webkit"])
```

### Options

| Option | Default | Meaning |
|---|---|---|
| `browsers` | `("chromium",)` | Browser values as on the command line, or `BrowserTarget` objects. |
| `viewports` | `((1280, 800),)` | `(width, height)` pairs. Each side is 1 to 10000; a size outside that raises `TraceQAError` before anything starts. |
| `out_dir` | `runs` | Where the run folder is created. |
| `headless` | `True` | Hide the browser windows. |
| `crops` | `False` | Also save a screenshot of the page and a picture of each component. |
| `include_hidden` | `True` | Include elements that are not visible. |
| `max_scroll_steps` | `50` | How many screens to scroll through. `0` reads the page without scrolling. |
| `timeout_ms` | `30000` | Time allowed for a page to load. |
| `settle_ms` | `0` | Extra time to wait after the page has loaded. For pages with a timed intro. |
| `parallel` | chosen from the machine | How many browsers run at the same time. |
| `storage_state` | none | A state file's path, or the state as a dict, for pages behind a sign-in. |
| `on_event` | `None` | Called with each progress event as it happens. |

`traceqa.extract` and `traceqa.Engine` also accept `binary`, the path of the
engine, and `drivers`, a list of driver commands. Without `binary`, the SDK uses
the `TRACEQA_BIN` environment variable and then `traceqa` on `PATH`.

The SDK prints nothing by default, as a library should. The engine and its drivers
log to standard error (for example `[driver info] opened chromium 128.0`); the SDK
reads that and drops it. Pass `on_log=` (to `traceqa.extract` or `traceqa.Engine`)
to receive each line as a string, already redacted by the engine; a callback that
raises is ignored from then on and the reading goes on. When the engine dies,
`EngineExitedError.stderr_tail` holds its last stderr lines (at most 20 lines of 500
characters, 4000 in all, the same bounds as the TypeScript SDK) and the message ends
with them after `engine stderr (last lines):`.

### Result

| Member | Meaning |
|---|---|
| `result.status` | `completed`, `failed` or `cancelled`. |
| `result.targets` | One summary per browser and viewport: folder name, status, counts, duration, error. |
| `result.components(target_dir)` | The components found for one target. The argument may be omitted when the run has one target. |
| `result.run_folder` | Path of the run folder. |
| `result.manifest` | The full run manifest. |

A browser that fails does not raise an exception. Inspect `result.status` and
each target's `status` and `error`.

### Diff and findings

`engine.diff(a, b, target_a=None, target_b=None)` returns a `DiffFile` and
`engine.findings(run, target=None, suppressions=None)` a `FindingsResult` (one
`FindingsFile` per target, then one for the run when it has something to say). They
read run folders (paths, or the results of `extract`), start no browser and write
nothing. They need an engine that lists `diff` or `findings` in `engine.info.features`;
an older one raises `TraceQAError` of kind `unsupported`. A wrong folder or target
raises `EngineRefusedError` (kind `invalid_params`) with a message that says what to
fix. They cannot overlap an `extract` on the same `Engine`.

### Errors

Every error is a `TraceQAError`, which has a `kind` (the engine's, when it sent
one). The engine's own failures are subclasses:

| Class | When |
|---|---|
| `EngineStartError` | The engine was not found, or could not be started. |
| `EngineExitedError` | The pipe closed or the process died. `exit_code` is set when known; `stderr_tail` (read-only) holds the engine's last stderr lines, or `None`. |
| `EngineRefusedError` | The engine answered a request with an error. Has `kind` and `code`. |
| `EngineProtocolError` | The engine sent a line that is not JSON, or a message of the wrong shape. The message quotes at most the first 80 characters of the line. |

A plain `TraceQAError` is a mistake in the call: a bad browser string, a bad
storage state file, `components()` on a run with several targets. Errors chain
with `raise ... from`, so the original exception is in `__cause__`.

### Cancel, close and one run at a time

`engine.cancel()` may be called from any thread. It cancels the running
extraction, which then returns with status `cancelled`; it does nothing when no
run is active. Ctrl+C inside `extract` cancels the run before
`KeyboardInterrupt` propagates.

An `Engine` runs one extraction at a time, on purpose: a second `extract` while
another is active raises `TraceQAError`. Use one `Engine` per concurrent run.

`engine.close(timeout=30.0)` cancels a run that is still going, closes the
engine's input and waits up to `timeout` seconds for it to exit, then kills it.
Leaving a `with` block calls it.

## TypeScript SDK

Node 24 or later. The package is ESM only and has no runtime dependencies. See
[its README](../sdks/typescript/README.md#esm-only) for loading it from CommonJS.

```ts
import { extract } from "traceqa";

const result = await extract("https://example.com", { browsers: ["chromium", "firefox"] });
const components = await result.components("chromium-1280x800");
```

To make several calls on one engine process:

```ts
import { Engine } from "traceqa";

const engine = await Engine.start();
try {
  const first = await engine.extract("https://example.com");
  const second = await engine.extract("https://example.org", { browsers: ["webkit"] });
} finally {
  await engine.close();
}
```

One engine can run several extractions at the same time: start them with
`Promise.all`. Each run's events go only to its own `onEvent`.

`engine.close({ timeoutMs })` cancels runs that are still going, ends the
engine's input and waits up to `timeoutMs` (default 30000) for it to exit, then
kills it.

Errors are the Python ones, with the same names and camel-case fields:
`TraceQAError` (a caller's mistake, and the base of the others),
`EngineStartError`, `EngineExitedError` (`exitCode`, `stderrTail`), `EngineRefusedError`
(`kind`, `code`) and `EngineProtocolError`. The original error is in `cause`.
A protocol error fails the whole connection, because the stream can no longer be
trusted. `components()` checks the file it reads and rejects with
`EngineProtocolError` when a required field is missing.

The options are the Python ones in camel case: `browsers`, `viewports`
(`[width, height]` pairs), `outDir`, `headless`, `crops`, `includeHidden`,
`maxScrollSteps`, `timeoutMs`, `settleMs`, `parallel`, `storageState` and `onEvent`. Two differ:

| Option | Meaning |
|---|---|
| `signal` | An `AbortSignal`. Aborting it cancels the run, and the result resolves with status `cancelled`. |
| `onEvent` | As in Python. If it throws, the run is cancelled and `extract` rejects with that error. |

`extract` and `Engine.start` also accept `binary` and `drivers`, found the same
way as in Python.

The SDK writes nothing to your standard error by default. The engine's and the
drivers' log lines (`[driver info] opened chromium 128.0`) are read and dropped,
so the engine never blocks on a full pipe. Pass `onLog: (line) => console.error(line)`
to receive each line, already redacted by the engine; a handler that throws is
ignored from then on. When the engine dies, `EngineExitedError.message` ends with
its last stderr lines, and `stderrTail` holds them (at most 20 lines and 4000
characters). The result has `status`, `targets`, `manifest`, `runFolder`
and `components(targetDir)`, which returns a promise. The spec types are
exported, the common ones by name and every one under `protocol`.

`engine.diff(a, b, { targetA, targetB })` resolves with a `DiffFile` and
`engine.findings(run, { target, suppressions })` with a `FindingsResult`. `a`, `b` and
`run` are run folders, as paths or as the results of `extract`. Both read local files,
start no browser and write nothing, and may run at the same time as an extraction. The
answers are checked at run time like `components.json`: a required field of the wrong
type rejects with `EngineProtocolError`, without quoting the value. An engine that does
not list `diff` or `findings` in `engine.info.features` rejects with `TraceQAError` of
kind `unsupported`; a wrong folder or target rejects with `EngineRefusedError`
(`invalid_params`) and a message that says what to fix.

## Other languages

Any language that can start a process and read and write lines can call the
engine. See [clients.md](clients.md).

## Output files

```
runs/<runId>/
  run.json
  events.jsonl
  <browser>-<width>x<height>/
    components.json
    axe.json                  only with --axe
    findings.json             only after `traceqa findings`
    page.png                  only with --crops
    crops/<componentId>.png   only with --crops
```

Pictures are saved only with `--crops`. `page.png` is an overview: the whole
page when the browser can capture that, and the first screen otherwise. Crops are not cut from it. They come from the
screens the engine saw while scrolling, so a component in an animated or pinned
section is shown as it looked when it was on screen.

### `run.json`

The run manifest: run id, URL, start and finish time, overall status, and one
entry per target with its status, component count, instance count, warning
count, duration and, on failure, the error.

The URL here, in `components.json` (`url` and `finalUrl`) and in events is
written with the value of secret parameters hidden: `token`, `access_token`,
`id_token`, `refresh_token`, `api_key`, `apikey`, `secret`, `client_secret`,
`password`, `passwd`, `sig`, `signature`, `x_amz_signature`,
`x_amz_credential` and `x_amz_security_token`, in the query and in the fragment,
become `REDACTED`. The browser still loads the address as you gave it. A URL
without these is stored exactly as given. A URL with a user name or password
(`https://user:pw@host/`) is refused with exit code 2: sign in with
`traceqa login` and pass `--storage-state` instead.

Each completed target also has `timings`, in milliseconds:

| Field | Time spent |
|---|---|
| `openMs` | Starting the driver if needed, and opening the browser. |
| `loadMs` | Loading the page and waiting for it to settle, including any `--settle-ms`. |
| `readMs` | Running the collector. |
| `scrollMs` | Scrolling, and waiting for the page to settle after each scroll. |
| `screenshotMs` | Taking screenshots. |
| `cropMs` | Cutting the crops. |
| `screens` | Not a time: how many screens the page was read in. |

### `events.jsonl`

One JSON object per line, numbered from zero. The same events are delivered
live to an SDK's `on_event` callback.

| Event type | When |
|---|---|
| `run.started`, `run.finished` | Start and end of the run. |
| `target.started`, `target.finished` | Start and end of one browser at one viewport. `target.finished` carries its timings. |
| `target.retried` | A browser timed out while others were running and is being tried again on its own. |
| `target.cancelled` | A target was stopped by a cancel while its browser or driver was already going away. The message holds what the driver said; the target's status is `cancelled` and has no `error`. |
| `page.opened` | The page has loaded. |
| `page.scrolled` | The page has been scrolled through. |
| `elements.collected` | The collector has returned. |
| `components.grouped` | Elements have been grouped into components. |
| `crop.saved` | Screenshot crops have been written. |
| `warning` | A non-fatal problem. See below. |
| `error` | A target failed. |

### `components.json`

| Field | Meaning |
|---|---|
| `id` | The component identifier: `c_` followed by 12 hexadecimal characters. Stable across browsers, viewports and runs. |
| `role` | ARIA role, explicit or implied by the tag. `generic` when there is none. |
| `name` | Accessible name. `*` when `nameIsTemplate` is true. |
| `nameIsTemplate` | True when the instances have different names, as with product titles in a grid. |
| `confidence` | `high` for native controls and explicit interactive roles. `low` for elements that only look interactive. |
| `signals` | Why the element is considered interactive: `native`, `role`, `contenteditable`, `tabindex`, `onclick-attr`, `cursor-pointer`. |
| `path` | The structural path to the component, from its innermost named ancestor down (the whole path when none is named), for example `list "products" > listitem`. |
| `crop` | Path of a crop of the component, relative to `components.json`. Absent without `--crops`, and when the component never appeared on screen. |
| `locator` | How to find the component again with Playwright. See [Locator](#locator). |
| `instances` | Every element that belongs to the component. |

Each instance has a `key`, its own `name`, `nameSource` (where that name came
from: `aria-label`, `aria-labelledby`, `label`, `content`, `alt`, `title`,
`placeholder` or `value`; absent when the name is empty), `tag`, a subset of `attrs`
(`id`, `data-testid`, `type`, `href`, `name`, `placeholder`, `tabindex`,
`autocomplete`, `role`; these do not feed the component id),
`selectors`, a `box` in page coordinates, `state`, the `frameId` it was found in
and `inShadowDom`.

A selector is a CSS selector within the element's frame. For an element inside
shadow DOM it has parts joined by ` >>> `, each resolved inside the shadow root
of the element the previous part matched.

### Locator

`locator` describes how to find the component with Playwright's locators, and is what
[`traceqa codegen`](#traceqa-codegen) turns into code. `strategy` is `role` (`role`, and
`name` matched exactly), `testid` (`testId`), `text` (`name`, exact) or `css` (`css`, only
with `forced`). `scope` lists ancestors to search inside, outermost first (`role` and
`name`), `frame` is the iframe's label, and `needsIncludeHidden` is true when every instance
is hidden. `matches` is how many collected instances the locator finds in this file and
`own` how many of them belong to the component; `unique` means they are equal and cover
every instance, and `ambiguousWith` lists up to five other components it also finds. The
counts follow Playwright: an exact name is compared with whitespace squashed and case kept,
and hidden elements are not found unless `needsIncludeHidden`. They only cover this file, so
a locator is proven unique for one page in one browser; `codegen` combines the browsers.
Roles the collector reports but Playwright does not see (`summary` as a button, a
`contenteditable` element as a textbox) are never looked up by role.

### Warnings

| Code | Meaning |
|---|---|
| `unnamed_interactive_element` | An interactive element has no accessible name. |
| `cross_origin_frame_skipped` | A frame from another origin could not be read. |
| `frame_eval_failed` | A same-origin frame could not be read. |
| `scroll_limit_reached` | The page is longer than the scroll limit, so its end was not read. |
| `screenshot_failed` | The browser could not take a screenshot, so some crops are missing. The component data is unaffected. |
| `browsers_disagree` | Browsers found different numbers of elements at the same viewport. Reported once per such viewport, in `events.jsonl`. The message names the component ids each browser lacks (up to 5 per browser, `and N more` after that), never page text; the event data holds `viewport`, `counts`, `missing` (up to 20 ids per browser) and `missingTotal`. It fires when the instance counts differ; browsers that find the same number of different elements raise no warning, but `traceqa findings` reports them as `browsers-disagree` findings. |
| `http_error_status` | The page answered with an HTTP error, such as 404 or 500. It was still read, since an error page can be the thing under test. WebDriver browsers do not report the status, so they never raise this. |
| `page_load_incomplete` | The page's content arrived, but it had not finished loading when the time limit ran out, for example because one image never arrived. It was read as it stood. A page whose content never arrives fails instead. |
| `covered_elements` | Components on screen lie under something else, such as a cookie banner or a modal's backdrop, so a click would not reach them. Each such instance has `covered: true` and `coveredBy`. |
| `closed_shadow_roots` | The page has closed shadow roots, which no script can read inside, so what they hold is not listed. Only Chromium-based browsers can tell. |
| `elements_removed_while_scrolling` | The page removed controls it had shown as it was scrolled, as long tables and feeds that keep only the rows near the screen do. The rows that had left the screen are not listed. |
| `session_not_accepted` | The run was given a state, but the page is still a sign-in page by its address or a password field. The session has probably expired. |
| `signed_in_pictures` | The run used a stored session and `--crops`, so `page.png` and `crops/` may show personal data. Raised once per target; nothing else about what is saved changes. |
| `collector_tampered` | The page replaced built-in functions such as `Array.prototype.map` or `getBoundingClientRect`, which can make a script read the page wrongly. The warning names them. With Playwright (Chromium, Firefox, WebKit) the driver captured the browser's own functions before the page's scripts ran and the page was read with those, so the result is right and the warning is for information. With a WebDriver browser (Safari, or any browser behind a WebDriver server) there is no way to run a script first, so the collector uses what it finds, spots functions that no longer look like native code, and says the result may be wrong. This check is best effort: a page can also fake how a function prints itself, and a few reads that are not captured (such as the `checked` state of an input) are not defended. |
| `requests_failed` | Requests for scripts, styles, data (fetch or XHR) or frames failed while the page loaded, by a network error or an HTTP status of 400 or more. The warning gives the count and the first few addresses, without query strings, because parts of the page may then be missing from the list. Pictures, fonts and requests cancelled by a navigation are not counted. Only Playwright browsers (Chromium, Firefox, WebKit) report this; a WebDriver browser such as Safari cannot see failed requests and never raises it. |
| `axe_failed` | `--axe` was asked for, but axe-core did not run (or not in every frame): the driver cannot wait for a script that returns a Promise, the script timed out, or a frame refused it. The message says which. The components are unaffected, and there is no `axe.json` when axe ran nowhere. |
| `viewport_not_reached` | The browser could not give the viewport asked for, for example Safari in a window macOS will not resize. The page was read at the size it really had, which `components.json` records. The target keeps its name, such as `safari-390x844`. |

## Speed

Three things decide how long a run takes.

**How many browsers run at once.** By default several do, so a run takes about
as long as its slowest browsers and not all of them added up. Use `--parallel`
to change the number. A browser that times out while others are running is
tried once more on its own, because a page that is heavy to render can time out
only for lack of a free processor.

**Whether pictures are wanted.** By default they are not: the page is only
scrolled, to load what loads on scroll, and read once at the end. With
`--crops` the page is gone through a screen at a time: read, photographed,
scrolled, and given time for its animations. The components and their ids are
the same either way.

**The page itself.** Its length, how long it takes to load, and how long its
intro runs. `--timings` shows which of these a run spent its time on.

Measured on one machine, three browsers at two viewports:

| Page | Default | With `--crops` | With `--crops --parallel 1` |
|---|---|---|---|
| Demo shop | 7 s | 8 s | 24 s |
| Marketing page, 10,000 px, intro and scroll animations | 26 s | 34 s | 94 s |
| Portfolio, 21,000 px | 27 s | 86 s | about 250 s |

## Pages that are not ready when they load

TraceQA waits for the load event, a quiet network, a stable page height and the
end of any running intro animation. A page can still be unready after all of
that: a splash screen held by a timer gives no sign that more is coming.

When browsers report different numbers of elements for the same page, the run
ends with a `browsers_disagree` warning that names the ids each browser lacks. Give such a page time with
`--settle-ms`:

```sh
traceqa extract https://example.com --settle-ms 5000
```

## Demo site and bench

```sh
uv run python fixtures/buggy-shop/serve.py     # http://127.0.0.1:4173/
uv run bench/run.py                            # all browsers, two viewports, three repeats
uv run bench/run.py --browsers chromium --repeats 1 --json scores.json
```

The bench compares extraction results with
`fixtures/buggy-shop/manifest.yaml` and reports, per target, how many expected
components were found, how many unexpected ones were reported, how many differed
from the manifest, whether identifiers were stable across all runs, and the
median time. It exits with a non-zero status when any score falls short.

`traceqa verify --manifest fixtures/buggy-shop/manifest.yaml RUN --strict` applies the same checks to
one run and agrees with the bench on the shop (`tests/test_verify.py` asserts it). The bench stays
the scorer for the matrix of browsers and repeats.
