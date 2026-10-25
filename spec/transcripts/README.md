# Transcripts

Recorded conversations between a client and the engine. Every client should be
tested by playing these back. See [docs/clients.md](../../docs/clients.md).

## Format

One JSON object per line:

| Line | Meaning |
|---|---|
| `{"note": "..."}` | What the conversation shows. Ignored when playing. |
| `{"client": message}` | A message the client must send next. |
| `{"engine": message}` | A message the engine sends. |

## Playing one back

`traceqa-replay` (built by `cargo build`) stands in for the engine. A client
starts it exactly as it would start `traceqa`, with `serve` as the argument,
and with two environment variables set:

| Variable | Meaning |
|---|---|
| `TRACEQA_REPLAY` | Path of the transcript. |
| `TRACEQA_REPLAY_REPORT` | A file to write the verdict to: `ok`, or what went wrong. |

The stand-in writes the engine lines up to the next client line, then reads one
line from the client and checks it, and so on. When the client closes the
engine's input, every client line must have been matched. A test makes its
calls, closes the engine, and checks that the report says `ok`.

## Matching rules

A message from the client matches when:

- Every field the transcript lists is present with the same value, except a
  field whose value is `"<any>"`: it may hold anything, or be left out.
- It has no field the transcript does not list.
- Numbers are equal as numbers, so `1` matches `1.0`. Key order does not matter.
- Request ids may be any number or string. The engine lines that answer a
  request are sent with the id the client used.

A message that does not match is answered with an error of kind
`transcript_mismatch` naming the field, so the client's call fails with the
reason.

## The conversations

| File | How the client produces it | What it must do |
|---|---|---|
| `extract.jsonl` | Extract `http://127.0.0.1:4173/` in chromium at 1280x800 and 390x844, with no scrolling, a 30000 ms timeout and 2 browsers at a time. Pass any `outDir`. | Deliver every event, in order, then return a completed result for the run folder named in the answer. |
| `refused.jsonl` | Extract `not-a-url` with default options. | Fail the call with kind `invalid_params`. |
| `diff.jsonl` | Extract the shop as for `extract.jsonl`, then call `diff.compare` with the run folder from the answer as both `a` and `b`, `targetA` `chromium-1280x800` and `targetB` `chromium-390x844`. Write the folder with forward slashes, as the transcript does. | Send the request exactly as recorded and return the `DiffFile` of the answer. |
| `findings.jsonl` | Extract the shop as for `extract.jsonl`, then call `findings.analyse` with the run folder as `run` and nothing else. | Send the request exactly as recorded and return the `FindingsResult` of the answer. |
| `cancel.jsonl` | Extract the shop at both viewports, 1 browser at a time, and cancel the run when the `page.opened` event arrives. | Send `run.cancel` for the run. |
| `version-mismatch.jsonl` | Start an engine. | Fail with kind `version_mismatch`. |

Every conversation that got past `engine.hello` ends with the client sending
`engine.shutdown` (the engine answers `{}`) before it closes the engine's input.

Default options mean pictures off (`crops: false`), hidden elements included
(`includeHidden: true`), and headless browsers. Leave out every option the
caller did not set.

## Re-recording

```
cargo build
uv run scripts/record_transcripts.py
```

The script plays each conversation against the real engine and the Buggy Shop.
Do this after changing the client protocol, and read the diff. `cargo test`
checks every message in these files against the schemas.
