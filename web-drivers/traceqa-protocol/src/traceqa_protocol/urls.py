"""Address handling shared by every driver."""

from __future__ import annotations

import re
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


# An address in running text ends at white space or one of these.
_ADDRESS = re.compile(r"https?://[^\s\"'<>)]+", re.IGNORECASE)


def redact_text(text: str, limit: int | None = None) -> str:
    """The text with every http(s) address cut down by `without_query`, then cut to `limit` characters.
    For page text that reaches a result (a console message can quote a request address with a token in it).
    The addresses are changed first, so cutting cannot leave the start of a secret behind."""
    safe = _ADDRESS.sub(lambda found: without_query(found.group(0)), text)
    return safe if limit is None else safe[:limit]
