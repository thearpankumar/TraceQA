"""What every driver shares: the method table, sessions, `hello`, `shutdown` and scroll maths.

A driver subclasses `BaseDriver` and defines one method per protocol method it supports:
`driver.hello` is `hello`, `session.open` is `session_open`, `page.goto` is `page_goto`.
`handlers()` finds them by reflection over `contracts.DRIVER_PROTOCOL_METHODS`, so adding a
protocol method means regenerating `contracts` and writing the method, nothing else.
"""

from __future__ import annotations

import importlib.metadata
import struct
from abc import ABC, abstractmethod
from typing import ClassVar, Generic, TypeVar

from . import contracts, models
from .rpc import Channel, Handler, RpcError

S = TypeVar("S")

# Version reported when the driver runs from a bare checkout, without its package installed.
SOURCE_VERSION = "0.0.0+source"


def installed_version(distribution: str) -> str:
    """The installed version of `distribution`, the single source being its pyproject."""
    try:
        return importlib.metadata.version(distribution)
    except importlib.metadata.PackageNotFoundError:
        return SOURCE_VERSION


def method_name(method: str) -> str:
    """`driver.hello` is `hello`; any other `a.b` is `a_b`."""
    return method.removeprefix("driver.").replace(".", "_")


class BaseDriver(ABC, Generic[S]):
    NAME: ClassVar[str]
    VERSION: ClassVar[str]
    BROWSERS: ClassVar[tuple[str, ...]]
    EXCLUSIVE: ClassVar[tuple[str, ...]] = ()

    def __init__(self, channel: Channel) -> None:
        self._channel = channel
        self._sessions: dict[str, S] = {}
        self._opened = 0

    def handlers(self) -> dict[str, Handler]:
        """The methods this driver defines, keyed by protocol method name."""
        table: dict[str, Handler] = {}
        for method in contracts.DRIVER_PROTOCOL_METHODS:
            handler = getattr(self, method_name(method), None)
            if callable(handler):
                table[method] = handler
        return table

    def _session(self, session_id: str) -> S:
        try:
            return self._sessions[session_id]
        except KeyError:
            raise RpcError("session_not_found", f"no session {session_id}") from None

    def _new_session_id(self) -> str:
        self._opened += 1
        return f"s{self._opened}"

    @abstractmethod
    def _close(self, session_id: str) -> None:
        """Close one session and forget it."""

    def _stop(self) -> None:
        """Release what outlives the sessions. Called by `shutdown` after they are closed."""

    def hello(self, params: models.DriverHelloParams) -> models.DriverHelloResult:
        if params.protocol_version != contracts.DRIVER_PROTOCOL_VERSION:
            raise RpcError(
                "version_mismatch",
                f"driver speaks protocol {contracts.DRIVER_PROTOCOL_VERSION}, engine asked for {params.protocol_version}",
            )
        return models.DriverHelloResult(
            driver_name=self.NAME,
            driver_version=self.VERSION,
            protocol_version=contracts.DRIVER_PROTOCOL_VERSION,
            browsers=list(self.BROWSERS),
            exclusive_browsers=list(self.EXCLUSIVE) or None,
        )

    def shutdown(self, _params: models.Empty | None = None) -> models.Empty:
        for session_id in list(self._sessions):
            self._close(session_id)
        self._stop()
        return models.Empty()


def check_scroll_target(params: models.PageScrollParams) -> None:
    """A scroll names where to go with exactly one of `to` and `byY`."""
    if (params.to is None) == (params.by_y is None):
        raise RpcError("invalid_params", "give exactly one of 'to' or 'byY'")


def scrolled_to_end(y: float, view: float, height: float) -> bool:
    return y + view >= height - 1


def png_dimensions(data: bytes) -> tuple[int, int]:
    """Width and height from the PNG header (the first 24 bytes)."""
    width, height = struct.unpack(">II", data[16:24])
    return width, height
