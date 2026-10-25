"""`traceqa login`: what it refuses before opening a browser, and how it saves the state.

The state file holds session keys. It is written to a temporary file that only its owner can
open, and moved into place, so it is never readable by others, not even for a moment, and a
failure leaves nothing behind. A bad `--save` is reported before a browser opens, so nobody
signs in just to lose the result. Usage mistakes exit with 2, as `extract` does.
"""

from __future__ import annotations

import importlib.util
import json
import os
import stat
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)
NO_DRIVER = "traceqa-no-such-driver"


def _load(name: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / "tests" / "support" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module  # a dataclass in the module looks itself up here
    spec.loader.exec_module(module)
    return module


interrupt = _load("interrupt")

pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="build the engine first: cargo build")


def traceqa(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(BINARY), *args], check=False, capture_output=True, text=True, encoding="utf-8", timeout=180
    )


def test_a_scheme_that_is_not_http_exits_2(tmp_path: Path) -> None:
    run = traceqa("login", "ftp://example.com/", "--save", str(tmp_path / "s.json"), "--driver", NO_DRIVER)
    assert run.returncode == 2, run.stderr
    assert "http or https" in run.stderr
    assert not (tmp_path / "s.json").exists()


@pytest.mark.parametrize(
    "url",
    ["https://user:pass@example.com/", "https://user@example.com/", "http://a\tb:c@example.com/", "https://u:p@h/"],
)
def test_an_address_with_a_password_in_it_exits_2(tmp_path: Path, url: str) -> None:
    run = traceqa("login", url, "--save", str(tmp_path / "s.json"), "--driver", NO_DRIVER)
    assert run.returncode == 2, run.stderr
    assert "--storage-state" in run.stderr
    # The password the person typed is not echoed back.
    assert "pass@" not in run.stderr and "user" not in run.stderr.replace("user name", "")


def test_an_at_sign_after_the_host_is_fine(tmp_path: Path) -> None:
    run = traceqa(
        "login", "https://example.com/path?next=a@b#x@y", "--save", str(tmp_path / "s.json"), "--driver", NO_DRIVER
    )
    # It gets as far as looking for the driver, which does not exist.
    assert run.returncode == 1, run.stderr
    assert "not found" in run.stderr or NO_DRIVER in run.stderr or "driver" in run.stderr


def test_a_missing_folder_is_reported_before_any_driver_starts(tmp_path: Path) -> None:
    run = traceqa(
        "login", "https://example.com/", "--save", str(tmp_path / "missing" / "s.json"), "--driver", NO_DRIVER
    )
    assert run.returncode == 2, run.stderr
    assert "--save" in run.stderr and "does not exist" in run.stderr
    assert NO_DRIVER not in run.stderr, "the driver must not even be looked for"


def test_a_directory_is_reported_before_any_driver_starts(tmp_path: Path) -> None:
    run = traceqa("login", "https://example.com/", "--save", str(tmp_path), "--driver", NO_DRIVER)
    assert run.returncode == 2, run.stderr
    assert "--save" in run.stderr and "directory" in run.stderr
    assert NO_DRIVER not in run.stderr


def test_a_failed_login_leaves_no_file_behind(tmp_path: Path) -> None:
    run = traceqa("login", "https://example.com/", "--save", str(tmp_path / "s.json"), "--driver", NO_DRIVER)
    assert run.returncode == 1
    assert list(tmp_path.iterdir()) == []


def test_headless_login_writes_a_parseable_state_only_the_owner_can_read(shop_url: str, tmp_path: Path) -> None:
    path = tmp_path / "state.json"
    path.write_text("old", encoding="utf-8")
    if os.name != "nt":
        path.chmod(0o644)
    run = traceqa("login", shop_url, "--save", str(path), "--headless")
    assert run.returncode == 0, run.stderr
    assert "owner-only" in run.stderr
    state = json.loads(path.read_text(encoding="utf-8"))
    assert set(state) >= {"cookies", "origins"}
    if os.name != "nt":
        assert stat.S_IMODE(path.stat().st_mode) == 0o600, "an existing file must end up owner-only too"
    assert [p.name for p in tmp_path.iterdir()] == ["state.json"], "no temporary file is left"


def test_ctrl_c_during_login_removes_the_temporary_file(trouble: str, tmp_path: Path) -> None:
    # The page never finishes loading, so the login waits for it: the temporary file exists the
    # whole time, and an interrupt must not leave it behind (nor create the destination).
    folder = tmp_path / "saved"
    folder.mkdir()
    command = [str(BINARY), "login", trouble + "hang", "--save", str(folder / "s.json"), "--headless"]
    result = interrupt.run_interrupted(command, ready={"glob": str(folder / ".s.json.tmp-*")}, delay=0.5)
    assert result.ready, f"the temporary file never appeared:\n{result.stderr}"
    assert result.returncode == 130, result.stderr
    assert "nothing was saved" in result.stderr
    assert list(folder.iterdir()) == [], "neither the destination nor a temporary file may remain"
    assert result.seconds is not None and result.seconds < 15, f"the login took {result.seconds} s to stop"
    assert result.left == [], f"left processes running: {result.left}"


def test_second_ctrl_c_during_login_kills_a_deaf_driver(trouble: str, tmp_path: Path) -> None:
    # The driver ignores signals and its input closing. A second interrupt must still end the
    # command at once, remove the temporary file and take the driver with it.
    folder = tmp_path / "saved"
    folder.mkdir()
    driver = f'"{sys.executable}" "{ROOT / "tests" / "support" / "deaf_driver.py"}"'
    command = [
        str(BINARY),
        "login",
        trouble + "hang",
        "--save",
        str(folder / "s.json"),
        "--headless",
        "--driver",
        driver,
    ]
    result = interrupt.run_interrupted(
        command, ready={"glob": str(folder / ".s.json.tmp-*")}, signals=2, delay=1.0, exit_timeout=20
    )
    assert result.ready, f"the temporary file never appeared:\n{result.stderr}"
    assert result.returncode == 130, result.stderr
    assert result.seconds is not None and result.seconds < 10, f"quit after {result.seconds} s"
    assert list(folder.iterdir()) == []
    assert result.left == [], f"the driver outlived the command: {result.left}"


@pytest.mark.skipif(os.name == "nt", reason="SIGTERM and SIGHUP cannot be sent to a Windows console command")
def test_sigterm_during_login_removes_the_temp_file(trouble: str, tmp_path: Path) -> None:
    folder = tmp_path / "saved"
    folder.mkdir()
    command = [str(BINARY), "login", trouble + "hang", "--save", str(folder / "s.json"), "--headless"]
    result = interrupt.run_interrupted(
        command,
        ready={"glob": str(folder / ".s.json.tmp-*")},
        delay=0.5,
        signal_name="SIGTERM",
        target="process",
    )
    assert result.ready, f"the temporary file never appeared:\n{result.stderr}"
    assert result.returncode == 130, result.stderr
    assert "nothing was saved" in result.stderr
    assert list(folder.iterdir()) == []
    assert result.left == [], f"left processes running: {result.left}"
