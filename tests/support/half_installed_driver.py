"""A driver whose browser `alpha` opens and whose browser `beta` is not installed.

Used by tests/test_cli.py to check that `traceqa doctor` names the missing browsers.
"""

from __future__ import annotations

import json
import sys


def reply(request: dict, result: dict | None = None, error: dict | None = None) -> None:
    message: dict = {"jsonrpc": "2.0", "id": request["id"]}
    message.update({"error": error} if error else {"result": result})
    print(json.dumps(message), flush=True)


for line in sys.stdin:
    request = json.loads(line)
    method = request["method"]
    if method == "driver.hello":
        reply(
            request,
            {"driverName": "half", "driverVersion": "0", "protocolVersion": 1, "browsers": ["alpha", "beta"]},
        )
    elif method == "session.open":
        if request["params"]["target"]["browser"] == "alpha":
            reply(
                request,
                {
                    "sessionId": "s1",
                    "browser": {"name": "alpha", "version": "1"},
                    "capabilities": {
                        "clipScreenshot": False,
                        "fullPageScreenshot": False,
                        "evalAllFrames": False,
                        "crossOriginFrames": False,
                        "parallelSessions": False,
                        "setViewport": False,
                        "consoleEvents": False,
                        "networkEvents": False,
                    },
                },
            )
        else:
            reply(
                request,
                error={
                    "code": -32000,
                    "message": "beta is not installed. Run: traceqa install beta",
                    "data": {"kind": "unsupported", "retryable": False},
                },
            )
    elif method == "session.close":
        reply(request, {})
    elif method == "driver.shutdown":
        reply(request, {})
        break
