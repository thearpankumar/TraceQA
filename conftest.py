"""Fixtures shared by every Python test suite in the repository."""

from __future__ import annotations

import importlib.util
from collections.abc import Iterator
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent


@pytest.fixture(scope="session")
def shop_url() -> Iterator[str]:
    """The Buggy Shop, served on free ports for the whole test session."""
    spec = importlib.util.spec_from_file_location("buggy_shop_serve", ROOT / "fixtures" / "buggy-shop" / "serve.py")
    shop = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(shop)
    port, servers = shop.start_on_free_ports()
    yield f"http://127.0.0.1:{port}/"
    for server in servers:
        server.shutdown()
