"""Address handling shared by every driver."""

from __future__ import annotations

from urllib.parse import urlsplit


def without_query(url: str) -> str:
    """The address as scheme, host and path only: no user name, password, query or fragment.
    Messages end up in logs and run files, and a query can carry a token."""
    try:
        parts = urlsplit(url)
        host = parts.hostname or ""
        if ":" in host:
            host = f"[{host}]"
        port = f":{parts.port}" if parts.port else ""
        if parts.scheme and host:
            return f"{parts.scheme}://{host}{port}{parts.path}"
    except ValueError:
        pass
    return url.split("?", 1)[0].split("#", 1)[0]
