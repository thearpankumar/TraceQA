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
| `transcripts/` | Recorded client conversations | Every client is tested against them. See `transcripts/README.md`. |

## How messages travel

Both protocols are JSON-RPC 2.0, one message per line.

- **Client protocol:** the client starts the engine (`traceqa serve`) and writes
  requests to its stdin. The engine writes responses and notifications to its
  stdout.
- **Driver protocol:** the engine starts the driver and is always the caller.
  The driver only sends responses and `driver.log` notifications.

Stderr is free for human-readable logs on both sides.

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
