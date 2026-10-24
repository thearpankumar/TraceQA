"""Guards that keep a skipped test from passing for a working one in CI.

conftest.py registers this module as a plugin. The guard is off unless CI sets its variable:

TRACEQA_REQUIRE_BINARY=1
    A test skipped with "build the engine first" (or "build the stand-in engine first") fails
    instead, so a missing binary cannot hide a whole suite.
"""

from __future__ import annotations

import os
import re
from typing import Any

import pytest

MISSING_BINARY = re.compile(r"build the (stand-in )?engine first.*", re.DOTALL)


def _reason(report: Any) -> str:
    longrepr = report.longrepr
    text = longrepr[2] if isinstance(longrepr, tuple) else str(longrepr)
    return text.removeprefix("Skipped: ")


def _is_plain_skip(report: Any) -> bool:
    # An expected failure is reported as "skipped" too; it carries wasxfail.
    return report.skipped and not hasattr(report, "wasxfail")


@pytest.hookimpl(wrapper=True)
def pytest_runtest_makereport(item: pytest.Item, call: pytest.CallInfo[None]):
    report = yield
    if os.environ.get("TRACEQA_REQUIRE_BINARY") == "1" and _is_plain_skip(report):
        reason = _reason(report)
        if MISSING_BINARY.fullmatch(reason):
            report.outcome = "failed"
            report.longrepr = f"TRACEQA_REQUIRE_BINARY=1 but the binary is missing: {reason}"
    return report
