"""The one-line installers, scripts/install.sh and scripts/install.ps1, against a fake release.

A local HTTP server plays GitHub: it serves a release archive, its SHA256SUMS.txt and the API answer that
names the newest release. Every install goes into a temporary folder with a temporary HOME. install.sh runs
on Linux and macOS, install.ps1 on Windows; the test that lets install.ps1 change the user PATH runs only in
CI, on a throwaway runner, and puts the old PATH back.
"""

from __future__ import annotations

import functools
import hashlib
import io
import json
import os
import platform
import shutil
import subprocess
import sys
import tarfile
import threading
import zipfile
from collections.abc import Iterator
from dataclasses import dataclass
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[1]
INSTALL_SH = ROOT / "scripts" / "install.sh"
INSTALL_PS1 = ROOT / "scripts" / "install.ps1"
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)
TAG = "v9.9.9"
RELEASES_API = "api/repos/thearpankumar/TraceQA/releases"
STUB = b'#!/bin/sh\necho "traceqa 9.9.9"\n'

unix_only = pytest.mark.skipif(
    sys.platform == "win32", reason="install.sh is for Linux and macOS; install.ps1 is tested on Windows"
)
windows_only = pytest.mark.skipif(
    sys.platform != "win32", reason="install.ps1 is for Windows; install.sh is tested on Linux and macOS"
)


def release_platform() -> str | None:
    """The platform name in the archive the installer will ask for here, or None when there is no build."""
    machine = platform.machine().lower()
    if sys.platform == "win32":
        return "windows-x86_64"
    if sys.platform.startswith("linux") and machine in ("x86_64", "amd64"):
        return "linux-x86_64"
    if sys.platform == "darwin":
        if machine == "arm64":
            return "macos-arm64"
        rosetta = subprocess.run(["sysctl", "-n", "hw.optional.arm64"], capture_output=True, text=True, check=False)
        return "macos-arm64" if rosetta.stdout.strip() == "1" else None
    return None


class QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: Any) -> None:
        pass


@dataclass
class FakeGitHub:
    root: Path
    url: str

    @property
    def download_base(self) -> str:
        return f"{self.url}/download"

    @property
    def api(self) -> str:
        return f"{self.url}/api"

    def publish(self, archive: str, data: bytes, checksum: str | None = None) -> None:
        folder = self.root / "download" / TAG
        folder.mkdir(parents=True, exist_ok=True)
        (folder / archive).write_bytes(data)
        digest = checksum or hashlib.sha256(data).hexdigest()
        (folder / "SHA256SUMS.txt").write_text(f"{digest}  {archive}\n", encoding="utf-8")

    def list_releases(self, releases: list[dict[str, Any]], indent: int | None = None) -> None:
        path = self.root / RELEASES_API
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(releases, indent=indent), encoding="utf-8")


@pytest.fixture
def github(tmp_path: Path) -> Iterator[FakeGitHub]:
    root = tmp_path / "www"
    root.mkdir()
    server = ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(QuietHandler, directory=str(root)))
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield FakeGitHub(root, f"http://127.0.0.1:{server.server_address[1]}")
    finally:
        server.shutdown()
        server.server_close()


def clean_env(**extra: str) -> dict[str, str]:
    """This process's environment without any setting the installers read, plus `extra`."""
    read = ("TRACEQA_", "XDG_BIN_HOME", "XDG_CONFIG_HOME", "ZDOTDIR")
    env = {key: value for key, value in os.environ.items() if not key.startswith(read)}
    env.update(extra)
    return env


# ---- install.sh ------------------------------------------------------------------------------


def tar_release(binary: bytes) -> bytes:
    """A release archive laid out as .github/workflows/release.yml packs it."""
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
        for name, data, mode in (
            ("traceqa", binary, 0o755),
            ("README.md", b"readme\n", 0o644),
            ("LICENSE", b"license\n", 0o644),
        ):
            info = tarfile.TarInfo(name)
            info.size = len(data)
            info.mode = mode
            archive.addfile(info, io.BytesIO(data))
    return buffer.getvalue()


def sh_archive() -> str:
    name = release_platform()
    if name is None:
        pytest.skip(f"there is no prebuilt release for {platform.system()} {platform.machine()}")
    return f"traceqa-{TAG}-{name}.tar.gz"


def run_sh(tmp_path: Path, *args: str, stdin: bytes | None = None, **env: str) -> subprocess.CompletedProcess[str]:
    home = tmp_path / "home"
    home.mkdir(exist_ok=True)
    command = ["sh", "-s", "--", *args] if stdin is not None else ["sh", str(INSTALL_SH), *args]
    return subprocess.run(
        command,
        input=stdin.decode("utf-8") if stdin is not None else None,
        env=clean_env(**{"HOME": str(home), "SHELL": "/bin/sh", **env}),
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
    )


@unix_only
def test_sh_installs_a_pinned_release(tmp_path: Path, github: FakeGitHub) -> None:
    github.publish(sh_archive(), tar_release(STUB))
    target = tmp_path / "bin"
    run = run_sh(
        tmp_path,
        TRACEQA_VERSION=TAG,
        TRACEQA_DOWNLOAD_BASE=github.download_base,
        TRACEQA_INSTALL_DIR=str(target),
        TRACEQA_NO_MODIFY_PATH="1",
    )
    assert run.returncode == 0, run.stderr
    installed = target / "traceqa"
    assert os.access(installed, os.X_OK)
    assert subprocess.run([str(installed)], capture_output=True, text=True, check=True).stdout == "traceqa 9.9.9\n"
    assert "checksum verified" in run.stdout
    assert "installed traceqa 9.9.9" in run.stdout
    # Told about PATH, but no profile was written.
    assert f'export PATH="{target}:$PATH"' in run.stdout
    assert list((tmp_path / "home").iterdir()) == []
    assert not list(target.glob(".traceqa.new"))


@unix_only
@pytest.mark.parametrize("indent", [None, 2], ids=["minified", "indented"])
def test_sh_finds_the_newest_release_through_the_api(tmp_path: Path, github: FakeGitHub, indent: int | None) -> None:
    github.publish(sh_archive(), tar_release(STUB))
    # A pre-release, which /releases/latest would not return.
    github.list_releases([{"url": "u", "id": 1, "tag_name": TAG, "name": "TraceQA 9.9.9", "prerelease": True}], indent)
    run = run_sh(
        tmp_path,
        TRACEQA_GITHUB_API=github.api,
        TRACEQA_DOWNLOAD_BASE=github.download_base,
        TRACEQA_INSTALL_DIR=str(tmp_path / "bin"),
        TRACEQA_NO_MODIFY_PATH="1",
    )
    assert run.returncode == 0, run.stderr
    assert f"downloading traceqa-{TAG}-" in run.stdout


@unix_only
def test_sh_says_so_when_there_is_no_release(tmp_path: Path, github: FakeGitHub) -> None:
    sh_archive()
    github.list_releases([])
    run = run_sh(tmp_path, TRACEQA_GITHUB_API=github.api, TRACEQA_INSTALL_DIR=str(tmp_path / "bin"))
    assert run.returncode == 1
    assert "TraceQA has no release yet" in run.stderr
    assert not (tmp_path / "bin").exists()


@unix_only
def test_sh_a_wrong_checksum_installs_nothing(tmp_path: Path, github: FakeGitHub) -> None:
    github.publish(sh_archive(), tar_release(STUB), checksum="0" * 64)
    run = run_sh(
        tmp_path,
        TRACEQA_VERSION=TAG,
        TRACEQA_DOWNLOAD_BASE=github.download_base,
        TRACEQA_INSTALL_DIR=str(tmp_path / "bin"),
    )
    assert run.returncode == 1
    assert "checksum mismatch" in run.stderr
    assert not (tmp_path / "bin").exists()


@unix_only
def test_sh_refuses_plain_http_to_another_machine(tmp_path: Path) -> None:
    sh_archive()
    run = run_sh(
        tmp_path,
        TRACEQA_VERSION=TAG,
        TRACEQA_DOWNLOAD_BASE="http://example.com/download",
        TRACEQA_INSTALL_DIR=str(tmp_path / "bin"),
    )
    assert run.returncode == 1
    assert "only https addresses are allowed" in run.stderr


@unix_only
def test_sh_adds_the_folder_to_path_once(tmp_path: Path, github: FakeGitHub) -> None:
    github.publish(sh_archive(), tar_release(STUB))
    target = tmp_path / "bin"
    for _ in range(2):
        run = run_sh(
            tmp_path,
            TRACEQA_VERSION=TAG,
            TRACEQA_DOWNLOAD_BASE=github.download_base,
            TRACEQA_INSTALL_DIR=str(target),
            SHELL="/bin/bash",
        )
        assert run.returncode == 0, run.stderr
    profiles = [".bashrc", ".bash_profile"] if sys.platform == "darwin" else [".bashrc"]
    assert sorted(p.name for p in (tmp_path / "home").iterdir()) == sorted(profiles)
    for profile in profiles:
        text = (tmp_path / "home" / profile).read_text(encoding="utf-8")
        assert text.count("# >>> traceqa installer >>>") == 1, profile
        assert f'export PATH="{target}:$PATH"' in text


@unix_only
def test_sh_runs_from_a_pipe_with_arguments(tmp_path: Path, github: FakeGitHub) -> None:
    github.publish(sh_archive(), tar_release(STUB))
    target = tmp_path / "bin"
    # As `curl ... | sh -s -- ARGS`; the tag without its v is accepted too.
    run = run_sh(
        tmp_path,
        "--version",
        TAG.removeprefix("v"),
        "--install-dir",
        str(target),
        "--no-modify-path",
        stdin=INSTALL_SH.read_bytes(),
        TRACEQA_DOWNLOAD_BASE=github.download_base,
    )
    assert run.returncode == 0, run.stderr
    assert (target / "traceqa").is_file()


# ---- install.ps1 -----------------------------------------------------------------------------

# Windows always has powershell; elsewhere the tests are skipped as Windows-only, whatever is installed.
POWERSHELLS = [shell for shell in ("powershell", "pwsh") if shutil.which(shell)] or ["powershell"]


def user_path() -> str | None:
    """The user PATH as the registry holds it, unexpanded."""
    assert sys.platform == "win32"
    import winreg

    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, "Environment") as key:
        try:
            return str(winreg.QueryValueEx(key, "Path")[0])
        except FileNotFoundError:
            return None


def restore_user_path(value: str | None) -> None:
    assert sys.platform == "win32"
    import winreg

    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, "Environment", 0, winreg.KEY_SET_VALUE) as key:
        if value is None:
            winreg.DeleteValue(key, "Path")
        else:
            winreg.SetValueEx(key, "Path", 0, winreg.REG_EXPAND_SZ, value)


def zip_release() -> bytes:
    if not BINARY.is_file():
        pytest.skip("build the engine first: cargo build")
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.write(BINARY, "traceqa.exe")
        archive.writestr("README.md", "readme\n")
        archive.writestr("LICENSE", "license\n")
    return buffer.getvalue()


def run_ps1(shell: str, *args: str, **env: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [shell, "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(INSTALL_PS1), *args],
        env=clean_env(**env),
        capture_output=True,
        text=True,
        timeout=180,
        check=False,
    )


@windows_only
@pytest.mark.parametrize("shell", POWERSHELLS)
def test_ps1_installs_a_pinned_release_and_leaves_path_alone(tmp_path: Path, github: FakeGitHub, shell: str) -> None:
    github.publish(f"traceqa-{TAG}-windows-x86_64.zip", zip_release())
    before = user_path()
    target = tmp_path / "bin"
    run = run_ps1(
        shell,
        TRACEQA_VERSION=TAG,
        TRACEQA_DOWNLOAD_BASE=github.download_base,
        TRACEQA_INSTALL_DIR=str(target),
        TRACEQA_NO_MODIFY_PATH="1",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert "checksum verified" in run.stdout
    assert "installed traceqa" in run.stdout
    assert (target / "traceqa.exe").is_file()
    assert user_path() == before


@windows_only
@pytest.mark.parametrize("shell", POWERSHELLS)
def test_ps1_finds_the_newest_release_through_the_api(tmp_path: Path, github: FakeGitHub, shell: str) -> None:
    github.publish(f"traceqa-{TAG}-windows-x86_64.zip", zip_release())
    github.list_releases([{"url": "u", "id": 1, "tag_name": TAG, "prerelease": True}])
    run = run_ps1(
        shell,
        TRACEQA_GITHUB_API=github.api,
        TRACEQA_DOWNLOAD_BASE=github.download_base,
        TRACEQA_INSTALL_DIR=str(tmp_path / "bin"),
        TRACEQA_NO_MODIFY_PATH="1",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert (tmp_path / "bin" / "traceqa.exe").is_file()


@windows_only
@pytest.mark.parametrize("shell", POWERSHELLS)
def test_ps1_a_wrong_checksum_installs_nothing(tmp_path: Path, github: FakeGitHub, shell: str) -> None:
    github.publish(f"traceqa-{TAG}-windows-x86_64.zip", zip_release(), checksum="0" * 64)
    run = run_ps1(
        shell,
        TRACEQA_VERSION=TAG,
        TRACEQA_DOWNLOAD_BASE=github.download_base,
        TRACEQA_INSTALL_DIR=str(tmp_path / "bin"),
        TRACEQA_NO_MODIFY_PATH="1",
    )
    assert run.returncode == 1
    assert "checksum mismatch" in run.stdout
    assert not (tmp_path / "bin").exists()


@pytest.mark.skipif(shutil.which("pwsh") is None, reason="needs PowerShell 7 to take its module path from")
@windows_only
def test_ps1_works_in_windows_powershell_started_from_powershell_7(tmp_path: Path, github: FakeGitHub) -> None:
    # Windows PowerShell started from PowerShell 7 (or from a shell PowerShell 7 started) inherits PowerShell 7's
    # PSModulePath, loads the wrong Utility module and has no Get-FileHash; CI's test step is such a shell.
    pwsh_modules = subprocess.run(
        ["pwsh", "-NoProfile", "-Command", "$env:PSModulePath"], capture_output=True, text=True, check=True
    ).stdout.strip()
    github.publish(f"traceqa-{TAG}-windows-x86_64.zip", zip_release())
    target = tmp_path / "bin"
    run = run_ps1(
        "powershell",
        PSModulePath=pwsh_modules,
        TRACEQA_VERSION=TAG,
        TRACEQA_DOWNLOAD_BASE=github.download_base,
        TRACEQA_INSTALL_DIR=str(target),
        TRACEQA_NO_MODIFY_PATH="1",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert "checksum verified" in run.stdout
    assert (target / "traceqa.exe").is_file()


@windows_only
@pytest.mark.parametrize("shell", POWERSHELLS)
def test_ps1_through_iex_reports_a_failure_without_closing_the_session(tmp_path: Path, shell: str) -> None:
    # The address is refused before any download, and the command after iex still runs.
    command = f"Get-Content -Raw '{INSTALL_PS1}' | Invoke-Expression; 'session still open'"
    run = subprocess.run(
        [shell, "-NoProfile", "-Command", command],
        env=clean_env(
            TRACEQA_VERSION=TAG,
            TRACEQA_DOWNLOAD_BASE="http://example.com/download",
            TRACEQA_INSTALL_DIR=str(tmp_path / "bin"),
            TRACEQA_NO_MODIFY_PATH="1",
        ),
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
    )
    assert "only https addresses are allowed" in run.stdout
    assert "session still open" in run.stdout


# The mark nearest the function is checked first, so Linux and macOS report the Windows-only reason.
@pytest.mark.skipif(os.environ.get("GITHUB_ACTIONS") != "true", reason="changes the user PATH: runs in CI only")
@windows_only
def test_ps1_adds_the_folder_to_the_user_path_once(tmp_path: Path, github: FakeGitHub) -> None:
    github.publish(f"traceqa-{TAG}-windows-x86_64.zip", zip_release())
    before = user_path()
    target = tmp_path / "bin"
    try:
        for _ in range(2):
            run = run_ps1(
                POWERSHELLS[0],
                TRACEQA_VERSION=TAG,
                TRACEQA_DOWNLOAD_BASE=github.download_base,
                TRACEQA_INSTALL_DIR=str(target),
            )
            assert run.returncode == 0, run.stdout + run.stderr
        after = user_path() or ""
        assert [entry for entry in after.split(";") if entry == str(target)] == [str(target)]
        # The entries that were there are kept as they were, unexpanded.
        assert after.endswith(before or "")
    finally:
        restore_user_path(before)
