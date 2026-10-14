# Plan

TraceQA is an agentic QA tool. You give it a site and it reports what is broken.
It will have two modes: check one feature, or sweep the whole site. It must work
in every browser, including agent browsers reached over CDP.

This file records what was decided and why, and where the build stands.

See [architecture.md](architecture.md) for how the parts fit together and
[usage.md](usage.md) for how to run it.

## Requirements

- **Any browser.** Chromium, Firefox, WebKit, real Safari, and agent or cloud
  browsers reached over CDP or WebDriver.
- **Any language.** Callers use a documented protocol, not a language binding.
- **macOS, Windows and Linux.** One codebase, no per-system branches, little
  upkeep. Every CI job should run on all three.

## Ways to use TraceQA

| Surface | Status |
|---|---|
| Command line | Built: `traceqa extract`. |
| Python SDK | Built: `traceqa.extract`. |
| TypeScript SDK | Built: `extract` and `Engine`, Node 18 or later, no runtime dependencies. |
| Any other language | The protocol is built (`traceqa serve`). [clients.md](clients.md) says how to write a client, and the recorded transcripts test it. |
| Releases | Built: pushing a tag such as `v0.1.0` publishes binaries for macOS, Windows and Linux. See [releasing.md](releasing.md). |
| CI action | Not started. The command line already works in CI through its exit code and `--json`. |
| MCP server | Built: `traceqa mcp`, with the tools `extract`, `list_runs`, `get_components` and `get_crop`. |
| Setup | Built: `traceqa install` and `traceqa doctor`. |
| Web dashboard | Not started. It needs the hosted service. |

## Decisions

| Decision | Why |
|---|---|
| The engine, the AI harness and the CLI are one Rust binary. | One place for all logic, so every driver and SDK behaves the same. A single binary is easy to ship. |
| Browser drivers are separate processes, written in Python. | Playwright has no official Rust binding. A driver holds no logic, so a new browser never changes the engine. |
| The page is read by a JavaScript collector injected into the page. | It is the only code that runs the same in every browser. It is plain JavaScript with no build step, so the engine can embed the file as it is. |
| Programs talk in JSON-RPC 2.0, one message per line, over stdio. | Every language can start a program and read and write lines. The same messages work over HTTP when hosted. |
| `spec/` JSON Schemas are the source of truth. Rust and Python types are generated. | The two sides cannot drift apart. |
| The engine returns a run folder and a live event stream. | Reports and dashboards only read files. Any run can be inspected later. |
| The engine works on snapshots (data). Screenshots are evidence for people. | Data is small, exact and comparable across browsers. |
| The engine is a library crate. The CLI is a thin crate on top. | The server and MCP crates can reuse the engine without depending on the CLI. |
| The engine picks a driver by asking each one which browsers it opens. | The engine contains no browser names. |
| Crops are cut from screenshots taken while scrolling through the page. | A single full-page screenshot misses content that animates in or is pinned while scrolling. It also needs no special ability from the browser. |
| The engine uses threads and blocking pipes, not an async runtime. | Each worker talks to one driver at a time. Nothing here needs more yet. |
| Browsers run in parallel, each worker with its own driver processes. | A run takes about as long as its slowest browsers. A driver that hangs takes only its own target with it. |
| Pictures are off unless asked for. Without them the page is only scrolled, then read once. | Nothing in the engine reads the pictures, and they cost most of the time on a long page. The features that come next work on the data. |
| Clients are tested against recorded conversations, played back by a stand-in engine. | Every client must send exactly the same messages. The recordings test that without a browser, and a client in a new language gets the same tests for free. |
| The stand-in engine is a Rust binary in the workspace. | It builds on every system with the engine, and a client test in any language can start it like the engine. |

## Feature 1: component extraction

`traceqa extract <url>` lists every interactive element on a page as data and as
a screenshot crop, the same way in every browser. It reads the page in the state
it is in and does not click anything. Check and sweep are built on its output.

### Build order

| # | Step | Status |
|---|---|---|
| 1 | Contracts: `spec/` schemas, generated types, repo skeleton | Done |
| 2 | Buggy Shop: demo site and `manifest.yaml` | Done |
| 3 | Collector: the in-page script | Done |
| 4 | Playwright driver | Done. Tested in Chromium, Firefox and WebKit, and attaching over CDP to a local Chromium. |
| 5 | Engine and CLI | Done |
| 6 | Bench and CI | Done. CI runs every test and the bench on Linux, macOS and Windows, and passes on all three. |
| 7 | Python SDK | Done |
| 8 | Safari and Kitesurf | Safari done, see below. Kitesurf not yet. |

### Measured on the Buggy Shop

`uv run bench/run.py`, three repeats:

| Target | Found | Extra | Wrong | Median time |
|---|---|---|---|---|
| chromium 1280x800 | 37 of 37 | 0 | 0 | 4.2 s |
| chromium 390x844 | 37 of 37 | 0 | 0 | 3.4 s |
| firefox 1280x800 | 37 of 37 | 0 | 0 | 4.7 s |
| firefox 390x844 | 37 of 37 | 0 | 0 | 4.6 s |
| webkit 1280x800 | 37 of 37 | 0 | 0 | 4.1 s |
| webkit 390x844 | 37 of 37 | 0 | 0 | 3.6 s |

Component ids were identical across all 18 runs. The same ids also came back
through the WebDriver driver.

These numbers say the pieces work together. They do not say how well extraction
does on real sites: the shop was written by the same hand as the collector, so
it only contains cases the collector was built for.

### Real sites

Four sites were run in three browsers at two viewports. Each result was checked
against an independent count of the page's links and controls, and the crops
were looked at.

| Site | Kind of page | Elements found | Components | Same ids in every browser | Time per browser and viewport |
|---|---|---|---|---|---|
| `www.avqentra.in` | Marketing page: intro animation, sections that appear on scroll, pinned carousel | 46 of 46 | 46 | Yes | 14 to 22 s |
| `portfolio-theta-lyart-35.vercel.app` | Very long portfolio (21,000 px), project cards, media controls | 73 of 73 | 58 | Yes | 31 to 56 s |
| `t3works.vercel.app` | Company site with a mega menu and video buttons | 59 of 59 | 50 | Yes | 17 to 28 s |
| `prabhav.avqentra.in` | Single-screen app behind a timed intro | 72 of 72 (desktop) | 64 desktop, 72 mobile | Yes, with `--settle-ms 6000` | 8 to 10 s |

What these sites showed, and what changed because of them:

- **Lists of different links were merged.** A footer, a mobile menu and a
  contact list each became one component named `*`. The rule now templates
  only cards: items with at least two controls in a shared arrangement.
- **Sections lost their names.** With three or more labelled sections side by
  side, links in different sections got the same path. Landmarks now keep
  their names.
- **Crops were blank.** One full-page screenshot was taken while an intro was
  still playing, and it could never show scroll-driven sections. The engine now
  waits for ending animations and goes through the page a screen at a time,
  cropping each component from the screen that showed it.
- **Three unnamed buttons became one component.** Previous, play and next had
  no names and sat side by side. Unnamed elements beside each other are now
  separate components.
- **A screenshot blocked on a web font** and failed the whole browser. The
  driver no longer waits for fonts, and a screenshot that fails costs the
  pictures, not the run.
- **A timed intro gave different results in different browsers.** Nothing on
  the page shows that more is coming, so this cannot be detected in general.
  There is now a `--settle-ms` option, and a `browsers_disagree` warning when
  browsers find different numbers of elements.

Limits these sites showed that are still there:

- **Covered elements.** Fixed for elements on screen when the page is read: they
  are marked `covered`, with what covers them. An element further down the page
  is not tested, because only what is on screen can be.
- **Names that change.** Fixed for names that change while the page is read,
  such as a countdown: they are left out of the id. A name that changes only
  between visits still changes the id.
- **Different pages at different sizes.** One site builds a different page for
  mobile, so desktop and mobile share only some ids. That is correct, but it
  means ids are comparable across browsers, not always across viewports.
- **Time.** Long pages take up to a minute per browser and viewport.

### Speed

Measured on one machine (Apple silicon, 4 browsers at a time), three browsers at
two viewports. The default is in parallel without pictures. "One at a time" is
`--parallel 1`.

| Page | Pictures, one at a time | Pictures, in parallel | Default: no pictures, in parallel |
|---|---|---|---|
| Demo shop | 24 s | 8 s | 7 s |
| `www.avqentra.in` | 94 s | 34 s | 26 s |
| `t3works.vercel.app` | not measured | 53 s | 34 s |
| `portfolio-theta-lyart-35.vercel.app` | about 250 s, on an earlier build | 86 s | 27 s |
| `prabhav.avqentra.in`, with `--settle-ms 6000` | not measured | 22 s | not measured |

Components and ids were the same in every mode. `--timings` shows that on a long
page most of the time is scrolling and waiting after each scroll, and on a short
one most of it is the page load.

One parallel run of `prabhav.avqentra.in` had page loads of 24 seconds and one
timeout, where a later run had loads of 8 seconds. That led to the retry for
targets that time out beside others.

### Not done or not verified

- **Safari window size.** On the Mac Safari was tested on, Safari ignored every
  request to resize its automation window and stayed at the screen's size,
  1710x1021. Resizing with a position, after maximizing or minimizing, and in a
  new window made no difference, and one session out of several did resize. It
  is a macOS window state that a driver cannot change. The page is then read and
  recorded at the size it really has, with a `viewport_not_reached` warning, so
  a Safari "390x844" run on such a Mac is a desktop-sized run and says so.
- **Kitesurf.** `--browser cdp=ENDPOINT` works against a local Chromium. It has
  not been pointed at Kitesurf, which needs an endpoint from Cloudflare.
- **Real sites.** Four have been run (see above), all simple public pages built
  by the same few people. None had a login, a large form or a real shop grid.
- **Windows and Linux.** Proven by CI, which passes on all three systems. Its
  first run found three problems, all fixed: a lint that newer Rust added,
  Chromium's sandbox on Ubuntu 24.04, and finding chromedriver on Windows. A
  second run found a test that stopped scrolling too early on a slow machine.
- **Fingerprint freeze.** The recipe is stable on the shop and on four real
  sites, after one change. See `spec/fingerprint.md`.
- **`page.act`.** Reserved in the driver protocol. No driver implements it.
- **Slow content at the end of a page.** Fixed: at the bottom of a page that
  scrolls, the engine waits a second and looks again, and goes on while the page
  grows. Content that takes longer than a second after each bottom is missed.
- **Parallel runs on heavy pages.** Several browsers rendering a heavy page at
  once can make its loads slow. A browser that times out is retried alone, but
  one that is merely slow is not.
- **Virtualised lists.** Detected but not yet read in full. A list that keeps
  only the rows near the screen is reported with `elements_removed_while_scrolling`,
  and only the rows present at the end are listed. Reading every screen and
  merging the readings needs care, because such lists reuse page elements for
  other rows and fixed headers appear on every screen. Rows whose controls all
  have the same text, such as a column of "Open" buttons alone, are not told
  apart, so such a list is not detected.

## Multi-language support

| Piece | Status |
|---|---|
| TypeScript types generated from `spec/` | Done, with a check in CI that they are up to date. |
| TypeScript SDK, `sdks/typescript` | Done. Tested against the real engine and the Buggy Shop: one run, several runs on one engine, two runs at once, refusals, cancelling by a throwing handler and by an `AbortSignal`. |
| Recorded conversations, `spec/transcripts/` | Done: a completed run, a refused run, a cancelled run, a version mismatch. Recorded from the real engine by `scripts/record_transcripts.py`, except the version mismatch. |
| Stand-in engine, `crates/traceqa-replay` | Done. |
| Both SDKs replay every transcript | Done. They send identical messages. |
| Guide for other languages, `docs/clients.md` | Done. |
| CI | The TypeScript checks are in the Lint job and the tests in the Test job on all three systems. Passing on all three. |
| Publishing to npm | Not done. Needs an npm account. |

On Windows the TypeScript tests depend on `npm` and the workspace's `python`
being found on `PATH`, which CI sets up.

## MCP server and setup

| Piece | Status |
|---|---|
| `traceqa mcp`, crate `crates/traceqa-mcp` | Done. Speaks the four MCP versions that use the `initialize` handshake (2024-11-05 to 2025-11-25). Clients that first try the newer `server/discover` handshake of MCP 2026-07-28 fall back to it, which the tests check. |
| Tools | `extract` with progress and cancelling, `list_runs`, `get_components` with filters, `get_crop` returning the picture. |
| `traceqa install` | Done. Creates `~/.traceqa/env` with uv, installs the drivers from GitHub or from a checkout (pinned to its `uv.lock`), downloads browsers. |
| `traceqa doctor` | Done. Shows where each driver was found and which browsers it opens. |
| Engine finds installed drivers | Done. `PATH` first, then `~/.traceqa/env`. |
| Tests | Rust unit tests for every MCP message. End to end, with the official MCP client library and a real browser, and from an empty folder through install to an extraction. |

Not done:

- **uv is not installed for the user.** `install` says how to install it. Fetching
  and running an installer script for the user was left out on purpose.
- **Installing from GitHub is not tested in CI.** The tests install from the
  checkout, which is what they can prove. The GitHub path is the same command with
  a different source.
- **MCP 2026-07-28.** Its `server/discover` handshake is not spoken yet. Clients fall back to `initialize`.
- **The MCP server has not been tried in Claude Code by hand.** The tests use the
  official MCP client library for Python, which speaks the same protocol, but a
  manual check in a real AI tool is still worth doing.

## CI speed and releases

| Change | Why |
|---|---|
| Pull requests build and test in debug only | Rust was compiled twice per job, once for the tests and once in release. Debug also keeps overflow and sanity checks on while testing. |
| Release binaries come from `release.yml`, on a version tag | Downloads get a version number and a permanent home on the Releases page, instead of 90-day CI artifacts. |
| The bench runs once in CI | CI checks that every component is found. Speed is measured locally, where three repeats mean something. |
| Documentation-only changes skip Lint and Test | A final `CI` job always reports, so it works as a required check. |
| `scripts/check_version.py` | Every package must carry the same version, and a release tag must match it. |
| `traceqa install` defaults to the binary's own tag | A released binary always gets the drivers of its own version. |

Browser caching was considered and left out: Playwright advises against it,
because restoring the cache takes about as long as downloading.

Not done: the MCP server, the installer and these CI changes have passed on
macOS but not yet in CI on Windows and Linux. The release workflow runs for the
first time when `v0.1.0` is tagged, and installing drivers from GitHub needs
that tag to exist.

## Safari

Tested on Safari 26.6 on macOS, through `safaridriver` and the WebDriver driver.

| Check | Result |
|---|---|
| The driver's own test (frames, shadow DOM, lazy content) | Passes |
| The demo shop, beside Chromium, Firefox and WebKit at two viewports | 37 of 37 in every browser. Component ids, names and instance counts identical in all four. |
| The collector case pages | All pass, as in the other browsers |
| Window size | Not reached on this Mac. See "Safari window size" above. |

The `viewport_not_reached` warning came out of this: before it, a run that could
not reach its viewport recorded the size asked for. The driver now reports the
size it reached, and the engine reads, scrolls, crops and records at that size.

Safari cannot run on GitHub's machines, which do not allow remote automation.
It is checked by hand before a release:

```sh
TRACEQA_TEST_SAFARI=1 uv run pytest drivers/webdriver-py collector
```

## Test plan

| Phase | Tests | Status |
|---|---|---|
| 1 | Every run folder a test produces is checked against `spec/` and against rules a schema cannot state (event order, ids, counts, crops). The checker has its own tests that break a folder nine ways. | Done |
| 1 | Collector case pages: about 95 cases on 6 pages, each checked in Chromium, Firefox and WebKit, plus a check that nothing unexpected is listed. | Done |
| 2 | Failure cases against `fixtures/trouble/`, a site that fails on purpose: 404, 500, a redirect, a load that never finishes, an endless page, plain text, an address nothing answers, a driver that crashes, and Ctrl-C. | Done |
| 2 | Command-line tests: every exit code, `--json` equals `run.json`, refusals with reasons, help for every command. | Done |
| 2 | Golden ids: the 37 ids of the Buggy Shop are saved in `fixtures/buggy-shop/golden-ids.json` and every run must reproduce them. | Done |
| 3 | Real-world pattern pages | Planned |
| 4 | Nightly: language versions, comparison with the browser's accessibility tree | Planned |

What phase 1 found:

- **Content of a closed `<details>` counted as visible**, in all three browsers.
  Browsers hide it with `content-visibility`, which size and `visibility` do not
  show. The collector now asks the browser with `checkVisibility()`, which also
  makes `opacity: 0` count as hidden.

What phase 2 found:

- **HTTP errors passed silently.** A 404 or 500 page completed with no sign of
  it. It is still read, and now raises `http_error_status`.
- **One stuck image failed the whole page.** A page whose load event never fired
  failed with a timeout, though its content was there. The drivers now wait for
  the content first, then for the load event in the time left, and a page that
  never finishes is read as it stands, with `page_load_incomplete`.
- **Ctrl-C left the run saying `running`,** and the target under way `failed`,
  because the terminal also stopped the drivers. Drivers now ignore Ctrl-C and
  the engine closes them. The run ends `cancelled`, with exit code 130, and no
  process is left behind.
- **Errors carried Playwright's call log,** several lines of it. Only the first
  line is kept.

## Accuracy fixes

| Fix | What changed | Tested by |
|---|---|---|
| `role="presentation"` on a focusable element | ARIA ignores it, so the element keeps its role: a button stays a button. | Case pages, 4 browsers |
| Visually hidden elements | One pixel or less, `clip: rect(0 0 0 0)` and `clip-path: inset(50%)` count as hidden, as skip links are. | Case pages, 4 browsers |
| Covered elements | Five points of each element on screen are hit-tested. Under something at all five, it is `covered`, with `coveredBy`, and one `covered_elements` warning names the first. An overlay that lets clicks through does not cover, nor does the element's own content or label. | Case pages, 4 browsers; a cookie banner end to end |
| Content that arrives after the bottom is reached | The engine waits a second at the bottom of a page that scrolls, and goes on while it grows. | End to end, in both modes |
| Closed shadow roots | Counted in Chromium, the only browser that lets a driver see them, and reported with `closed_shadow_roots`. No browser lets a script read inside one. | End to end, Chromium and Firefox |
| Lists that remove rows | Detected and reported with `elements_removed_while_scrolling`. | End to end, in both modes, and the shop raises no false warning |
| Names that change, such as a countdown | The page is read before and after the walk, at least a second apart. A name that differs at the same place is left out of the id, and the component has `nameChanges`. Other ids are unchanged. | A countdown keeps its id across runs, in both modes; a unit test; none of the four real sites has a false alarm, and each finds the same components as before |

The case pages have no known gaps left.

## What comes next

1. Merge, then tag `v0.1.0`, the first release. See [releasing.md](releasing.md).
2. Run extraction on sites of other kinds (a shop, a form-heavy app, a login page) before freezing the fingerprint.
3. Explorer: click components and follow links, calling extraction on each new
   state. This is the first feature that needs an AI key.
4. Oracles, check mode, sweep mode and reports.
5. A GitHub Action, and the SDKs on PyPI and npm.

## Layout

```
spec/                      contracts (source of truth)
collector/collector.js     the in-page script
crates/
  traceqa-protocol/        generated types and the JSON-RPC line format
  traceqa-engine/          all logic (library)
  traceqa-recorder/        run folder, events, crops
  traceqa-cli/             the traceqa command, install and doctor
  traceqa-mcp/             the MCP server
  traceqa-replay/          stand-in engine for testing clients
python/
  traceqa-protocol/        generated Python types, the driver request loop
drivers/
  playwright-py/           Chromium, Edge, Firefox, WebKit, CDP endpoints
  webdriver-py/            real Safari, any WebDriver server or grid
sdks/python/               the traceqa Python package
sdks/typescript/           the traceqa TypeScript package
fixtures/buggy-shop/       demo site and its manifest
bench/                     scoring
tests/                     end-to-end tests of mcp and install
scripts/                   code generation, transcript recording
```

Added by later features: `crates/traceqa-models/`, `crates/traceqa-server/`,
`apps/dashboard/`.
