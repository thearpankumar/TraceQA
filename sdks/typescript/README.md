# traceqa (TypeScript)

TypeScript client for TraceQA. It starts the `traceqa` engine and talks to it;
all logic lives in the engine. Node 22 or later, no runtime dependencies.

```ts
import { extract } from "traceqa";

const result = await extract("https://example.com", {
  browsers: ["chromium", "firefox"],
  viewports: [
    [1280, 800],
    [390, 844],
  ],
  onEvent: (event) => console.log(event.message),
});

for (const target of result.targets) {
  for (const component of (await result.components(target.dir)).components) {
    console.log(component.id, component.role, component.name, component.instances.length);
  }
}
```

Compare two runs and run the automated checks, on one engine, without a browser:

```ts
const engine = await Engine.start();
const before = await engine.extract("https://example.com");
const after = await engine.extract("https://example.com");
const diff = await engine.diff(before, after); // a DiffFile; `added` means only in `after`
const found = await engine.findings(after, { suppressions: "suppress.yaml" }); // a FindingsResult
await engine.close();
```

`diff(a, b, { targetA, targetB })` takes run folders (paths or the results of
`extract`); name a target (`chromium-1280x800`) for a run with several. A wrong folder
or target rejects with an `EngineRefusedError` (kind `invalid_params`) whose message
says what to fix. Answers are checked at run time, like `components.json`. `findings`
returns the checks, not a conformance claim.

All options are in [docs/usage.md](../../docs/usage.md#typescript-sdk).

## Logs

The engine and its drivers log to their standard error (`[driver info] opened
chromium ...`). The SDK reads that and drops it, so a library never writes to
your stderr. Pass `onLog` to `extract` or `Engine.start` to see the lines:

```ts
await extract(url, { onLog: (line) => console.error(line) });
```

If the engine dies, `EngineExitedError.message` ends with its last stderr lines,
also available as `error.stderrTail`.

Errors are `TraceQAError` and its subclasses `EngineStartError`,
`EngineExitedError`, `EngineRefusedError` and `EngineProtocolError`; the original
error is kept in `cause`. `engine.close({ timeoutMs })` kills an engine that does
not exit in time. After a Ctrl-C, SIGTERM or SIGHUP the engine cancels its runs and
exits with code 130; a run in flight then ends `cancelled`.

## ESM only

The package is ESM only (`"type": "module"`, no CommonJS build). From ESM, `import`
it as above. From CommonJS, use a dynamic import:

```js
const { extract } = await import("traceqa");
```

On Node 22.12 or later, `require("traceqa")` also works, because Node can load
ES modules with `require`.

## Development

```sh
npm ci
npm run gen          # regenerate src/protocol.ts from spec/
npm run gen:check    # fail if src/protocol.ts is out of date
npm run typecheck
npm test             # after `uv sync` and `cargo build` at the repository root
```

The tests need the workspace's Python and the browser drivers: they start the
Buggy Shop and run the real engine. They find both in the repository's `.venv`
(`uv sync` makes it) without `uv run` or a `PATH` edit; `TRACEQA_PYTHON` names
another Python. A first test file checks this and fails within seconds with
"Run `uv sync` first" instead of hanging. They also
play back `spec/transcripts/` against the stand-in engine, `traceqa-replay`.
