"""Helpers for testing a driver: play the engine's part against a real driver process."""

from __future__ import annotations

import json
import subprocess
from typing import Any


class RpcFailure(Exception):
    """A request the driver answered with an error."""

    def __init__(self, error: dict[str, Any]) -> None:
        super().__init__(error["message"])
        self.kind: str | None = (error.get("data") or {}).get("kind")


class DriverClient:
    """Starts a driver and sends it requests, one at a time."""

    def __init__(self, command: list[str]) -> None:
        self.process = subprocess.Popen(
            command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, encoding="utf-8"
        )
        self._next_id = 0

    def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        self._next_id += 1
        request = {"jsonrpc": "2.0", "id": self._next_id, "method": method, "params": params or {}}
        self.process.stdin.write(json.dumps(request) + "\n")
        self.process.stdin.flush()
        while True:
            line = self.process.stdout.readline()
            if not line:
                raise RuntimeError(f"driver exited while handling {method}")
            message = json.loads(line)
            if message.get("id") != self._next_id:
                continue  # a notification, such as driver.log
            if "error" in message:
                raise RpcFailure(message["error"])
            return message["result"]

    def shutdown(self) -> None:
        self.call("driver.shutdown")
        self.process.wait(timeout=15)
