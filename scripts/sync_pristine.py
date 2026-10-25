"""Keep the copies of page-script/pristine.js equal to it.

    python3 scripts/sync_pristine.py            # rewrite the copies from page-script/pristine.js
    python3 scripts/sync_pristine.py --check    # exit 1 if a copy differs, change nothing

page-script/pristine.js is the one place to edit. Its text has to be copied into two other files:

- the block between `// BEGIN pristine.js` and `// END pristine.js` in page-script/page-script.js, which
  must stay one self-contained expression with no build step (a WebDriver browser has no way to run
  a script before the page's own, so the collector captures the built-ins itself), and
- the raw string PRISTINE_JS between `# BEGIN pristine.js` and `# END pristine.js` in the Playwright
  driver, which runs it as an init script before the page's scripts.

Never edit the text between those markers by hand. Standard library only, so it runs before any install.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PRISTINE = ROOT / "page-script" / "pristine.js"
COLLECTOR = ROOT / "page-script" / "page-script.js"
PLAYWRIGHT_DRIVER = ROOT / "web-drivers" / "playwright-py" / "src" / "traceqa_driver_playwright" / "driver.py"

IN_COLLECTOR = re.compile(r"(  // BEGIN pristine\.js\n  var capture = )(.*?)(;\n  // END pristine\.js)", re.DOTALL)
IN_DRIVER = re.compile(r'(# BEGIN pristine\.js\nPRISTINE_JS = r"""\n)(.*?)(\n"""\n# END pristine\.js)', re.DOTALL)

# Where each copy lives and how to find its block.
COPIES = (
    ("page-script/page-script.js", COLLECTOR, IN_COLLECTOR),
    ("the Playwright driver", PLAYWRIGHT_DRIVER, IN_DRIVER),
)


class SyncError(Exception):
    """The source or a copy cannot be used."""


def source(path: Path = PRISTINE) -> str:
    text = path.read_text(encoding="utf-8").strip()
    # The driver holds the text in a raw triple-quoted string: neither can appear in it.
    if '"""' in text or "\\" in text:
        raise SyncError(f"{path.name} must not contain a triple double quote or a backslash")
    return text


def embedded(path: Path, pattern: re.Pattern[str]) -> str:
    """The text currently between the markers of `path`."""
    match = pattern.search(path.read_text(encoding="utf-8"))
    if not match:
        raise SyncError(f"{path.name} has no BEGIN/END pristine.js block")
    return match.group(2).strip()


def drifted(pristine: Path = PRISTINE, copies: tuple[tuple[str, Path, re.Pattern[str]], ...] = COPIES) -> list[str]:
    """The names of the copies whose block differs from `pristine`."""
    text = source(pristine)
    return [name for name, path, pattern in copies if embedded(path, pattern) != text]


def write(pristine: Path = PRISTINE, copies: tuple[tuple[str, Path, re.Pattern[str]], ...] = COPIES) -> list[str]:
    """Rewrite every copy from `pristine`. Returns the names of the copies that changed."""
    text = source(pristine)
    changed = []
    for name, path, pattern in copies:
        current = path.read_text(encoding="utf-8")
        if not pattern.search(current):
            raise SyncError(f"{path.name} has no BEGIN/END pristine.js block")
        updated = pattern.sub(lambda m: m.group(1) + text + m.group(3), current, count=1)
        if updated != current:
            # Files in this repository use LF on every system.
            path.write_text(updated, encoding="utf-8", newline="\n")
            changed.append(name)
    return changed


def main(argv: list[str]) -> int:
    if set(argv) - {"--check"}:
        print(__doc__, file=sys.stderr)
        return 2
    try:
        if "--check" in argv:
            stale = drifted()
            if stale:
                print(
                    f"page-script/pristine.js and {', '.join(stale)} differ. Run: python3 scripts/sync_pristine.py",
                    file=sys.stderr,
                )
                return 1
            return 0
        for name in write():
            print(f"updated {name}", file=sys.stderr)
    except SyncError as error:
        print(f"sync_pristine: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
