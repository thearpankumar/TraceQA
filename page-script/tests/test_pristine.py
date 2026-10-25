"""The table of the browser's original built-ins, and the three places its text lives.

page-script/pristine.js is the source. Its text is copied into page-script.js (which has to be one
self-contained expression) and into the Playwright driver (which runs it before the page's own
scripts). scripts/sync_pristine.py makes and checks those copies: these tests run its check, and
keep page-script.js from calling a page object's methods directly. After editing pristine.js run

    python3 scripts/sync_pristine.py

which copies it into the other two.
"""

from __future__ import annotations

import importlib.util
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location("sync_pristine", ROOT / "scripts" / "sync_pristine.py")
assert _spec
assert _spec.loader
sync = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(sync)

COLLECTOR = sync.COLLECTOR
IN_COLLECTOR = sync.IN_COLLECTOR


def source() -> str:
    return sync.source()


def test_pristine_has_no_triple_quotes_or_backslashes() -> None:
    assert '"""' not in source() and "\\" not in source()


def test_the_copies_of_pristine_are_in_sync() -> None:
    assert sync.drifted() == [], "run: python3 scripts/sync_pristine.py"


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
    """page-script.js without comments, strings and the pristine block."""
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
    assert used - defined == set(), f"page-script.js uses N.{sorted(used - defined)}, which pristine.js does not define"
