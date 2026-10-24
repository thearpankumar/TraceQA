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
| `common.schema.json` | Shared types | Used by the files above |
| `fingerprint.md` | Component id recipe | The engine and anything that compares runs |
| `fingerprint-vectors.json` | Shared test vectors for the recipe | The engine's unit tests and the collector test (`tests/test_fingerprint_vectors.py`) |
| `transcripts/` | Recorded client conversations | Every client is tested against them. See `transcripts/README.md`. |

Every `$ref` must be `#/$defs/Name` or `file.schema.json#/$defs/Name`, and each `$defs` name
must be unique across files: the three generators flatten refs to the last segment.
`scripts/check_spec_refs.py` enforces this in CI.

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

## Run folder layout

```
runs/<runId>/
  run.json                  RunManifest
  events.jsonl              one Event per line
  <browser>-<width>x<height>/
    components.json         ComponentsFile
    page.png                screenshot of the page    (only with pictures)
    crops/<componentId>.png one crop per component    (only with pictures)
```

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
(cd sdks/typescript && npm run gen)      # rewrite the TypeScript types
uv run scripts/record_transcripts.py     # after a client protocol change
uv run pytest
```
