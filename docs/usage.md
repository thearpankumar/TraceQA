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
| `--storage-state FILE` | none | Start every browser with the cookies and stored values in FILE, for pages behind a sign-in. See [Pages behind a sign-in](#pages-behind-a-sign-in). |
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
| `1` | At least one target failed. |
| `2` | The request was invalid, for example a malformed URL or viewport. Nothing was run. |
| `130` | The run was stopped with Ctrl-C. |

Ctrl-C stops the run at its next step, closes the browsers and writes a run
folder that says `cancelled`. Targets that had already finished keep their
results. A second Ctrl-C quits at once.

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

## `traceqa mcp`

```
traceqa mcp [--out DIR] [--state-dir DIR] [--driver COMMAND]...
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
traceqa install [BROWSERS]... [--from FOLDER] [--ref REF] [--with-deps]
```

Installs the browser drivers and browsers, so the binary is all a user needs
to download. Needs [uv](https://docs.astral.sh/uv/); without it, the command
says how to install it.

| Step | What happens |
|---|---|
| 1 | Creates a Python environment in `~/.traceqa/env` (or `$TRACEQA_HOME/env`), away from the user's other Python projects. |
| 2 | Installs the two drivers into it: from GitHub, or from a checkout with `--from`. From a checkout, the versions come from its `uv.lock`. |
| 3 | Downloads the browsers asked for. Default `chromium`. Known values: `chromium`, `chrome`, `msedge`, `firefox`, `webkit`, `safari` (nothing to download). |

| Option | Meaning |
|---|---|
| `--from FOLDER` | Install the drivers from a TraceQA checkout instead of GitHub. |
| `--ref REF` | Branch, tag or commit on GitHub. Default: the binary's own release tag, such as `v0.1.0`, so the binary and its drivers match. |
| `--with-deps` | Also install the system libraries the browsers need. Linux only, may ask for `sudo`. |

Running it again updates the drivers. The engine finds the installed drivers
by itself; see [Drivers](#drivers).

## `traceqa doctor`

Checks the setup and says what is missing: TraceQA's folder, uv, and for each
driver where it was found, whether it starts, and which browsers it opens.
Exits with 1 when no driver works.

```
TraceQA 0.1.0
Home:    /Users/me/.traceqa (drivers installed here)
uv:      /opt/homebrew/bin/uv

Drivers:
  /Users/me/.traceqa/env/bin/traceqa-driver-playwright
    installed: /Users/me/.traceqa/env/bin/traceqa-driver-playwright
    works: traceqa-driver-playwright 0.1.0, opens chromium, chrome, msedge, firefox, webkit, cdp
  ...

Ready. Browsers available: cdp, chrome, chromium, firefox, msedge, safari, webdriver, webkit
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
| `viewports` | `((1280, 800),)` | `(width, height)` pairs. |
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
`maxScrollSteps`, `timeoutMs`, `settleMs`, `parallel`, `storageState` and `onEvent`. Two differ:

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
| `http_error_status` | The page answered with an HTTP error, such as 404 or 500. It was still read, since an error page can be the thing under test. WebDriver browsers do not report the status, so they never raise this. |
| `page_load_incomplete` | The page's content arrived, but it had not finished loading when the time limit ran out, for example because one image never arrived. It was read as it stood. A page whose content never arrives fails instead. |
| `covered_elements` | Components on screen lie under something else, such as a cookie banner or a modal's backdrop, so a click would not reach them. Each such instance has `covered: true` and `coveredBy`. |
| `closed_shadow_roots` | The page has closed shadow roots, which no script can read inside, so what they hold is not listed. Only Chromium-based browsers can tell. |
| `elements_removed_while_scrolling` | The page removed controls it had shown as it was scrolled, as long tables and feeds that keep only the rows near the screen do. The rows that had left the screen are not listed. |
| `session_not_accepted` | The run was given a state, but the page is still a sign-in page by its address or a password field. The session has probably expired. |
| `signed_in_pictures` | The run used a stored session and `--crops`, so `page.png` and `crops/` may show personal data. Raised once per target; nothing else about what is saved changes. |
| `collector_tampered` | The page replaced built-in functions such as `Array.prototype.map` or `getBoundingClientRect`, which can make a script read the page wrongly. The warning names them. With Playwright (Chromium, Firefox, WebKit) the driver captured the browser's own functions before the page's scripts ran and the page was read with those, so the result is right and the warning is for information. With a WebDriver browser (Safari, or any browser behind a WebDriver server) there is no way to run a script first, so the collector uses what it finds, spots functions that no longer look like native code, and says the result may be wrong. This check is best effort: a page can also fake how a function prints itself, and a few reads that are not captured (such as the `checked` state of an input) are not defended. |
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
