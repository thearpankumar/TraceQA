"""The commands in the README's Development and testing section work as written on every platform."""

from __future__ import annotations

import re
import shlex
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]


def development_commands() -> list[str]:
    text = (ROOT / "README.md").read_text(encoding="utf-8")
    section = re.search(r"^## Development and testing\n(.*?)^## ", text, re.DOTALL | re.MULTILINE)
    assert section, "README.md has no Development and testing section"
    blocks = re.findall(r"^```sh\n(.*?)^```", section.group(1), re.DOTALL | re.MULTILINE)
    commands = [line for block in blocks for line in block.splitlines() if line.strip()]
    assert commands, "the section has no sh blocks"
    return commands


@pytest.mark.parametrize("command", development_commands())
def test_no_shell_specific_syntax(command: str) -> None:
    # `uv run npm` cannot find npm.cmd on Windows; ( ) subshells and && differ between PowerShell, cmd and sh;
    # a # comment is not one in cmd.
    for banned in ("uv run npm", "(", ")", "&&", "||", ";", "|", " #", "$("):
        assert banned not in command, f"{command!r} uses {banned!r}"


@pytest.mark.parametrize("command", development_commands())
def test_directories_named_by_a_command_exist(command: str) -> None:
    words = shlex.split(command)
    for flag in ("--prefix", "--directory"):
        if flag in words:
            target = words[words.index(flag) + 1]
            assert (ROOT / target).is_dir(), f"{command!r}: {target} is not a directory"
            assert not Path(target).is_absolute()
