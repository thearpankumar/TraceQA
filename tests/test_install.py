"""`traceqa install` and `traceqa doctor`, from an empty TraceQA folder to a working extraction.

The engine runs with the repository's drivers taken off PATH, so it can only succeed by
finding the copies `install` made. Installing from GitHub is not tested here: it would test
the branch on GitHub, not this checkout.
"""

from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)

pytestmark = [
    pytest.mark.skipif(not BINARY.is_file(), reason="build the engine first: cargo build"),
    pytest.mark.skipif(shutil.which("uv") is None, reason="traceqa install needs uv"),
]


def without_workspace_drivers(home: Path) -> dict[str, str]:
    """The environment of a user who has only the binary: no repository .venv on PATH."""
    env = {key: value for key, value in os.environ.items() if key not in ("VIRTUAL_ENV", "TRACEQA_DRIVERS")}
    venv = (ROOT / ".venv").resolve()
    parts = [p for p in env.get("PATH", "").split(os.pathsep) if p and not Path(p).resolve().is_relative_to(venv)]
    env["PATH"] = os.pathsep.join(parts)
    env["TRACEQA_HOME"] = str(home)
    return env


def traceqa(*args: str, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run([str(BINARY), *args], env=env, capture_output=True, text=True, encoding="utf-8", timeout=600)


def test_install_then_doctor_then_extract(shop_url: str, tmp_path: Path, run_folder_checker) -> None:
    home = tmp_path / "traceqa-home"
    env = without_workspace_drivers(home)

    before = traceqa("doctor", env=env)
    assert before.returncode == 1 and "No driver works" in before.stdout

    installed = traceqa("install", "--from", str(ROOT), "chromium", env=env)
    assert installed.returncode == 0, installed.stderr
    # From a checkout, the versions come from its uv.lock.
    assert (home / "constraints.txt").is_file()

    after = traceqa("doctor", env=env)
    assert after.returncode == 0, after.stdout
    assert "installed:" in after.stdout and "opens chromium" in after.stdout

    run = traceqa("extract", shop_url, "--out", str(tmp_path / "runs"), "--json", env=env)
    assert run.returncode == 0, run.stderr
    assert '"status": "completed"' in run.stdout
    (folder,) = (tmp_path / "runs").iterdir()
    run_folder_checker(folder)


def test_install_refuses_what_it_cannot_do(tmp_path: Path) -> None:
    env = without_workspace_drivers(tmp_path / "home")
    unknown = traceqa("install", "netscape", env=env)
    assert unknown.returncode == 1 and "unknown browser" in unknown.stderr
    not_a_checkout = traceqa("install", "--from", str(tmp_path), env=env)
    assert not_a_checkout.returncode == 1 and "not a TraceQA checkout" in not_a_checkout.stderr
    # Nothing was created for a refused install.
    assert not (tmp_path / "home").exists()
