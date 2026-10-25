"""The CI guards in tests/support/ci_guards.py, run against small throwaway test suites."""

from __future__ import annotations

from pathlib import Path

import pytest

pytest_plugins = ["pytester"]

GUARDS = Path(__file__).resolve().parent / "support" / "ci_guards.py"

# The same loading the repository's conftest.py does, so the guards under test are the real ones.
CONFTEST = f"""
import importlib.util

_spec = importlib.util.spec_from_file_location("ci_guards", {str(GUARDS)!r})
_guards = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_guards)


def pytest_configure(config):
    config.pluginmanager.register(_guards, "ci-guards")
"""

SUITE = """
import pytest


@pytest.mark.skipif(True, reason="build the engine first: cargo build")
def test_binary_skip():
    pass


@pytest.mark.skipif(True, reason="build the stand-in engine first: cargo build")
def test_replay_skip():
    pass


def test_because():
    pytest.skip("because")


@pytest.mark.skipif(True, reason="set TRACEQA_TEST_SAFARI=1 to drive real Safari")
def test_allowed():
    pass


@pytest.mark.xfail(reason="known")
def test_xfail():
    assert False


def test_ok():
    pass
"""

SKIP_BECAUSE = "import pytest\n\n\ndef test_a():\n    pytest.skip('because')\n"


@pytest.fixture
def suite(pytester: pytest.Pytester, monkeypatch: pytest.MonkeyPatch) -> pytest.Pytester:
    for name in ("TRACEQA_REQUIRE_BINARY", "TRACEQA_NO_UNEXPECTED_SKIPS"):
        monkeypatch.delenv(name, raising=False)
    pytester.makeconftest(CONFTEST)
    return pytester


def run(pytester: pytest.Pytester, source: str) -> pytest.RunResult:
    pytester.makepyfile(test_inner=source)
    return pytester.runpytest_subprocess("-p", "no:cacheprovider", "-rs")


def test_missing_binary_skips_stay_skips_by_default(suite: pytest.Pytester) -> None:
    result = run(suite, SUITE)
    result.assert_outcomes(passed=1, skipped=4, xfailed=1)


def test_require_binary_turns_binary_skips_into_failures(
    suite: pytest.Pytester, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("TRACEQA_REQUIRE_BINARY", "1")
    result = run(suite, SUITE)
    result.assert_outcomes(passed=1, errors=2, skipped=2, xfailed=1)
    result.stdout.fnmatch_lines(["*TRACEQA_REQUIRE_BINARY=1 but the binary is missing: build the engine first*"])
    result.stdout.fnmatch_lines(["*TRACEQA_REQUIRE_BINARY=1 but the binary is missing: build the stand-in engine*"])


def test_require_binary_leaves_other_skips_alone(suite: pytest.Pytester, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("TRACEQA_REQUIRE_BINARY", "1")
    run(suite, SKIP_BECAUSE).assert_outcomes(skipped=1)


def test_unexpected_skips_fail_the_run_and_are_named(suite: pytest.Pytester, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("TRACEQA_NO_UNEXPECTED_SKIPS", "1")
    result = run(suite, SUITE)
    assert result.ret == pytest.ExitCode.TESTS_FAILED
    lines = result.stdout.lines
    listed = "\n".join(lines[lines.index("Unexpected skips:") + 1 :])
    assert "test_because" in listed
    assert "because" in listed
    assert "test_allowed" not in listed
    assert "test_xfail" not in listed


def test_allow_listed_skips_pass(suite: pytest.Pytester, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("TRACEQA_NO_UNEXPECTED_SKIPS", "1")
    source = (
        "import pytest\n\n\n"
        "@pytest.mark.skipif(True, reason='set TRACEQA_TEST_SAFARI=1 to drive real Safari')\n"
        "def test_a():\n    pass\n\n\n"
        'def test_b():\n    pytest.skip("the WebDriver driver has no hook to run a script before the page\'s")\n\n\n'
        "@pytest.mark.xfail(reason='x')\ndef test_c():\n    assert False\n"
    )
    result = run(suite, source)
    assert result.ret == pytest.ExitCode.OK
    result.assert_outcomes(skipped=2, xfailed=1)


def test_skips_are_ignored_when_the_variable_is_unset(suite: pytest.Pytester) -> None:
    result = run(suite, SKIP_BECAUSE)
    assert result.ret == pytest.ExitCode.OK
