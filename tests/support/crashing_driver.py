"""A driver that says hello like a real one, then dies while opening a browser.

Used by tests/test_failures.py to check that a crashed driver fails only its own target,
with a reason, and leaves a valid run folder.
"""

from __future__ import annotations

import json
import sys

for line in sys.stdin:
    request = json.loads(line)
    if request["method"] == "driver.hello":
        result = {"driverName": "crashing", "driverVersion": "0", "protocolVersion": 1, "browsers": ["chromium"]}
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
    elif request["method"] == "driver.shutdown":
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": {}}), flush=True)
        break
    else:
        sys.exit(3)  # dies without answering, as a driver that crashes does
