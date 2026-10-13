# spec

The contracts every part of TraceQA agrees on. These JSON Schemas are the
single source of truth: the Rust and Python types are generated from them, so
the two sides cannot drift apart.

| File | Contract | Between |
|---|---|---|
| `client-protocol.schema.json` | Client protocol | A client (SDK, MCP server, hosted worker) and the engine |
| `driver-protocol.schema.json` | Driver protocol | The engine and a browser driver |
| `collector.schema.json` | Collector output | The in-page script and the engine |
| `run-folder.schema.json` | Run folder | The engine and anything that reads results |
| `common.schema.json` | Shared types | Used by the files above |
| `fingerprint.md` | Component id recipe | The engine and anything that compares runs |

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

## Run folder layout

```
runs/<runId>/
  run.json                  RunManifest
  events.jsonl              one Event per line
  <browser>-<width>x<height>/
    components.json         ComponentsFile
    page.png                full-page screenshot
    crops/<componentId>.png one crop per component
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
uv run pytest
```
