"""docs/features.md names every command, MCP tool, findings rule and SDK export, so it cannot fall behind."""

from __future__ import annotations

import ast
import os
import re
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
FEATURES = (ROOT / "docs" / "features.md").read_text(encoding="utf-8")
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)


def section(title: str) -> str:
    """The text of one `## N. Title` section."""
    match = re.search(rf"^## \d+\. {re.escape(title)}\n(.*?)(?=^## |\Z)", FEATURES, re.DOTALL | re.MULTILINE)
    assert match, f"docs/features.md has no section {title!r}"
    return match.group(1)


def subcommands(*command: str) -> list[str]:
    """The names in the Commands section of `traceqa [command] --help`, or none when it has no subcommands."""
    out = subprocess.run(
        [str(BINARY), *command, "--help"], check=True, capture_output=True, text=True, encoding="utf-8"
    ).stdout
    if "Commands:\n" not in out:
        return []
    listed = out.split("Commands:\n", 1)[1].split("\n\n", 1)[0]
    names = [m.group(1) for line in listed.splitlines() if (m := re.match(r"^  (\S+)", line))]
    return [name for name in names if name != "help"]


@pytest.mark.skipif(not BINARY.is_file(), reason="build the engine first: cargo build")
def test_every_command_and_subcommand_is_listed() -> None:
    commands = subcommands()
    assert len(commands) >= 15
    for command in commands:
        assert re.search(rf"`traceqa {command}\b", FEATURES), f"traceqa {command}"
        for sub in subcommands(command):
            assert f"`traceqa {command} {sub}`" in FEATURES, f"traceqa {command} {sub}"


def test_every_mcp_tool_is_listed() -> None:
    source = (ROOT / "crates" / "traceqa-mcp" / "src" / "tools.rs").read_text(encoding="utf-8")
    listing = source.split("pub fn list(", 1)[1].split("\n}\n", 1)[0]
    tools = re.findall(r'^\s+"name": "([a-z_]+)",\s*$', listing, re.MULTILINE)
    assert len(tools) >= 8
    mcp = section("MCP server")
    for tool in tools:
        assert f"- **`{tool}`**" in mcp, tool


def test_every_findings_rule_is_listed() -> None:
    source = (ROOT / "crates" / "traceqa-findings" / "src" / "rules.rs").read_text(encoding="utf-8")
    listing = re.search(r"pub const RULES[^=]*=\s*\[(.*?)\];", source, re.DOTALL)
    assert listing, "rules.rs has no RULES list"
    rules = re.findall(r'"([a-z-]+)"', listing.group(1))
    assert len(rules) >= 8
    findings = section("Findings")
    for rule in rules:
        assert f"- **`{rule}`**" in findings, rule


def test_every_python_sdk_export_is_named() -> None:
    tree = ast.parse((ROOT / "sdks" / "python" / "src" / "traceqa" / "__init__.py").read_text(encoding="utf-8"))
    exported = next(
        ast.literal_eval(node.value)
        for node in tree.body
        if isinstance(node, ast.Assign) and any(getattr(t, "id", "") == "__all__" for t in node.targets)
    )
    for name in exported:
        assert re.search(rf"\b{name}\b", FEATURES), name


def test_every_typescript_sdk_value_export_is_named() -> None:
    index = (ROOT / "sdks" / "typescript" / "src" / "index.ts").read_text(encoding="utf-8")
    blocks = re.findall(r"^export \{(.*?)\}", index, re.DOTALL | re.MULTILINE)
    names = [n.strip() for block in blocks for n in block.split(",") if n.strip() and not n.strip().startswith("type ")]
    assert "Engine" in names
    for name in names:
        assert re.search(rf"\b{name}\b", FEATURES), name


def test_readme_links_to_the_features() -> None:
    assert "(docs/features.md)" in (ROOT / "README.md").read_text(encoding="utf-8")
