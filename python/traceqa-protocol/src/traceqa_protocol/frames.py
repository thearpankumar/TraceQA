"""Helpers every driver needs when it reports the frames of a page."""

from __future__ import annotations

from urllib.parse import urlsplit

# A frame with one of these schemes has no origin of its own: it takes its parent's.
INHERITED_SCHEMES = {"about", "blob"}


def is_cross_origin(frame_url: str, page_url: str) -> bool:
    """True when a frame at `frame_url` has a different origin from the page at `page_url`."""
    frame, page = urlsplit(frame_url), urlsplit(page_url)
    if frame.scheme in INHERITED_SCHEMES:
        return False
    return (frame.scheme, frame.netloc) != (page.scheme, page.netloc)
