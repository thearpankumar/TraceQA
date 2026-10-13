"""JSON-RPC 2.0 over lines, and the request loop every driver runs.

A driver is a program the engine starts. It reads one request per line from
stdin and writes one response per line to stdout. `serve` does that plumbing
and checks every message against the spec, so a driver only supplies handlers.
"""

from __future__ import annotations

import json
import os
import sys
from collections.abc import Callable
from typing import IO, Any

from pydantic import BaseModel, ValidationError

from . import contracts, models

INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INVALID_PARAMS = -32602
APPLICATION_ERROR = -32000

Handler = Callable[[Any], BaseModel]


def to_wire(message: BaseModel) -> dict[str, Any]:
    """Turn a model into the JSON object that goes on the wire or into a file."""
    return message.model_dump(mode="json", by_alias=True, exclude_none=True)


class RpcError(Exception):
    """Raise from a handler to fail a request with a spec `RpcErrorData`."""

    def __init__(self, kind: str, message: str, *, retryable: bool = False, code: int = APPLICATION_ERROR) -> None:
        super().__init__(message)
        self.kind = kind
        self.message = message
        self.retryable = retryable
        self.code = code

    def to_wire(self) -> dict[str, Any]:
        data = models.RpcErrorData(kind=self.kind, retryable=self.retryable)
        return {"code": self.code, "message": self.message, "data": to_wire(data)}


class Channel:
    """The driver's side of the pipe to the engine."""

    def __init__(self, reader: IO[str], writer: IO[str]) -> None:
        self._reader = reader
        self._writer = writer

    @classmethod
    def from_stdio(cls) -> Channel:
        """Take over stdout for the protocol and point stray prints at stderr."""
        writer = os.fdopen(os.dup(sys.stdout.fileno()), "w", encoding="utf-8")
        os.dup2(sys.stderr.fileno(), sys.stdout.fileno())
        sys.stdout = sys.stderr
        reader = open(sys.stdin.fileno(), encoding="utf-8", closefd=False)  # noqa: SIM115
        return cls(reader, writer)

    def send(self, message: dict[str, Any]) -> None:
        self._writer.write(json.dumps({"jsonrpc": "2.0", **message}, ensure_ascii=False) + "\n")
        self._writer.flush()

    def log(self, level: str, message: str) -> None:
        params = models.DriverLogParams(level=models.LogLevel(level), message=message)
        self.send({"method": "driver.log", "params": to_wire(params)})

    def serve(self, handlers: dict[str, Handler]) -> None:
        """Answer requests until `driver.shutdown` is handled or the engine closes the pipe."""
        for line in self._reader:
            if not line.strip():
                continue
            request_id = None
            method = None
            try:
                try:
                    request = json.loads(line)
                except json.JSONDecodeError:
                    continue  # no id can be read from it, so there is nobody to answer
                if not isinstance(request, dict):
                    raise RpcError("invalid_params", "not a JSON-RPC request", code=INVALID_REQUEST)
                # Read the id first, so a request that is refused below still gets its answer.
                request_id = request.get("id")
                method = request.get("method")
                if not isinstance(method, str):
                    raise RpcError("invalid_params", "a request needs a method", code=INVALID_REQUEST)
                result = self._dispatch(handlers, method, request.get("params"))
            except RpcError as error:
                if request_id is not None:
                    self.send({"id": request_id, "error": error.to_wire()})
            except Exception as error:  # a driver must answer every request
                if request_id is not None:
                    failure = RpcError("internal", f"{type(error).__name__}: {error}")
                    self.send({"id": request_id, "error": failure.to_wire()})
            else:
                if request_id is not None:
                    self.send({"id": request_id, "result": result})
            if method == "driver.shutdown":
                return

    @staticmethod
    def _dispatch(handlers: dict[str, Handler], method: str, params: Any) -> dict[str, Any]:
        spec = contracts.DRIVER_PROTOCOL_METHODS.get(method)
        handler = handlers.get(method)
        if spec is None or handler is None:
            raise RpcError("unsupported", f"unknown method {method}", code=METHOD_NOT_FOUND)
        try:
            parsed = getattr(models, spec["params"]).model_validate(params or {})
        except ValidationError as error:
            raise RpcError("invalid_params", str(error), code=INVALID_PARAMS) from error
        result = handler(parsed)
        return to_wire(getattr(models, spec["result"]).model_validate(result))
