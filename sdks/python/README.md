# traceqa (Python)

Python client for [TraceQA](https://github.com/thearpankumar/TraceQA). It starts
the `traceqa` engine and talks to it; all logic lives in the engine.

```python
import traceqa

result = traceqa.extract("https://example.com", browsers=["chromium", "firefox"])
```

Compare two runs and run the automated checks, on one engine, without a browser:

```python
with traceqa.Engine() as engine:
    before = engine.extract("https://example.com")
    after = engine.extract("https://example.com")
    diff = engine.diff(before, after)  # a DiffFile; `added` means only in `after`
    found = engine.findings(after, suppressions="suppress.yaml")  # a FindingsResult
```

`diff(a, b, target_a=None, target_b=None)` takes run folders (paths or the results
of `extract`); name a target (`chromium-1280x800`) for a run with several. A wrong
folder or target raises `EngineRefusedError` (kind `invalid_params`) with a message
that says what to fix. `findings(run, target=None, suppressions=None)` returns the
checks, not a conformance claim.

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
