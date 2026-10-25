"""A driver whose page never finishes loading, but which closes cleanly when asked.

Used by tests/test_cli.py and tests/test_mcp.py to keep a run in flight while a signal is sent to
`traceqa serve` or `traceqa mcp`. It answers hello, session.open, session.close and
driver.shutdown, and never answers page.goto. `--marker FILE` is created when page.goto arrives,
so a test knows the run is in the middle of a page. Unlike deaf_driver.py it ends when its input
closes, so it cannot outlive the engine that started it, and it ignores Ctrl-C like a driver that
shares the terminal.
"""

from __future__ import annotations

import json
import signal
import sys
from pathlib import Path

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
    "driver.hello": {"driverName": "slow", "driverVersion": "0", "protocolVersion": 1, "browsers": ["chromium"]},
    "session.open": OPENED,
    "session.close": {},
    "driver.shutdown": {},
}

for name in ("SIGINT", "SIGBREAK"):
    if hasattr(signal, name):
        signal.signal(getattr(signal, name), signal.SIG_IGN)

marker = Path(sys.argv[sys.argv.index("--marker") + 1]) if "--marker" in sys.argv else None

for line in sys.stdin:
    request = json.loads(line)
    if request["method"] == "page.goto" and marker is not None:
        marker.write_text("goto", encoding="utf-8")
    answer = ANSWERS.get(request["method"])
    if answer is not None:
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": answer}), flush=True)
    # page.goto and anything else are read and left unanswered.
