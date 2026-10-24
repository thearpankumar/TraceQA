"""Structural checks of the GitHub workflows: who may run on the self-hosted pool, what is trusted."""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path
from typing import Any

import yaml

ROOT = Path(__file__).resolve().parents[1]
WORKFLOWS = ROOT / ".github" / "workflows"


def load(name: str) -> dict[str, Any]:
    return yaml.safe_load((WORKFLOWS / name).read_text(encoding="utf-8"))


def all_workflows() -> dict[str, dict[str, Any]]:
    return {p.name: load(p.name) for p in sorted(WORKFLOWS.glob("*.yml"))}


def steps_of(workflow: dict[str, Any]) -> Iterator[tuple[str, dict[str, Any]]]:
    for job_name, job in workflow["jobs"].items():
        for step in job.get("steps", []):
            yield job_name, step


def triggers_of(workflow: dict[str, Any]) -> Any:
    # PyYAML reads the bare key `on` as the boolean True.
    return workflow.get(True, workflow.get("on"))


def test_self_hosted_labels_are_only_referenced_in_the_changes_job() -> None:
    ci = load("ci.yml")
    holders = [n for n, j in ci["jobs"].items() if "RUNNER_LABELS" in str(j)]
    assert holders == ["changes"]
    assert "RUNNER_LABELS" in str(ci["jobs"]["changes"]["outputs"]["runner"])
    assert ci["jobs"]["changes"]["runs-on"] == "ubuntu-latest"
    for name, job in ci["jobs"].items():
        if name == "changes":
            continue
        runs_on = str(job["runs-on"])
        assert runs_on == "ubuntu-latest" or "needs.changes.outputs.runner" in runs_on, (name, runs_on)


def test_the_runner_rule_excludes_fork_and_dependabot_pull_requests() -> None:
    expr = load("ci.yml")["jobs"]["changes"]["outputs"]["runner"]
    assert "github.event.pull_request.head.repo.full_name == github.repository" in expr
    assert "dependabot[bot]" in expr
    assert "github.event_name != 'pull_request'" in expr
    assert "'\"ubuntu-latest\"'" in expr


def test_no_workflow_uses_pull_request_target() -> None:
    for name, wf in all_workflows().items():
        assert "pull_request_target" not in str(triggers_of(wf)), name


def test_every_checkout_drops_credentials() -> None:
    seen = 0
    for name, wf in all_workflows().items():
        for job, step in steps_of(wf):
            if step.get("uses", "").startswith("actions/checkout"):
                seen += 1
                assert step.get("with", {}).get("persist-credentials") is False, (name, job)
    assert seen >= 4


def test_no_run_script_interpolates_event_data() -> None:
    for name, wf in all_workflows().items():
        for job, step in steps_of(wf):
            assert "github.event." not in step.get("run", ""), (name, job, step.get("name"))
