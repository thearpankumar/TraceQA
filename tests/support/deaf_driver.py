"""A driver that cannot be stopped from outside: it ignores signals and survives its input closing.

Used by tests/test_failures.py and tests/test_login.py to check that a second interrupt kills and
reaps the drivers the command started. It answers hello, session.open and page.goto like
stuck_driver.py and stays silent after that. Unlike stuck_driver.py it does not end when its
input closes: it sleeps until it is killed. The tests fail if it outlives the command, and the
process list in the failure names it. It is safe to leave behind by accident: it ends itself
after ten minutes.

`--grandchild FILE [--setsid]` makes it start a second deaf process of the same kind (`--leaf`:
no stdin, ignores signals) and write both pids to FILE as JSON, `{"driver": N, "grandchild": M}`,
before it answers anything. With `--setsid` the grandchild leaves the driver's session and group,
as a browser may, so only a walk of parent pids reaches it.
"""

from __future__ import annotations

import json
import os
import signal
import subprocess
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

if "--leaf" in sys.argv:
    # The grandchild: never reads its input, ends itself after ten minutes.
    time.sleep(600)
    sys.exit(0)

if "--grandchild" in sys.argv:
    target = sys.argv[sys.argv.index("--grandchild") + 1]
    options: dict = {"start_new_session": True} if "--setsid" in sys.argv and os.name != "nt" else {}
    leaf = subprocess.Popen(
        [sys.executable, __file__, "--leaf"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        **options,
    )
    with open(target + ".tmp", "w", encoding="utf-8") as handle:
        json.dump({"driver": os.getpid(), "grandchild": leaf.pid}, handle)
    os.replace(target + ".tmp", target)

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
