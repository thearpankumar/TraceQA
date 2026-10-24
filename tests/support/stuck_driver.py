"""A driver that opens a page and then goes silent: it never answers closing the session.

Used by tests/test_failures.py to check that a second Ctrl-C quits at once with a valid run
folder even though the driver is stuck. Run it with a long `--settle-ms`: the engine waits there
between driver calls, so the first Ctrl-C stops the run with the driver idle and not lost, and the
engine then asks it to close the session, which waits for the driver's own timeout (30 s). Only the
abort can end the run before that. The driver ends when its input closes, so it never outlives
the command that started it.
"""

from __future__ import annotations

import json
import signal
import sys

OPENED = {
    "sessionId": "s1",
    "browser": {"name": "chromium", "version": "1.0"},
    "capabilities": {
        "clipScreenshot": False,
        "fullPageScreenshot": True,
        "evalAllFrames": True,
        "crossOriginFrames": False,
        "parallelSessions": False,
        "setViewport": True,
        "consoleEvents": False,
        "networkEvents": False,
    },
}
ANSWERS = {
    "driver.hello": {"driverName": "stuck", "driverVersion": "0", "protocolVersion": 1, "browsers": ["chromium"]},
    "session.open": OPENED,
    "page.goto": {"finalUrl": "http://127.0.0.1/", "status": 200, "title": "Stuck"},
}

# A real stuck driver does not die of Ctrl-C either: ignore what a terminal sends.
for name in ("SIGINT", "SIGBREAK"):
    if hasattr(signal, name):
        signal.signal(getattr(signal, name), signal.SIG_IGN)

for line in sys.stdin:
    request = json.loads(line)
    answer = ANSWERS.get(request["method"])
    if answer is not None:
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": answer}), flush=True)
    # Anything else (session.close, driver.shutdown) is read and left unanswered.
