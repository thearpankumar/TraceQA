# traceqa-driver-playwright

TraceQA browser driver for Chromium, Edge, Firefox, WebKit and CDP endpoints,
built on [Playwright](https://playwright.dev/python/). The `traceqa` engine
starts it as a separate program and speaks the TraceQA driver protocol with it
over standard input and output.

The command is `traceqa-driver-playwright`. This package is built from the
repository and is not published to PyPI yet; install it from a checkout
(`uv sync`, then `uv run playwright install chromium`). See
[docs/usage.md](https://github.com/thearpankumar/TraceQA/blob/master/docs/usage.md#drivers)
for how the engine finds drivers.

Licensed under Apache-2.0.
