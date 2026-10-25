"""The descendant filter and the leftover check of tests/support/console_runner.py, on every OS."""

from __future__ import annotations

import importlib.util
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "console_runner", Path(__file__).resolve().parent / "support" / "console_runner.py"
)
assert SPEC is not None and SPEC.loader is not None
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


def test_a_real_grandchild_is_a_descendant() -> None:
    table = {10: (1, "cmd", 100), 20: (10, "a", 110), 30: (20, "b", 120), 40: (5, "other", 90)}
    assert set(runner.filter_descendants(10, table)) == {20, 30}


def test_a_reused_parent_pid_does_not_make_a_foreign_process_a_descendant() -> None:
    # pid 99 names pid 10 as its parent but is older than pid 10: the old owner of pid 10 started it.
    table = {10: (1, "cmd", 100), 99: (10, "foreign", 50), 98: (99, "its child", 60), 20: (10, "a", 110)}
    assert set(runner.filter_descendants(10, table)) == {20}


def test_unknown_creation_times_fall_back_to_parent_links() -> None:
    table = {10: (1, "cmd", None), 20: (10, "a", None), 30: (20, "b", 5)}
    assert set(runner.filter_descendants(10, table)) == {20, 30}


def test_cycles_and_self_parent_terminate() -> None:
    table = {10: (1, "cmd", 100), 20: (20, "self", 110), 30: (40, "x", 120), 40: (30, "y", 120), 50: (10, "ok", 110)}
    assert set(runner.filter_descendants(10, table)) == {50}
    assert set(runner.filter_descendants(30, table)) == {40}


def test_a_leftover_check_ignores_a_reused_pid() -> None:
    group = {20: ("a.exe", 110)}
    assert runner.leftovers(group, alive=lambda pid: True, created_of=lambda pid: 500) == []
    assert runner.leftovers(group, alive=lambda pid: True, created_of=lambda pid: 110) == ["20 a.exe"]
    assert runner.leftovers(group, alive=lambda pid: False, created_of=lambda pid: 110) == []


def test_a_leftover_with_an_unreadable_time_is_still_reported() -> None:
    group = {20: ("a.exe", 110)}
    assert runner.leftovers(group, alive=lambda pid: True, created_of=lambda pid: None) == ["20 a.exe"]
