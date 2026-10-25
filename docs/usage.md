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
| `--max-scroll-steps N` | `50` | How many screens to scroll through on a long page. `0` reads the page without scrolling. |
| `--timeout-ms MS` | `30000` | Time allowed for a page to load. At least 1. |
| `--settle-ms MS` | `0` | Extra time to wait after the page has loaded, before reading it. For pages with a timed intro or splash screen. |
| `--parallel N` | chosen from the machine | How many browsers run at the same time. `1` runs them one after another. The default is half the machine's cores, at most 4. At least 1. |
| `--timings` | off | Also print where each browser's time went. |
| `--json` | off | Print the run manifest as JSON instead of a table. Standard output holds that one document and nothing else; progress and the `Run folder:` line go to standard error. |
| `--storage-state FILE` | none | Start every browser with the cookies and stored values in FILE, for pages behind a sign-in. See [Pages behind a sign-in](#pages-behind-a-sign-in). |
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
| `--format markdown\|json` | Markdown (the default) is for people. `json` is the `DiffFile` contract (`spec/diff.schema.json`). |
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
the same TraceQA). `1` is kept for gates that fail on a difference.

The report is the same bytes every time for the same inputs.

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
traceqa mcp [--out DIR] [--state-dir DIR] [--allow-remote-browsers]
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
| `get_components` | The components of one run and target: a short line each, filtered by `role` or `name`, or one component in full with `component_id`. |
| `get_crop` | The picture of one component, for runs made with `crops`. |

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
again.` The reading tools (`list_runs`, `get_components`, `get_crop`) have a
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
cut it says `truncated: true` with a note), `get_crop` refuses pictures over
2 MiB (and only pictures that really sit inside the runs folder, never
through a link), and error texts are cut to 500 characters. No answer, error or
progress message shows a local path: the runs folder is never named, and other
absolute paths in engine errors are replaced by `<path>`. The tools carry MCP
annotations: `extract` opens pages (open world, not read-only); the other
three are read-only.

## `traceqa install`

```
traceqa install [BROWSERS]... [--from FOLDER] [--ref REF] [--with-deps] [--dry-run]
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

Node 22 or later. The package is ESM only and has no runtime dependencies. See
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
| `instances` | Every element that belongs to the component. |

Each instance has a `key`, its own `name`, `tag`, a subset of `attrs`,
`selectors`, a `box` in page coordinates, `state`, the `frameId` it was found in
and `inShadowDom`.

A selector is a CSS selector within the element's frame. For an element inside
shadow DOM it has parts joined by ` >>> `, each resolved inside the shadow root
of the element the previous part matched.

### Warnings

| Code | Meaning |
|---|---|
| `unnamed_interactive_element` | An interactive element has no accessible name. |
| `cross_origin_frame_skipped` | A frame from another origin could not be read. |
| `frame_eval_failed` | A same-origin frame could not be read. |
| `scroll_limit_reached` | The page is longer than the scroll limit, so its end was not read. |
| `screenshot_failed` | The browser could not take a screenshot, so some crops are missing. The component data is unaffected. |
| `browsers_disagree` | Browsers found different numbers of elements at the same viewport. Reported once for the run, in `events.jsonl`. |
| `http_error_status` | The page answered with an HTTP error, such as 404 or 500. It was still read, since an error page can be the thing under test. WebDriver browsers do not report the status, so they never raise this. |
| `page_load_incomplete` | The page's content arrived, but it had not finished loading when the time limit ran out, for example because one image never arrived. It was read as it stood. A page whose content never arrives fails instead. |
| `covered_elements` | Components on screen lie under something else, such as a cookie banner or a modal's backdrop, so a click would not reach them. Each such instance has `covered: true` and `coveredBy`. |
| `closed_shadow_roots` | The page has closed shadow roots, which no script can read inside, so what they hold is not listed. Only Chromium-based browsers can tell. |
| `elements_removed_while_scrolling` | The page removed controls it had shown as it was scrolled, as long tables and feeds that keep only the rows near the screen do. The rows that had left the screen are not listed. |
| `session_not_accepted` | The run was given a state, but the page is still a sign-in page by its address or a password field. The session has probably expired. |
| `signed_in_pictures` | The run used a stored session and `--crops`, so `page.png` and `crops/` may show personal data. Raised once per target; nothing else about what is saved changes. |
| `collector_tampered` | The page replaced built-in functions such as `Array.prototype.map` or `getBoundingClientRect`, which can make a script read the page wrongly. The warning names them. With Playwright (Chromium, Firefox, WebKit) the driver captured the browser's own functions before the page's scripts ran and the page was read with those, so the result is right and the warning is for information. With a WebDriver browser (Safari, or any browser behind a WebDriver server) there is no way to run a script first, so the collector uses what it finds, spots functions that no longer look like native code, and says the result may be wrong. This check is best effort: a page can also fake how a function prints itself, and a few reads that are not captured (such as the `checked` state of an input) are not defended. |
| `requests_failed` | Requests for scripts, styles, data (fetch or XHR) or frames failed while the page loaded, by a network error or an HTTP status of 400 or more. The warning gives the count and the first few addresses, without query strings, because parts of the page may then be missing from the list. Pictures, fonts and requests cancelled by a navigation are not counted. Only Playwright browsers (Chromium, Firefox, WebKit) report this; a WebDriver browser such as Safari cannot see failed requests and never raises it. |
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
ends with a `browsers_disagree` warning. Give such a page time with
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
