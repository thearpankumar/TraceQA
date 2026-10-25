"""Reads the expectations that test pages state on their own elements.

    <button id="save" data-expect="role=button; name=Save">Save</button>
    <div id="text" data-expect="absent">Plain text</div>

Used by page-script/tests/test_cases.py (the collector alone) and tests/test_patterns.py (the
whole engine). Each passes the keys it can check, so a page cannot ask for one it cannot.
"""

from __future__ import annotations

from collections.abc import Iterable
from dataclasses import dataclass
from html.parser import HTMLParser
from pathlib import Path


@dataclass(frozen=True)
class Case:
    page: str
    id: str
    expect: dict[str, str]  # empty means the element must be absent
    gap: str | None


def parse_expectation(text: str, keys: Iterable[str]) -> dict[str, str]:
    known = set(keys)
    if text.strip() == "absent":
        return {}
    expect = {}
    for part in text.split(";"):
        if not part.strip():
            continue
        key, separator, value = part.partition("=")
        key = key.strip()
        if not separator or key not in known:
            raise ValueError(f"unknown expectation {part.strip()!r}. Known keys: {', '.join(sorted(known))}")
        value = value.strip()
        expect[key] = ",".join(sorted(value.split(","))) if key == "signals" else value
    return expect


class _Reader(HTMLParser):
    def __init__(self, page: str, keys: Iterable[str]) -> None:
        super().__init__()
        self.page = page
        self.keys = list(keys)
        self.cases: list[Case] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        found = dict(attrs)
        if "data-expect" not in found:
            return
        if not found.get("id"):
            raise ValueError(f"{self.page}: a <{tag}> with data-expect has no id")
        expect = parse_expectation(found["data-expect"] or "", self.keys)
        self.cases.append(Case(self.page, found["id"], expect, found.get("data-known-gap")))


def read_cases(folder: Path, keys: Iterable[str]) -> list[Case]:
    """Every case on every page in `folder`, in page and document order."""
    keys = list(keys)
    cases = []
    for path in sorted(folder.glob("*.html")):
        reader = _Reader(path.name, keys)
        reader.feed(path.read_text(encoding="utf-8"))
        cases.extend(reader.cases)
    ids = [(case.page, case.id) for case in cases]
    duplicates = {item for item in ids if ids.count(item) > 1}
    if duplicates:
        raise ValueError(f"case ids must be unique within a page: {sorted(duplicates)}")
    return cases
