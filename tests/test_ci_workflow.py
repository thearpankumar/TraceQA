"""Structure of .github/workflows/ci.yml: triggers, concurrency and the CI guards."""

from __future__ import annotations

from pathlib import Path
from typing import Any

import yaml

ROOT = Path(__file__).resolve().parents[1]


def load_ci() -> dict[str, Any]:
    return yaml.safe_load((ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8"))


def triggers() -> dict[str, Any]:
    ci = load_ci()
    # PyYAML reads the bare key `on` as the boolean True.
    return ci.get(True, ci.get("on"))


def test_runs_on_demand_and_weekly() -> None:
    on = triggers()
    assert "workflow_dispatch" in on
    (entry,) = on["schedule"]
    fields = entry["cron"].split()
    assert len(fields) == 5
    assert fields[4] != "*", "the scheduled run is weekly, not daily"
    assert fields[0] not in ("0", "*/5", "*/15", "30"), "avoid the busiest minutes of the hour"


def test_changes_job_uses_the_tested_script() -> None:
    steps = load_ci()["jobs"]["changes"]["steps"]
    filter_step = next(step for step in steps if step.get("id") == "filter")
    assert filter_step["run"].strip() == "python3 scripts/ci_changed.py"
    assert (ROOT / "scripts" / "ci_changed.py").is_file()


def test_the_test_job_turns_on_the_skip_guards() -> None:
    env = load_ci()["jobs"]["test"]["env"]
    assert env["TRACEQA_REQUIRE_BINARY"] == "1"
    assert env["TRACEQA_NO_UNEXPECTED_SKIPS"] == "1"
    assert env["TRACEQA_REQUIRE_CHROMEDRIVER"] == "1"
