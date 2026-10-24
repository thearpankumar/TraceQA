"""Writes crates/traceqa-cli/driver-requirements.txt, the pinned and hashed dependencies of the drivers.

`traceqa install` installs these with `uv pip install --require-hashes`, so the third-party
packages of a release are the ones in uv.lock, checked by sha256, and not whatever PyPI serves
that day. The file comes from uv.lock and must not be edited by hand:

    uv run scripts/gen_driver_requirements.py           # rewrite it
    uv run scripts/gen_driver_requirements.py --check   # fail when it no longer matches uv.lock

Run it again whenever uv.lock changes. tests/test_driver_requirements.py runs the check.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "crates" / "traceqa-cli" / "driver-requirements.txt"
# The drivers name every package they need, the protocol package included. --no-emit-workspace
# leaves out TraceQA's own packages, which `traceqa install` installs from the release commit.
DRIVERS = ["traceqa-driver-playwright", "traceqa-driver-webdriver"]


def export() -> str:
    uv = shutil.which("uv")
    if uv is None:
        sys.exit("gen_driver_requirements: uv is needed (https://docs.astral.sh/uv/)")
    with tempfile.TemporaryDirectory() as folder:
        out = Path(folder) / "requirements.txt"
        command = [uv, "export", "--frozen", "--no-dev", "--no-emit-workspace", "--no-header", "--quiet"]
        for driver in DRIVERS:
            command += ["--package", driver]
        # --no-header: the header would hold the path of this checkout.
        subprocess.run([*command, "--output-file", str(out)], cwd=ROOT, check=True)
        text = out.read_text(encoding="utf-8").replace("\r\n", "\n")
    if "--hash=sha256:" not in text:
        sys.exit("gen_driver_requirements: uv wrote no hashes")
    return text


def main() -> int:
    wanted = export()
    if "--check" in sys.argv[1:]:
        current = TARGET.read_text(encoding="utf-8").replace("\r\n", "\n") if TARGET.is_file() else ""
        if current != wanted:
            print(
                f"{TARGET.relative_to(ROOT)} does not match uv.lock. Run: uv run scripts/gen_driver_requirements.py",
                file=sys.stderr,
            )
            return 1
        return 0
    TARGET.write_text(wanted, encoding="utf-8", newline="\n")
    print(f"wrote {TARGET.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
