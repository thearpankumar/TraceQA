from __future__ import annotations

import sys
from collections.abc import Iterator

import pytest
from traceqa_protocol.testing import DriverClient


@pytest.fixture
def driver() -> Iterator[DriverClient]:
    client = DriverClient([sys.executable, "-m", "traceqa_driver_playwright"])
    yield client
    client.shutdown()
