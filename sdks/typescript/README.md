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

All options are in [docs/usage.md](../../docs/usage.md#typescript-sdk).

Errors are `TraceQAError` and its subclasses `EngineStartError`,
`EngineExitedError`, `EngineRefusedError` and `EngineProtocolError`; the original
error is kept in `cause`. `engine.close({ timeoutMs })` kills an engine that does
not exit in time.

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
uv run npm test      # from the repository root's environment, after cargo build
```

`uv run` puts the browser drivers and the workspace's Python on `PATH`. The
tests need both: they start the Buggy Shop and run the real engine. They also
play back `spec/transcripts/` against the stand-in engine, `traceqa-replay`.
