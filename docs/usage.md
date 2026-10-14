# Usage

## Setup

Requirements: Rust (stable) and [uv](https://docs.astral.sh/uv/).

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

Inside the repository, `uv run` puts both drivers on `PATH`:

```sh
uv run cargo run -- extract https://example.com
```

## `traceqa extract`

```
traceqa extract [OPTIONS] <URL>
```

| Option | Default | Meaning |
|---|---|---|
| `--browser LIST` | `chromium` | Browsers to run in, comma separated. See [Browser values](#browser-values). |
| `--viewport LIST` | `1280x800` | Viewport sizes as `WIDTHxHEIGHT`, comma separated. |
| `--out DIR` | `runs` | Where the run folder is created. |
| `--headed` | off | Show the browser windows. |
| `--crops` | off | Also save pictures: a screenshot of the page and one crop per component. Slower on long pages: see [Speed](#speed). |
| `--visible-only` | off | Leave out elements that are not visible. |
| `--max-scroll-steps N` | `50` | How many screens to scroll through on a long page. `0` reads the page without scrolling. |
| `--timeout-ms MS` | `30000` | Time allowed for a page to load. |
| `--settle-ms MS` | `0` | Extra time to wait after the page has loaded, before reading it. For pages with a timed intro or splash screen. |
| `--parallel N` | chosen from the machine | How many browsers run at the same time. `1` runs them one after another. The default is half the machine's cores, at most 4. |
| `--timings` | off | Also print where each browser's time went. |
| `--json` | off | Print the run manifest as JSON instead of a table. |
| `--driver COMMAND` | see above | Command that starts a browser driver. |

Every browser is run at every viewport, several at a time. Progress is written
to standard error, each line marked with the browser it is about, and the
result to standard output.

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
| `1` | At least one target failed or the run was cancelled. |
| `2` | The request was invalid, for example a malformed URL or viewport. |

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

## `traceqa serve`

```
traceqa serve [--driver COMMAND]...
```

Speaks the client protocol on standard input and output. SDKs start the engine
this way. See [architecture.md](architecture.md#client-protocol).

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
| `viewports` | `((1280, 800),)` | `(width, height)` pairs. |
| `out_dir` | `runs` | Where the run folder is created. |
| `headless` | `True` | Hide the browser windows. |
| `crops` | `False` | Also save a screenshot of the page and a picture of each component. |
| `include_hidden` | `True` | Include elements that are not visible. |
| `max_scroll_steps` | `50` | How many screens to scroll through. `0` reads the page without scrolling. |
| `timeout_ms` | `30000` | Time allowed for a page to load. |
| `settle_ms` | `0` | Extra time to wait after the page has loaded. For pages with a timed intro. |
| `parallel` | chosen from the machine | How many browsers run at the same time. |
| `on_event` | `None` | Called with each progress event as it happens. |

`traceqa.extract` and `traceqa.Engine` also accept `binary`, the path of the
engine, and `drivers`, a list of driver commands. Without `binary`, the SDK uses
the `TRACEQA_BIN` environment variable and then `traceqa` on `PATH`.

### Result

| Member | Meaning |
|---|---|
| `result.status` | `completed`, `failed` or `cancelled`. |
| `result.targets` | One summary per browser and viewport: folder name, status, counts, duration, error. |
| `result.components(target_dir)` | The components found for one target. The argument may be omitted when the run has one target. |
| `result.run_folder` | Path of the run folder. |
| `result.manifest` | The full run manifest. |

A browser that fails does not raise an exception. Inspect `result.status` and
each target's `status` and `error`. `TraceQAError` is raised when the engine
cannot be started or refuses the request.

## TypeScript SDK

Node 18 or later. The package has no runtime dependencies.

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

The options are the Python ones in camel case: `browsers`, `viewports`
(`[width, height]` pairs), `outDir`, `headless`, `crops`, `includeHidden`,
`maxScrollSteps`, `timeoutMs`, `settleMs`, `parallel` and `onEvent`. Two differ:

| Option | Meaning |
|---|---|
| `signal` | An `AbortSignal`. Aborting it cancels the run, and the result resolves with status `cancelled`. |
| `onEvent` | As in Python. If it throws, the run is cancelled and `extract` rejects with that error. |

`extract` and `Engine.start` also accept `binary` and `drivers`, found the same
way as in Python. The result has `status`, `targets`, `manifest`, `runFolder`
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
| `path` | The structural path to the component, for example `main > list "Products" > listitem`. |
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
