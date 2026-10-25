# Writing a client in another language

TraceQA has clients for Python (`sdks/python`) and TypeScript
(`sdks/typescript`). A client for any other language is a small amount of
plumbing, because the engine does all the work. This guide says what that
plumbing must do and how to prove it does.

## What a client is

A client starts the engine as a child process and exchanges JSON-RPC 2.0
messages with it, one per line, over the engine's standard input and output.
It holds no logic: it builds requests, hands events to the caller and returns
results. Any language that can start a process and read and write lines of text
can be a client.

## The conversation

| Step | Who | Line |
|---|---|---|
| 1 | Client | Start `traceqa serve`. Add `--driver COMMAND` for each driver the caller names. |
| 2 | Client | `{"jsonrpc":"2.0","id":1,"method":"engine.hello","params":{"protocolVersion":1,"clientName":"traceqa-go"}}` |
| 3 | Engine | `{"jsonrpc":"2.0","id":1,"result":{"engineVersion":"0.1.0","protocolVersion":1,"features":["extract","diff","findings"]}}` |
| 4 | Client | `{"jsonrpc":"2.0","id":2,"method":"run.start","params":{"feature":"extract","url":"https://example.com","targets":[{"browser":"chromium","headless":true}],"viewports":[{"width":1280,"height":800}],"options":{"crops":false,"includeHidden":true}}}` |
| 5 | Engine | `{"jsonrpc":"2.0","id":2,"result":{"runId":"r_20261014_065509_925a","runFolder":"/abs/path/runs/r_20261014_065509_925a"}}` |
| 6 | Engine | `{"jsonrpc":"2.0","method":"run.event","params":{"runId":"r_...","event":{...}}}`, many times |
| 7 | Engine | `{"jsonrpc":"2.0","method":"run.done","params":{"runId":"r_...","status":"completed","manifest":{...}}}`, once |
| 8 | Client | `{"jsonrpc":"2.0","id":4,"method":"engine.shutdown","params":{}}`. The engine answers `{}`, starts no more runs and lets the ones going finish. An engine from before this method answers `-32601`; ignore that. |
| 9 | Client | Close the engine's standard input. The engine exits once its runs are done. If the input ends without step 8, the engine assumes the client crashed: it cancels every run, finalises the run folders and exits. |

To cancel, send `{"jsonrpc":"2.0","id":3,"method":"run.cancel","params":{"runId":"r_..."}}`.
The run still ends with `run.done`, with status `cancelled`.

Two more requests read local files and answer at once, with no run and no events.
Send them after step 3, when `features` lists `diff` or `findings` (an older engine
answers `-32601`; report that the engine is too old):

- `{"jsonrpc":"2.0","id":5,"method":"diff.compare","params":{"a":"/abs/runs/r_1","b":"/abs/runs/r_2","targetA":"chromium-1280x800","targetB":"chromium-1280x800"}}`
  answers with a `DiffFile`. `targetA` and `targetB` may be left out for a run with one target.
- `{"jsonrpc":"2.0","id":6,"method":"findings.analyse","params":{"run":"/abs/runs/r_1","target":"chromium-1280x800","suppressions":"/abs/suppress.yaml"}}`
  answers with `{"files":[FindingsFile]}`. `target` and `suppressions` may be left out.

A folder that is not a run, a target the run lacks, an invalid suppression file or a
`components.json` over 64 MiB is refused with kind `invalid_params`; the message says
what to fix. Do not send either while you wait for a `run.start` answer on the same
connection unless you match answers by `id`, as the rules below say.

A full recorded example of each case is in [spec/transcripts/](../spec/transcripts/).

## Rules

| Rule | Why |
|---|---|
| Write one message per line, as UTF-8, and flush after each. | The engine reads line by line. |
| Match answers to requests by `id`. Match `run.event` and `run.done` to runs by `runId`. | Several runs can go at once on one engine. |
| Read the answer to `run.start` before handling any line after it. | The engine never sends a run's events before the answer that names the run, so a client that registers the run when it reads the answer misses nothing. |
| An answer with `error` is a refusal. Report `error.message`, and `error.data.kind` as a code. | Kinds such as `invalid_params` and `version_mismatch` let the caller react. |
| A browser that fails is not an error. | It shows in the target's `status` and `error` in the manifest. |
| Ignore fields and messages you do not know. | Adding an optional field does not change the protocol version. |
| Leave out options the caller did not set. Do not send `null`. | The engine applies its own defaults. |
| Leave the engine's standard error alone, or pass it through. | It holds logs for people. Only standard output is protocol. |
| When the caller is done, cancel runs still going, close standard input and wait for the process to exit. | No browser or run folder is left half finished. |
| A missing engine gets a clear error. Look for it in the caller's setting, then `TRACEQA_BIN`, then `traceqa` on `PATH`. | Same order as the other clients. |

## Types

The messages are defined by the JSON Schemas in [spec/](../spec/README.md). Do
not write the types by hand: generate them, so they cannot drift from the spec.

| Language | Generator used |
|---|---|
| Rust | `typify`, in `crates/traceqa-protocol/build.rs` |
| Python | `datamodel-code-generator`, in `scripts/gen_python_protocol.py` |
| TypeScript | `json-schema-to-typescript`, in `sdks/typescript/scripts/gen-protocol.mjs` |

For another language, pick a JSON Schema generator for it and follow the same
pattern: every `spec/*.schema.json` file adds its `$defs` to one shared
namespace, and a `--check` mode fails when the generated file is out of date.
Two kinds of field are open and must stay open: an object with no listed
properties (any keys), and a property with no type (any JSON value).

## Shape of the API

Keep the same shape as the other clients, in the language's own style, so a
reader of one can use another:

| Python | TypeScript | Purpose |
|---|---|---|
| `traceqa.extract(url, **options)` | `extract(url, options)` | Start an engine, run once, stop the engine. |
| `traceqa.Engine()` as a context manager | `Engine.start()`, then `close()` | Several runs on one engine. |
| `on_event=` | `onEvent:` | Called with each event, live and in order. |
| `on_log=` | `onLog:` | Called with each log line of the engine and drivers. Off by default: the SDK prints nothing. |
| `result.components(target_dir)` | `result.components(targetDir)` | Read one target's `components.json`. |
| `TraceQAError` with `.kind` | `TraceQAError` with `.kind` | A refusal, or an engine that cannot be reached. |

## Testing a client

A client is finished when it passes both of these.

**1. The recorded conversations.** Build the stand-in engine with
`cargo build`, then for each file in `spec/transcripts/`:

1. Set `TRACEQA_REPLAY` to the file and `TRACEQA_REPLAY_REPORT` to a new path.
2. Make the calls the table in [spec/transcripts/README.md](../spec/transcripts/README.md#the-conversations)
   describes, with `target/debug/traceqa-replay` as the engine binary.
3. Close the engine and check that the report file says `ok`.

The stand-in checks every field of every message the client sends, so these
tests prove the client speaks the protocol exactly as the others do, without a
browser. `sdks/python/tests/test_transcripts.py` and
`sdks/typescript/test/transcripts.test.ts` are worked examples.

**2. The real engine.** Run an extraction against the Buggy Shop
(`python fixtures/buggy-shop/serve.py --port 0` prints its address) and check
the result, as `sdks/python/tests/test_sdk.py` does. This proves the client
works end to end.

## Adding it to the repository

1. Put it in `sdks/<language>/`, with its generator and its tests.
2. Add its steps to `.github/workflows/ci.yml`: the generated-types check in the
   Lint job, and its tests in the Test job, which runs on Linux, macOS and
   Windows.
3. List it in the README and in `docs/usage.md`.
