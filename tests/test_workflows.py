"""Structural checks of the GitHub workflows: who may run on the self-hosted pool, what is trusted."""

from __future__ import annotations

import re
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


def test_rust_cache_saves_only_on_master_pushes() -> None:
    seen = 0
    for name, wf in all_workflows().items():
        for job, step in steps_of(wf):
            if step.get("uses", "").startswith("Swatinem/rust-cache@"):
                seen += 1
                save_if = step.get("with", {}).get("save-if", "")
                assert "refs/heads/master" in save_if and "'push'" in save_if, (name, job)
    assert seen == 2


def test_pull_requests_on_the_self_hosted_pool_use_isolated_tool_homes() -> None:
    ci = load("ci.yml")
    for job in ("lint", "test"):
        steps = ci["jobs"][job]["steps"]
        isolate = [i for i, s in enumerate(steps) if "runner.environment == 'self-hosted'" in s.get("if", "")]
        assert isolate, job
        step = steps[isolate[0]]
        assert "github.event_name == 'pull_request'" in step["if"]
        for var in ("CARGO_HOME", "RUSTUP_HOME", "UV_CACHE_DIR", "npm_config_cache", "TRACEQA_HOME"):
            assert f"{var}=$RUNNER_TEMP/" in step["run"], (job, var)
        toolchain = next(i for i, s in enumerate(steps) if "rust-toolchain" in s.get("uses", ""))
        assert isolate[0] < toolchain, job
        # The final step cleans the workspace on the pool, whatever happened before.
        assert steps[-1]["if"] == "always() && runner.environment == 'self-hosted'", job
        assert "rm -rf target" in steps[-1]["run"]


def test_release_publish_job_alone_can_write() -> None:
    release = load("release.yml")
    assert release["permissions"] == {"contents": "read"}
    for name, job in release["jobs"].items():
        perms = job.get("permissions", {})
        if name == "publish":
            assert perms == {"contents": "write", "id-token": "write", "attestations": "write"}
        else:
            assert not perms, name


def test_release_uses_the_release_environment_and_a_pinned_toolchain() -> None:
    release = load("release.yml")
    assert release["jobs"]["publish"]["environment"] == "release"
    assert re.fullmatch(r"\d+\.\d+\.\d+", release["env"]["RUST_TOOLCHAIN"])
    toolchain_steps = [
        step for _, step in steps_of(release) if step.get("uses", "").startswith("dtolnay/rust-toolchain@")
    ]
    assert toolchain_steps
    for step in toolchain_steps:
        assert step["with"]["toolchain"] == "${{ env.RUST_TOOLCHAIN }}"


def test_release_checks_the_tag_is_on_master() -> None:
    steps = load("release.yml")["jobs"]["check"]["steps"]
    checkout = steps[0]
    assert checkout["with"]["fetch-depth"] == 0
    script = "\n".join(s.get("run", "") for s in steps)
    assert "merge-base --is-ancestor" in script and "origin/master" in script


def test_release_builds_wait_for_the_audit_job() -> None:
    jobs = load("release.yml")["jobs"]
    assert "check" in jobs["audit"]["needs"]
    assert "audit" in jobs["build"]["needs"]
    script = "\n".join(s.get("run", "") for s in jobs["audit"]["steps"])
    assert "cargo audit" in script and "cargo deny" in script


def test_release_attests_every_asset_in_the_publish_job_only() -> None:
    jobs = load("release.yml")["jobs"]
    for name, job in jobs.items():
        uses = [s.get("uses", "") for s in job["steps"]]
        attests = [u for u in uses if u.startswith("actions/attest@")]
        assert bool(attests) == (name == "publish"), name
    publish = jobs["publish"]["steps"]
    attest = next(s for s in publish if s.get("uses", "").startswith("actions/attest@"))
    assert attest["with"]["subject-path"] == "dist/*"
    names = [s.get("name") for s in publish]
    assert names.index("Checksums") < publish.index(attest) < names.index("Create the release")
    assert publish.index(attest) == names.index("Attest the build provenance")
    assert "--verify-tag" in publish[-1]["run"]


def test_audit_workflow_never_uses_the_self_hosted_pool_and_has_read_only_permissions() -> None:
    audit = load("audit.yml")
    assert audit["permissions"] == {"contents": "read"}
    assert "RUNNER_LABELS" not in (WORKFLOWS / "audit.yml").read_text(encoding="utf-8")
    for name, job in audit["jobs"].items():
        assert job["runs-on"] == "ubuntu-latest", name
        assert "permissions" not in job and job["timeout-minutes"], name
    triggers = triggers_of(audit)
    assert {"schedule", "workflow_dispatch", "push", "pull_request"} <= set(triggers)
    for event in ("push", "pull_request"):
        assert "Cargo.lock" in triggers[event]["paths"] and "uv.lock" in triggers[event]["paths"]
    text = (WORKFLOWS / "audit.yml").read_text(encoding="utf-8")
    for command in ("cargo audit", "cargo deny", "npm audit", "pip-audit=="):
        assert command in text, command


def test_dependabot_covers_every_ecosystem_in_the_repo() -> None:
    config = yaml.safe_load((ROOT / ".github" / "dependabot.yml").read_text(encoding="utf-8"))
    assert config["version"] == 2
    ecosystems = {u["package-ecosystem"] for u in config["updates"]}
    assert {"cargo", "npm", "github-actions"} <= ecosystems
    assert ecosystems & {"uv", "pip"}
    for update in config["updates"]:
        assert (ROOT / update["directory"].lstrip("/")).is_dir(), update
        assert update["schedule"]["interval"] == "weekly"
        assert update["commit-message"]["prefix"] == "chore(deps)"
    # Each ecosystem's manifest or lock file really is where Dependabot is pointed.
    assert (ROOT / "Cargo.lock").is_file() and (ROOT / "uv.lock").is_file()
    assert (ROOT / "sdks" / "typescript" / "package-lock.json").is_file()


def test_codeowners_gates_the_github_directory() -> None:
    lines = (ROOT / ".github" / "CODEOWNERS").read_text(encoding="utf-8").splitlines()
    rules = dict(line.split(None, 1) for line in lines if line.strip() and not line.startswith("#"))
    assert rules["*"].startswith("@")
    for path in ("/.github/", "/SECURITY.md", "/deny.toml", "/scripts/", "/Cargo.lock", "/crates/traceqa-mcp/"):
        assert path in rules, path


def test_the_security_policy_names_the_private_reporting_channel() -> None:
    text = (ROOT / "SECURITY.md").read_text(encoding="utf-8")
    assert "https://github.com/thearpankumar/TraceQA/security/advisories/new" in text
    assert "SECURITY.md" in (ROOT / "README.md").read_text(encoding="utf-8")


def test_deny_policy_denies_unknown_sources_and_lists_its_licences() -> None:
    import tomllib

    policy = tomllib.loads((ROOT / "deny.toml").read_text(encoding="utf-8"))
    assert policy["sources"]["unknown-registry"] == "deny"
    assert policy["sources"]["unknown-git"] == "deny"
    assert policy["advisories"]["ignore"] == []
    assert set(policy["licenses"]["allow"]) == {"Apache-2.0", "MIT", "Unicode-3.0"}


PINNED = re.compile(r"^[\w.-]+/[\w./-]+@[0-9a-f]{40}$")


def test_every_action_is_pinned_to_a_full_commit_sha() -> None:
    checked = 0
    for path in sorted(WORKFLOWS.glob("*.yml")):
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            match = re.match(r"\s*(?:-\s+)?uses:\s*(\S+)(.*)$", line)
            if not match or match.group(1).startswith("./"):
                continue
            checked += 1
            where = f"{path.name}:{number}"
            assert PINNED.match(match.group(1)), where
            assert re.match(r"\s*#\s*(v\d[\w.-]*|master)\s*$", match.group(2)), where
    assert checked >= 10


def test_dtolnay_toolchain_steps_name_their_toolchain() -> None:
    seen = 0
    for name, wf in all_workflows().items():
        for job, step in steps_of(wf):
            if step.get("uses", "").startswith("dtolnay/rust-toolchain@"):
                seen += 1
                assert step.get("with", {}).get("toolchain"), (name, job)
    assert seen >= 3


def declared_rust_version() -> str:
    """`rust-version` under [workspace.package] in the root Cargo.toml."""
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    section = re.search(r"^\[workspace\.package\]\n(.*?)(?=^\[|\Z)", text, re.DOTALL | re.MULTILINE)
    assert section, "no [workspace.package] section"
    found = re.search(r'^rust-version\s*=\s*"(\d+\.\d+(?:\.\d+)?)"\s*$', section.group(1), re.MULTILINE)
    assert found, "[workspace.package] does not declare rust-version"
    return found.group(1)


def test_msrv_job_uses_the_declared_rust_version() -> None:
    ci = load("ci.yml")
    job = ci["jobs"]["msrv"]
    assert job["runs-on"] == "ubuntu-latest"
    toolchains = [s for s in job["steps"] if s.get("uses", "").startswith("dtolnay/rust-toolchain@")]
    assert [s["with"]["toolchain"] for s in toolchains] == [declared_rust_version()]
    runs = [s["run"] for s in job["steps"] if "run" in s]
    assert "cargo check --workspace --locked" in runs
    # The CI job waits for it and fails when it fails.
    assert "msrv" in ci["jobs"]["ci"]["needs"]
    assert "needs.msrv.result" in str(ci["jobs"]["ci"]["steps"])
    assert ci["jobs"]["msrv"]["if"] == ci["jobs"]["lint"]["if"]


def test_every_crate_inherits_rust_version() -> None:
    manifests = sorted((ROOT / "crates").glob("*/Cargo.toml"))
    assert len(manifests) >= 6
    for path in manifests:
        text = path.read_text(encoding="utf-8")
        assert re.search(r"^rust-version\.workspace\s*=\s*true\s*$", text, re.MULTILINE), path.parent.name
        assert not re.search(r"^rust-version\s*=", text, re.MULTILINE), path.parent.name
    members = re.search(r"^members\s*=\s*\[(.*?)\]", (ROOT / "Cargo.toml").read_text(encoding="utf-8"), re.MULTILINE)
    assert members and '"crates/*"' in members.group(1)


def test_clippy_runs_on_every_os() -> None:
    # Code behind #[cfg(windows)] or #[cfg(target_os = "macos")] is only linted on its own system.
    job = load("ci.yml")["jobs"]["test"]
    assert {"ubuntu-latest", "windows-latest", "macos-latest"} <= set(job["strategy"]["matrix"]["os"])
    clippy = [s for s in job["steps"] if "cargo clippy" in s.get("run", "")]
    assert clippy, "the Test job has no clippy step"
    assert all("-D warnings" in s["run"] and "--all-targets" in s["run"] for s in clippy)
    toolchain = [s for s in job["steps"] if s.get("uses", "").startswith("dtolnay/rust-toolchain")]
    assert "clippy" in toolchain[0]["with"]["components"]


def test_gitattributes_pins_lf_and_marks_binaries() -> None:
    import subprocess

    lines = [ln.split("#")[0].split() for ln in (ROOT / ".gitattributes").read_text(encoding="utf-8").splitlines()]
    rules = {ln[0]: ln[1:] for ln in lines if ln}
    assert rules["*"][:2] == ["text=auto", "eol=lf"]
    binary_globs = [g for g, attrs in rules.items() if "binary" in attrs]
    listing = subprocess.run(["git", "ls-files", "-z"], cwd=ROOT, capture_output=True, check=True)
    uncovered = []
    for name in filter(None, listing.stdout.decode().split("\0")):
        path = ROOT / name
        if not path.is_file():
            continue
        try:
            path.read_bytes().decode("utf-8")
        except UnicodeDecodeError:
            if not any(Path(name).match(g) for g in binary_globs):
                uncovered.append(name)
    assert not uncovered, f"binary files without a binary rule in .gitattributes: {uncovered}"


def test_release_build_pins_the_commit_explicitly() -> None:
    # The installer pins the commit the binary was built from; it must not depend on tree state.
    steps = [s for _, s in steps_of(load("release.yml")) if "cargo build --release" in s.get("run", "")]
    assert steps
    for step in steps:
        assert step.get("env", {}).get("TRACEQA_RELEASE_COMMIT") == "${{ github.sha }}"
