#!/usr/bin/env python3
"""Regenerate pretty-printed FIX examples in resources/example READMEs."""

from __future__ import annotations

import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FIXDECODER = ROOT / "target" / "debug" / "fixdecoder"
ANSI_RE = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")
GENERATED_BLOCK_RE = re.compile(
    r"<!-- regen-examples:start -->\n.*?<!-- regen-examples:end -->\n*",
    re.S,
)


@dataclass(frozen=True)
class ExampleReadme:
    path: Path
    fixlog: Path


EXAMPLE_READMES = (
    ExampleReadme(
        path=ROOT / "resources" / "examples" / "appendix_d" / "README.md",
        fixlog=ROOT / "resources" / "examples" / "appendix_d" / "all.fixlog",
    ),
    ExampleReadme(
        path=ROOT / "resources" / "examples" / "repeating_groups" / "README.md",
        fixlog=ROOT / "resources" / "examples" / "repeating_groups" / "all.fixlog",
    ),
)


def main() -> int:
    ensure_binary()
    for example in EXAMPLE_READMES:
        update_example_readme(example)
    print(
        f"Updated {len(EXAMPLE_READMES)} example README files "
        "with pretty-printed FIX output"
    )
    return 0


def ensure_binary() -> None:
    subprocess.run(
        ["cargo", "build", "--quiet", "--bin", "fixdecoder"],
        cwd=ROOT,
        check=True,
    )


def update_example_readme(example: ExampleReadme) -> None:
    if not example.path.exists():
        raise FileNotFoundError(f"README not found: {example.path}")
    if not example.fixlog.exists():
        raise FileNotFoundError(f"FIX log not found: {example.fixlog}")

    original = example.path.read_text()
    block = render_generated_block(example)
    if GENERATED_BLOCK_RE.search(original):
        updated = GENERATED_BLOCK_RE.sub(block, original)
    else:
        updated = append_block(original, block)
    example.path.write_text(updated)


def append_block(markdown: str, block: str) -> str:
    markdown = markdown.rstrip()
    return f"{markdown}\n\n{block}"


def render_generated_block(example: ExampleReadme) -> str:
    command = (
        "fixdecoder --fix=44 --style=plain --paging=never --colour=no "
        f"--nocounts --delimiter='|' {example.fixlog.relative_to(ROOT)}"
    )
    output = render_pretty_print(example.fixlog)
    return (
        "<!-- regen-examples:start -->\n"
        "\n"
        "## Pretty-Printed Messages\n"
        "\n"
        f"Generated from `{example.fixlog.relative_to(ROOT)}`. The aggregate log contains "
        f"{count_messages(example.fixlog)} FIX messages, and the output below pretty-prints all of them.\n"
        "\n"
        "```bash\n"
        f"$ {command}\n"
        f"{output}\n"
        "```\n"
        "\n"
        "<!-- regen-examples:end -->\n"
    )


def render_pretty_print(fixlog: Path) -> str:
    result = subprocess.run(
        [
            str(FIXDECODER),
            "--fix=44",
            "--style=plain",
            "--paging=never",
            "--colour=no",
            "--nocounts",
            "--delimiter=|",
            str(fixlog.relative_to(ROOT)),
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    output = f"{result.stdout}{result.stderr}"
    if result.returncode != 0:
        raise RuntimeError(f"failed to pretty-print {fixlog.relative_to(ROOT)}\n{output}")
    output = sanitise_output(output)
    return strip_file_banner(output)


def sanitise_output(output: str) -> str:
    output = ANSI_RE.sub("", output)
    output = output.replace("\x01", "|")
    output = output.replace(str(ROOT) + "/", "")
    output = output.replace(str(ROOT), ".")
    return "\n".join(line.rstrip() for line in output.rstrip().splitlines())


def strip_file_banner(output: str) -> str:
    lines = output.splitlines()
    if len(lines) >= 5 and lines[0].startswith("---") and lines[1].startswith("Filename: "):
        return "\n".join(lines[5:]).lstrip("\n")
    return output


def count_messages(fixlog: Path) -> int:
    return sum(1 for line in fixlog.read_text().splitlines() if line.startswith("8="))


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(str(exc), file=sys.stderr)
        raise SystemExit(1)
