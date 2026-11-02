from __future__ import annotations

import importlib.util
import sys
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
from traceqa_protocol.testing import DriverClient


@pytest.fixture
def driver() -> Iterator[DriverClient]:
    client = DriverClient([sys.executable, "-m", "traceqa_driver_playwright"])
    yield client
    client.shutdown()


ROOT = Path(__file__).resolve().parents[3]


def _load_by_path(name: str, path: Path) -> Any:
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def act_site() -> Iterator[Any]:
    """testbed/sites/act, the fixture with one control for each thing an action can cause. A fresh server for
    every test, so its request log holds only what that test did."""
    site = _load_by_path("act_serve", ROOT / "testbed" / "sites" / "act" / "serve.py")
    server = site.start()
    server.base = f"http://127.0.0.1:{server.server_address[1]}/"
    yield server
    server.shutdown()
    server.server_close()


@pytest.fixture
def other_site() -> Iterator[Any]:
    """A second act fixture on another port, which is another origin, for tests of what must not be reached."""
    site = _load_by_path("act_serve_other", ROOT / "testbed" / "sites" / "act" / "serve.py")
    server = site.start()
    server.base = f"http://127.0.0.1:{server.server_address[1]}/"
    yield server
    server.shutdown()
    server.server_close()
