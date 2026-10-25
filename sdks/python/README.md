# traceqa (Python)

Python client for [TraceQA](https://github.com/thearpankumar/TraceQA). It starts
the `traceqa` engine and talks to it; all logic lives in the engine.

```python
import traceqa

result = traceqa.extract("https://example.com", browsers=["chromium", "firefox"])
```

This package is built from the repository and is not published to PyPI yet. To
use it, work from a checkout (`uv sync`). It needs the `traceqa` engine binary
and a browser driver; see [docs/usage.md](https://github.com/thearpankumar/TraceQA/blob/master/docs/usage.md#python-sdk)
for setup, options and errors.

The SDK prints nothing to your stderr; pass `on_log=` to `traceqa.extract` or
`traceqa.Engine` to see the engine's log lines. If the engine dies,
`EngineExitedError.message` ends with its last stderr lines, also available as
`error.stderr_tail`.

After a Ctrl-C, SIGTERM or SIGHUP the engine cancels its runs and exits with code
130; a run in flight then ends `cancelled`.

Licensed under Apache-2.0.
