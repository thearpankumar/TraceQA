"""A driver that cannot be stopped from outside: it ignores signals and survives its input closing.

Used by tests/test_failures.py and tests/test_login.py to check that a second interrupt kills and
reaps the drivers the command started. It answers hello, session.open and page.goto like
stuck_driver.py and stays silent after that. Unlike stuck_driver.py it does not end when its
input closes: it sleeps until it is killed. The tests fail if it outlives the command, and the
process list in the failure names it. It is safe to leave behind by accident: it ends itself
after ten minutes.
"""

from __future__ import annotations

import json
import signal
import sys
import time

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
    "driver.hello": {"driverName": "deaf", "driverVersion": "0", "protocolVersion": 1, "browsers": ["chromium"]},
    "session.open": OPENED,
    "page.goto": {"finalUrl": "http://127.0.0.1/", "status": 200, "title": "Stuck"},
}

# Ignore everything a terminal or a process manager sends.
for name in ("SIGINT", "SIGTERM", "SIGHUP", "SIGBREAK"):
    if hasattr(signal, name):
        signal.signal(getattr(signal, name), signal.SIG_IGN)

for line in sys.stdin:
    request = json.loads(line)
    answer = ANSWERS.get(request["method"])
    if answer is not None:
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": answer}), flush=True)
    # Anything else (session.close, driver.shutdown) is read and left unanswered.

# The input closed: a well-behaved driver would exit here. This one waits to be killed.
deadline = time.monotonic() + 600
while time.monotonic() < deadline:
    time.sleep(1)
