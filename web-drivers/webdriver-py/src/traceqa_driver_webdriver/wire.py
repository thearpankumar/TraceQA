"""The few W3C WebDriver commands the driver needs, over plain HTTP.

https://www.w3.org/TR/webdriver2/
"""

from __future__ import annotations

import http.client
import json
import urllib.error
import urllib.request
from typing import Any

# Longer than any page load or script the engine asks for.
REQUEST_TIMEOUT_S = 120.0


class WebDriverError(Exception):
    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


class WebDriverSession:
    """One browser session on a WebDriver server."""

    def __init__(self, server_url: str, capabilities: dict[str, Any]) -> None:
        self._server = server_url.rstrip("/")
        self._base = self._server
        created = self._request("POST", "/session", {"capabilities": {"alwaysMatch": capabilities}})
        self.capabilities: dict[str, Any] = created["capabilities"]
        self._base = f"{self._server}/session/{created['sessionId']}"

    def _request(self, method: str, path: str, body: dict[str, Any] | None = None) -> Any:
        data = json.dumps(body).encode() if body is not None else None
        headers = {"Content-Type": "application/json; charset=utf-8"}
        request = urllib.request.Request(self._base + path, data=data, method=method, headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=REQUEST_TIMEOUT_S) as response:
                payload = json.loads(response.read())
        except urllib.error.HTTPError as error:
            try:
                value = json.loads(error.read()).get("value") or {}
            except ValueError:
                value = {}
            raise WebDriverError(value.get("error", "unknown error"), value.get("message", str(error))) from error
        except (OSError, http.client.HTTPException, ValueError) as error:
            # The server is gone, cut the connection, or answered with something that is not JSON.
            raise WebDriverError(
                "unreachable", f"WebDriver server at {self._server} did not answer: {error}"
            ) from error
        if not isinstance(payload, dict):
            raise WebDriverError("unknown error", f"WebDriver server at {self._server} sent an unexpected reply")
        return payload.get("value")

    def execute(self, script: str, *args: Any) -> Any:
        """Run the body of a function in the current frame. `arguments` holds `args`."""
        return self._request("POST", "/execute/sync", {"script": script, "args": list(args)})

    def execute_async(self, script: str, *args: Any) -> Any:
        """Run the body of a function that ends by calling the callback it is given as its last
        `arguments` entry. The server waits for that call, up to the session's script timeout."""
        return self._request("POST", "/execute/async", {"script": script, "args": list(args)})

    def navigate(self, url: str) -> None:
        self._request("POST", "/url", {"url": url})

    def add_cookie(self, cookie: dict[str, Any]) -> None:
        """Set a cookie for the site the browser is on. WebDriver allows no other."""
        self._request("POST", "/cookie", {"cookie": cookie})

    def cookies(self) -> list[dict[str, Any]]:
        """The cookies of the site the browser is on."""
        return self._request("GET", "/cookie")

    def set_timeouts(self, page_load_ms: int, script_ms: int) -> None:
        self._request("POST", "/timeouts", {"pageLoad": page_load_ms, "script": script_ms})

    def window_rect(self) -> dict[str, float]:
        return self._request("GET", "/window/rect")

    def set_window_size(self, width: int, height: int) -> None:
        self._request("POST", "/window/rect", {"width": width, "height": height})

    def switch_to_frame(self, element: dict[str, str] | None) -> None:
        """Enter a frame by its element reference, or go back to the top with None."""
        self._request("POST", "/frame", {"id": element})

    def switch_to_parent_frame(self) -> None:
        self._request("POST", "/frame/parent", {})

    def screenshot_base64(self) -> str:
        return self._request("GET", "/screenshot")

    def quit(self) -> None:
        self._request("DELETE", "")
