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
| Any other language | The protocol is built (`traceqa serve`). A caller writes its own plumbing from `spec/`. |
| TypeScript SDK | Not started. |
| CI action | Not started. The command line already works in CI through its exit code and `--json`. |
| MCP server | Not started. |
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
| 6 | Bench and CI | Bench done. The CI workflow covers Linux, macOS and Windows and builds the release binary on each. It has not run on GitHub yet. |
| 7 | Python SDK | Done |
| 8 | Safari and Kitesurf | Partly. See below. |

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

- **Covered elements.** An element behind another one (controls inside a closed
  book cover on one site) counts as visible, and its crop shows what covers it.
- **Names that change.** A low-confidence element takes its name from its text.
  When that text changes over time, so does the id.
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

- **Real Safari.** The WebDriver driver is written and passes its tests against
  chromedriver, which speaks the same protocol. It has not driven Safari: that
  needs "Allow remote automation" switched on in Safari's Developer settings.
  Then run `TRACEQA_TEST_SAFARI=1 uv run pytest drivers/webdriver-py`.
- **Kitesurf.** `--browser cdp=ENDPOINT` works against a local Chromium. It has
  not been pointed at Kitesurf, which needs an endpoint from Cloudflare.
- **Real sites.** Four have been run (see above), all simple public pages built
  by the same few people. None had a login, a large form or a real shop grid.
- **Windows and Linux.** Everything has only been run on macOS. The Windows
  problems found by reading the code are fixed (driver commands with spaces in
  the path, the run folder path format, the bench's binary name, line endings),
  and the CI workflow runs the tests and the bench on all three systems. Until
  that workflow has run, none of this is proven.
- **CI.** The workflow has never run on GitHub. A first run usually needs fixes.
- **Cancelling from the command line.** Ctrl-C stops the engine at once. Drivers
  close their browsers, but `run.json` is left saying `running`.
- **Viewport reached.** When a WebDriver browser cannot reach the requested
  viewport, the driver logs it, but `components.json` records the size asked for.
- **Setup command.** Installing the drivers and browsers is still manual (see the
  README). A `traceqa install` command does not exist.
- **Fingerprint freeze.** The recipe is stable on the shop and on four real
  sites, after one change. See `spec/fingerprint.md`.
- **`page.act`.** Reserved in the driver protocol. No driver implements it.
- **Slow content at the end of a page.** After each scroll the engine waits for
  the page height to stop changing. Content that takes longer than that to
  arrive when the bottom is reached is missed.
- **Parallel runs on heavy pages.** Several browsers rendering a heavy page at
  once can make its loads slow. A browser that times out is retried alone, but
  one that is merely slow is not.
- **Virtualised lists.** A list that removes rows as you scroll is read as it
  stands after the last scroll.

## What comes next

1. Run extraction on sites of other kinds (a shop, a form-heavy app, a login page) before freezing the fingerprint.
2. Open the pull request, let CI run on macOS, Windows and Linux for the first time, and fix what it finds.
3. Explorer: click components and follow links, calling extraction on each new
   state. This is the first feature that needs an AI key.
4. Oracles, check mode, sweep mode and reports.

## Layout

```
spec/                      contracts (source of truth)
collector/collector.js     the in-page script
crates/
  traceqa-protocol/        generated types and the JSON-RPC line format
  traceqa-engine/          all logic (library)
  traceqa-recorder/        run folder, events, crops
  traceqa-cli/             the traceqa command
python/
  traceqa-protocol/        generated Python types, the driver request loop
drivers/
  playwright-py/           Chromium, Edge, Firefox, WebKit, CDP endpoints
  webdriver-py/            real Safari, any WebDriver server or grid
sdks/python/               the traceqa Python package
fixtures/buggy-shop/       demo site and its manifest
bench/                     scoring
scripts/                   code generation
```

Added by later features: `crates/traceqa-models/`, `crates/traceqa-server/`,
`crates/traceqa-mcp/`, `apps/dashboard/`.
