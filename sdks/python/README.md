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

After a Ctrl-C, SIGTERM or SIGHUP the engine cancels its runs and exits with code
130; a run in flight then ends `cancelled`.

Licensed under Apache-2.0.
