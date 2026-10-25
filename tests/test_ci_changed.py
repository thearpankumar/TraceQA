"""scripts/ci_changed.py: which changes may skip the tests."""

from __future__ import annotations

import importlib.util
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location("ci_changed", ROOT / "scripts" / "ci_changed.py")
assert _spec is not None and _spec.loader is not None
ci_changed = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(ci_changed)


@pytest.mark.parametrize(
    "paths",
    [
        ["README.md"],
        ["LICENSE"],
        ["docs/a/b.md"],
        ["README.md", "docs/usage.md", "LICENSE"],
    ],
)
def test_documentation_only(paths: list[str]) -> None:
    assert ci_changed.is_docs_only(paths)


@pytest.mark.parametrize(
    "paths",
    [
        [],
        ["crates/schema.md"],
        ["spec/fingerprint.md"],
        ["sdks/typescript/README.md"],
        ["docs/page-script.js"],
        ["docs/x.png"],
        ["docs/run.sh"],
        ["docs/x.py"],
        ["docs/../crates/a.md"],
        ["README.md", "page-script/page-script.js"],
        ["docs"],
        ["docs.md"],
    ],
)
def test_everything_else_runs_the_tests(paths: list[str]) -> None:
    assert not ci_changed.is_docs_only(paths)


def git(repo: Path, *args: str) -> str:
    result = subprocess.run(["git", *args], cwd=repo, capture_output=True, text=True, check=True)
    return result.stdout.strip()


@pytest.fixture
def repo(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    git(tmp_path, "init", "-q", ".")
    git(tmp_path, "config", "user.email", "a@b")
    git(tmp_path, "config", "user.name", "t")
    git(tmp_path, "config", "commit.gpgsign", "false")
    (tmp_path / "page-script").mkdir()
    (tmp_path / "docs").mkdir()
    (tmp_path / "page-script" / "page-script.js").write_text("function collect(){ return 1 }\n// pad\n// pad\n// pad\n")
    (tmp_path / "README.md").write_text("hi\n")
    git(tmp_path, "add", "-A")
    git(tmp_path, "commit", "-qm", "base")
    monkeypatch.chdir(tmp_path)
    return tmp_path


def commit(repo: Path, message: str = "change") -> str:
    git(repo, "add", "-A")
    git(repo, "commit", "-qm", message)
    return git(repo, "rev-parse", "HEAD")


def test_a_move_into_docs_is_code(repo: Path) -> None:
    base = git(repo, "rev-parse", "HEAD")
    git(repo, "mv", "page-script/page-script.js", "docs/page-script.js")
    commit(repo)
    assert ci_changed.changed_paths(base) == ["docs/page-script.js", "page-script/page-script.js"]
    assert ci_changed.decide("pull_request", base, "") is True


def test_a_markdown_file_in_a_crate_is_code(repo: Path) -> None:
    base = git(repo, "rev-parse", "HEAD")
    (repo / "crates").mkdir()
    (repo / "crates" / "schema.md").write_text("bad\n")
    commit(repo)
    assert ci_changed.decide("pull_request", base, "") is True


def test_a_documentation_edit_skips_the_tests(repo: Path) -> None:
    base = git(repo, "rev-parse", "HEAD")
    (repo / "docs" / "usage.md").write_text("new page\n")
    (repo / "README.md").write_text("changed\n")
    commit(repo)
    assert ci_changed.decide("pull_request", base, "") is False


def test_names_with_spaces_are_kept(repo: Path) -> None:
    base = git(repo, "rev-parse", "HEAD")
    (repo / "docs" / "my page.md").write_text("x\n")
    (repo / "src u.rs").write_text("y\n")
    commit(repo)
    assert ci_changed.changed_paths(base) == ["docs/my page.md", "src u.rs"]
    assert ci_changed.decide("push", "", base) is True


def test_push_uses_the_before_commit(repo: Path) -> None:
    before = git(repo, "rev-parse", "HEAD")
    (repo / "docs" / "usage.md").write_text("x\n")
    commit(repo)
    assert ci_changed.decide("push", "", before) is False


@pytest.mark.parametrize("event", ["push", "schedule", "workflow_dispatch"])
def test_a_missing_base_tests_everything(repo: Path, event: str) -> None:
    assert ci_changed.decide(event, "", "") is True
    assert ci_changed.decide(event, "", "0" * 40) is True
    assert ci_changed.decide(event, "", "1" * 40) is True


def test_main_writes_the_output_file(repo: Path, monkeypatch: pytest.MonkeyPatch, tmp_path_factory) -> None:
    base = git(repo, "rev-parse", "HEAD")
    (repo / "docs" / "usage.md").write_text("x\n")
    commit(repo)
    out = tmp_path_factory.mktemp("out") / "output"
    monkeypatch.setenv("GITHUB_OUTPUT", str(out))
    monkeypatch.setenv("EVENT_NAME", "pull_request")
    monkeypatch.setenv("PR_BASE_SHA", base)
    monkeypatch.setenv("PUSH_BEFORE", "")
    assert ci_changed.main() == 0
    assert out.read_text() == "code=false\n"
