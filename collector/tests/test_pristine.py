"""The table of the browser's original built-ins, and the three places its text lives.

collector/pristine.js is the source. Its text is copied into collector.js (which has to be one
self-contained expression) and into the Playwright driver (which runs it before the page's own
scripts). These tests keep the copies identical and keep collector.js from calling a page
object's methods directly. After editing pristine.js run

    uv run python collector/tests/test_pristine.py --write

which copies it into the other two.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PRISTINE = ROOT / "collector" / "pristine.js"
COLLECTOR = ROOT / "collector" / "collector.js"
PLAYWRIGHT_DRIVER = ROOT / "drivers" / "playwright-py" / "src" / "traceqa_driver_playwright" / "driver.py"

IN_COLLECTOR = re.compile(r"(  // BEGIN pristine\.js\n  var capture = )(.*?)(;\n  // END pristine\.js)", re.DOTALL)
IN_DRIVER = re.compile(r'(# BEGIN pristine\.js\nPRISTINE_JS = r"""\n)(.*?)(\n"""\n# END pristine\.js)', re.DOTALL)


def source() -> str:
    return PRISTINE.read_text(encoding="utf-8").strip()


def embedded(path: Path, pattern: re.Pattern[str]) -> str:
    match = pattern.search(path.read_text(encoding="utf-8"))
    assert match, f"{path.name} has no BEGIN/END pristine.js block"
    return match.group(2).strip()


def test_pristine_has_no_triple_quotes_or_backslashes() -> None:
    assert '"""' not in source() and "\\" not in source()


def test_collector_carries_the_same_capture_code() -> None:
    assert embedded(COLLECTOR, IN_COLLECTOR) == source(), "run: uv run python collector/tests/test_pristine.py --write"


def test_playwright_driver_carries_the_same_capture_code() -> None:
    assert embedded(PLAYWRIGHT_DRIVER, IN_DRIVER) == source(), (
        "run: uv run python collector/tests/test_pristine.py --write"
    )


# Methods of built-in objects. A name here that the collector calls on a page object instead of
# through N is a way for a page to change what is read.
NATIVE_METHODS = {
    "push",
    "unshift",
    "concat",
    "indexOf",
    "join",
    "map",
    "slice",
    "trim",
    "toLowerCase",
    "charCodeAt",
    "getAttribute",
    "hasAttribute",
    "matches",
    "closest",
    "querySelector",
    "querySelectorAll",
    "getRootNode",
    "getBoundingClientRect",
    "checkVisibility",
    "contains",
    "elementFromPoint",
    "getElementById",
    "getPropertyValue",
    "forEach",
    "replace",
    "split",
    "test",
    "match",
    "charAt",
    "has",
    "add",
    "get",
    "set",
    "keys",
    "toString",
    "substring",
    "substr",
    "trimStart",
    "trimEnd",
    "startsWith",
    "endsWith",
    "includes",
    "filter",
    "reduce",
    "some",
    "every",
    "find",
    "sort",
    "reverse",
    "splice",
    "shift",
    "pop",
    "item",
    "call",
    "apply",
    "bind",
    "exec",
    "escape",
    "round",
    "max",
    "min",
    "floor",
    "ceil",
    "abs",
    "getComputedStyle",
    "hasOwnProperty",
}


def collector_body() -> str:
    """collector.js without comments, strings and the pristine block."""
    text = COLLECTOR.read_text(encoding="utf-8")
    text = IN_COLLECTOR.sub(r"\1;\3", text)
    text = re.sub(r"//[^\n]*", "", text)
    text = re.sub(r'"(?:[^"\\\n]|\\.)*"', '""', text)
    return re.sub(r"'(?:[^'\\\n]|\\.)*'", "''", text)


def test_the_collector_calls_built_ins_only_through_the_table() -> None:
    calls = re.findall(r"(\b[\w$]+)\.(\w+)\(", collector_body())
    direct = sorted({f"{owner}.{name}" for owner, name in calls if name in NATIVE_METHODS and owner != "N"})
    assert direct == [], f"call these through N (the table in pristine.js): {direct}"


def test_every_table_function_the_collector_uses_is_in_the_table() -> None:
    used = set(re.findall(r"\bN\.(\w+)", collector_body()))
    defined = set(re.findall(r'(?:method|getter|staticFn)\("(\w+)"', source())) | set(
        re.findall(r"\bT\.(\w+) =", source())
    )
    assert used - defined == set(), f"collector.js uses N.{sorted(used - defined)}, which pristine.js does not define"


def write() -> None:
    text = source()
    for path, pattern in ((COLLECTOR, IN_COLLECTOR), (PLAYWRIGHT_DRIVER, IN_DRIVER)):
        current = path.read_text(encoding="utf-8")
        # Files in this repository use LF on every system.
        path.write_text(pattern.sub(lambda m: m.group(1) + text + m.group(3), current), encoding="utf-8", newline="\n")


if __name__ == "__main__":
    if "--write" not in sys.argv:
        sys.exit(__doc__)
    write()
