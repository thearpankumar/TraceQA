"""Decide whether a change touches anything that can break, for the Changes job of ci.yml.

    python3 scripts/ci_changed.py

Reads EVENT_NAME, PR_BASE_SHA and PUSH_BEFORE from the environment and writes `code=true` or
`code=false` to $GITHUB_OUTPUT (or stdout when that is not set). Only a change made entirely of
files on the documentation allow-list skips the tests: README.md, LICENSE and Markdown files under
docs/. Anything else, a rename out of code into docs/ included, runs everything. When the base
cannot be found (a new branch, a force push, a scheduled or manual run) it runs everything too.

Plain standard library: it runs from the checkout of the change, at the same trust level as ci.yml.
"""

from __future__ import annotations

import os
import subprocess
import sys
from collections.abc import Iterable
from pathlib import PurePosixPath

NULL_SHA = "0" * 40
DOC_FILES = {"README.md", "LICENSE"}


def is_docs_only(paths: Iterable[str]) -> bool:
    """True when every path is README.md, LICENSE or a Markdown file under docs/. No paths is not docs only."""
    paths = list(paths)
    return bool(paths) and all(_is_doc(path) for path in paths)


def _is_doc(path: str) -> bool:
    if path in DOC_FILES:
        return True
    parts = PurePosixPath(path).parts
    return len(parts) >= 2 and parts[0] == "docs" and ".." not in parts and path.endswith(".md")


def changed_paths(base: str) -> list[str] | None:
    """Every path that differs between base and HEAD, or None when base is not a commit here.

    --no-renames lists both the old and the new path of a move, so moving code into docs/ still
    shows the code path. -z keeps names with spaces or non-ASCII characters intact.
    """
    known = subprocess.run(["git", "cat-file", "-e", f"{base}^{{commit}}"], capture_output=True, check=False)  # noqa: S607
    if known.returncode != 0:
        return None
    out = subprocess.run(
        ["git", "diff", "--name-only", "--no-renames", "-z", base, "HEAD"],  # noqa: S607
        capture_output=True,
        check=True,
    ).stdout
    return [name.decode("utf-8", "surrogateescape") for name in out.split(b"\0") if name]


def decide(event_name: str, pr_base: str, push_before: str) -> bool:
    """True when the tests must run."""
    base = pr_base if event_name == "pull_request" else push_before
    if not base or base == NULL_SHA:
        print("Cannot tell what changed; testing everything.")
        return True
    paths = changed_paths(base)
    if paths is None:
        print("Cannot tell what changed; testing everything.")
        return True
    print("Changed files:")
    for path in paths:
        print(f"  {path}")
    if is_docs_only(paths):
        print("Documentation only; the tests are skipped.")
        return False
    return True


def main() -> int:
    code = decide(
        os.environ.get("EVENT_NAME", ""),
        os.environ.get("PR_BASE_SHA", ""),
        os.environ.get("PUSH_BEFORE", ""),
    )
    line = f"code={'true' if code else 'false'}\n"
    target = os.environ.get("GITHUB_OUTPUT")
    if target:
        with open(target, "a", encoding="utf-8") as handle:
            handle.write(line)
    else:
        sys.stdout.write(line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
