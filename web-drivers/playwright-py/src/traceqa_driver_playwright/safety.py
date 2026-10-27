"""The safety policy of a session: what the browser may not do while it is being acted on.

It is enforced in the browser itself, on the requests it makes, so it does not depend on what a button is called or
on anything the page says. Every decision here is "fail closed": a request the guard cannot classify is blocked.
Nothing in this file runs for a session opened without a policy, so reading pages is unchanged.
"""

from __future__ import annotations

import contextlib
from collections.abc import Callable
from urllib.parse import urlsplit

from playwright.sync_api import BrowserContext, Request, Route, WebSocketRoute
from playwright.sync_api import Error as PlaywrightError
from traceqa_protocol import models
from traceqa_protocol.rpc import RpcError

BLOCKED = "blocked_by_policy"
SAFE_METHODS = frozenset({"GET", "HEAD"})
DEFAULT_PORTS = {"http": 80, "https": 443}
# Close code 1008: the connection was refused because of a policy.
WEBSOCKET_POLICY_CODE = 1008

# Playwright's own setting for service workers (`service_workers="block"`) does not stop a page from registering one
# in any of the three browsers, and a service worker's own requests do not pass through the routes below. So the
# page is not given `navigator.serviceWorker` at all. This is not enforced by the browser: a worker that a page starts
# (a dedicated worker does not run this script) is not covered, which the documentation of the policy says.
NO_SERVICE_WORKERS_JS = """(function () {
  try {
    Object.defineProperty(Navigator.prototype, "serviceWorker", { get: function () { return undefined; }, configurable: false });
  } catch (e) { /* already fixed by an earlier copy of this script */ }
})();"""


def origin_of(address: str) -> str | None:
    """`scheme://host[:port]` as a browser compares origins: lower case, no default port. None for an address
    that has no origin (a `data:` address, text that is not an address) and for any scheme but http and https."""
    try:
        parts = urlsplit(address)
        port = parts.port
    except ValueError:
        return None
    scheme = parts.scheme.lower()
    host = parts.hostname
    if scheme not in DEFAULT_PORTS or not host:
        return None
    if ":" in host:
        host = f"[{host}]"
    return f"{scheme}://{host}" if port is None or port == DEFAULT_PORTS[scheme] else f"{scheme}://{host}:{port}"


class Guard:
    """Decides, for each request the browser makes, whether the policy lets it through."""

    def __init__(self, policy: models.ActPolicy, on_block: Callable[[str, str, Request | None], None]) -> None:
        origins = [origin_of(address) for address in policy.allow_origins]
        bad = [address for address, origin in zip(policy.allow_origins, origins, strict=True) if origin is None]
        if bad or not origins:
            # Refused before any browser is started: a policy nobody can read is no policy.
            raise RpcError(
                "invalid_params",
                f"policy.allowOrigins must be http or https origins such as http://127.0.0.1:4173, got {bad or 'none'}",
            )
        self.policy = policy
        self.allowed = {origin for origin in origins if origin is not None}
        self.read_only = not policy.allow_writes
        self._on_block = on_block
        # WebSockets the page opened, which have not been closed yet. See `drain_web_sockets`.
        self._web_sockets: list[WebSocketRoute] = []
        # Addresses of top-level navigations that were stopped before the page they were for existed (the first request
        # of a new tab), in the order they happened. See `take_unowned`.
        self._unowned: list[str] = []
        # Requests let through, by the origin they went to (see `counts`).
        self._requests: dict[str, int] = {}

    def counts(self) -> dict[str, int]:
        """How many requests the session's browser has sent to each origin so far. A request the guard stopped never
        left the browser and is not in it, and neither is one for an address that has no origin (`data:`, `blob:`)."""
        return dict(self._requests)

    def allows_page(self, address: str) -> bool:
        """Whether the top-level page may be at this address."""
        return origin_of(address) in self.allowed

    def blocks(self, method: str, address: str, *, top_level_navigation: bool) -> bool:
        """True when the request must be aborted."""
        if top_level_navigation and origin_of(address) not in self.allowed:
            return True
        return self.read_only and method.upper() not in SAFE_METHODS

    def install(self, context: BrowserContext) -> None:
        """Routes every request and, in read-only mode, every WebSocket of the context through the guard."""
        context.add_init_script(script=NO_SERVICE_WORKERS_JS)
        context.route("**/*", self._route)
        if self.read_only:
            context.route_web_socket("**/*", self._web_socket)

    def _route(self, route: Route, request: Request) -> None:
        try:
            has_frame = True
            try:
                top = request.is_navigation_request() and request.frame.parent_frame is None
            except PlaywrightError:
                # Cannot tell what kind of request it is: treat it as the most dangerous kind. This is what a new tab's
                # first request looks like: its page does not exist yet.
                top = True
                has_frame = False
            if self.blocks(request.method, request.url, top_level_navigation=top):
                self._on_block(request.url, request.method, request)
                if top and not has_frame:
                    self._unowned.append(request.url)
                route.abort("blockedbyclient")
            else:
                origin = origin_of(request.url)
                if origin is not None:
                    self._requests[origin] = self._requests.get(origin, 0) + 1
                route.continue_()
        except Exception:
            # Whatever went wrong, the request does not go out. A route handler that raised would leave it hanging.
            with contextlib.suppress(PlaywrightError):
                route.abort("blockedbyclient")
            raise

    def _web_socket(self, route: WebSocketRoute) -> None:
        # Never connected to the server, so nothing the page sends goes anywhere. It is not closed here: Playwright's
        # synchronous API cannot be used from inside this handler (it waits for itself and never returns), so the
        # connection is closed by `drain_web_sockets`, once the handler is done.
        self._on_block(route.url, "GET", None)
        self._web_sockets.append(route)

    def take_unowned(self) -> list[str]:
        """The addresses stopped since the last call that belong to no page yet, oldest first, and forgets them. A new
        tab that was stopped shows an error page, and this is where it was going."""
        taken, self._unowned = self._unowned, []
        return taken

    def drain_web_sockets(self) -> None:
        """Closes the WebSockets the page opened since the last call, so that the page sees the refusal."""
        pending, self._web_sockets = self._web_sockets, []
        for route in pending:
            with contextlib.suppress(PlaywrightError):
                route.close(code=WEBSOCKET_POLICY_CODE, reason="blocked by the safety policy")
