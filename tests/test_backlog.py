"""docs/backlog.md keeps its shape: every item has Why, Design and Acceptance, and the README links to it."""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BACKLOG = (ROOT / "docs" / "backlog.md").read_text(encoding="utf-8")


def items() -> dict[str, str]:
    parts = re.split(r"^### ", BACKLOG, flags=re.MULTILINE)[1:]
    return {part.splitlines()[0]: part for part in parts}


def label_text(body: str, label: str) -> str:
    match = re.search(rf"^\*\*{label}\.\*\*(.*?)(?=^\*\*\w+\.\*\*|^## |\Z)", body, re.DOTALL | re.MULTILINE)
    return match.group(1).strip() if match else ""


def test_every_item_has_why_design_and_acceptance() -> None:
    found = {title: body for title, body in items().items() if re.match(r"[A-Z]+-\d+:", title)}
    assert len(found) >= 30
    for title, body in found.items():
        for label in ("Why", "Design", "Acceptance"):
            assert label_text(body, label), f"{title}: empty or missing {label}"
        assert label_text(body, "Size")[:1] in {"S", "M", "L"}, f"{title}: no size"


def test_every_level_three_heading_is_an_item_or_an_explorer_overview() -> None:
    overview = {"What it is", "How it fits together"}
    for title in items():
        assert title in overview or re.match(r"[A-Z]+-\d+: ", title), title


def test_readme_links_to_the_backlog() -> None:
    assert "(docs/backlog.md)" in (ROOT / "README.md").read_text(encoding="utf-8")


def test_the_named_topics_are_present() -> None:
    for word in ("F6", "F7", "A3", "A7", "A9", "A10", "ANN", "X19", "X20"):
        assert re.search(rf"\b{word}\b", BACKLOG), word


def test_no_infrastructure_follow_ups_in_the_public_file() -> None:
    lowered = BACKLOG.lower()
    assert "self-hosted" not in lowered
    assert "runner" not in lowered
