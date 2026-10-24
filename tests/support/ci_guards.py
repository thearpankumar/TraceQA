"""Guards that keep a skipped test from passing for a working one in CI.

conftest.py registers this module as a plugin. Both guards are off unless CI sets their variable:

TRACEQA_REQUIRE_BINARY=1
    A test skipped with "build the engine first" (or "build the stand-in engine first") fails
    instead, so a missing binary cannot hide a whole suite.
TRACEQA_NO_UNEXPECTED_SKIPS=1
    The run fails when any test is skipped for a reason that is not in ALLOWED_SKIPS. Adding a
    skip with a new reason breaks CI until the reason is listed here and reviewed with the change.
"""

from __future__ import annotations

import os
import re
from typing import Any

import pytest

# Full-string patterns. Everything else (binary missing, no chromedriver, no uv, golden file written)
# is a skip that CI must not accept.
ALLOWED_SKIPS = [
    r"the WebDriver driver has no hook to run a script before the page's",
    r"set TRACEQA_TEST_SAFARI=1 to drive real Safari",
]

MISSING_BINARY = re.compile(r"build the (stand-in )?engine first.*", re.DOTALL)

_unexpected: list[tuple[str, str]] = []


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


def pytest_runtest_logreport(report: pytest.TestReport) -> None:
    if os.environ.get("TRACEQA_NO_UNEXPECTED_SKIPS") != "1" or not _is_plain_skip(report):
        return
    reason = _reason(report)
    if not any(re.fullmatch(pattern, reason) for pattern in ALLOWED_SKIPS):
        _unexpected.append((report.nodeid, reason))


def pytest_sessionstart(session: pytest.Session) -> None:
    _unexpected.clear()


def pytest_sessionfinish(session: pytest.Session) -> None:
    if _unexpected and session.exitstatus == pytest.ExitCode.OK:
        session.exitstatus = pytest.ExitCode.TESTS_FAILED


def pytest_terminal_summary(terminalreporter: Any) -> None:
    if not _unexpected:
        return
    terminalreporter.write_line("")
    terminalreporter.write_line("Unexpected skips:")
    for nodeid, reason in _unexpected:
        terminalreporter.write_line(f"  {nodeid}: {reason}")
    terminalreporter.write_line("Fix the cause, or list the reason in ALLOWED_SKIPS in tests/support/ci_guards.py.")
