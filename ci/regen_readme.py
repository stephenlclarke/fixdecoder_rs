#!/usr/bin/env python3
"""Regenerate README command-line option output examples."""

from __future__ import annotations

import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
README = ROOT / "README.md"
USAGE = ROOT / "resources" / "messages" / "usage_en.txt"
FIXDECODER = ROOT / "target" / "debug" / "fixdecoder"
EXAMPLE_DIR = ROOT / "target" / "readme-examples"

OPTION_BLOCK_RE = re.compile(
    r"<!-- regen-readme:start --option=.*? -->\n.*?<!-- regen-readme:end --option=.*? -->\n?",
    re.S,
)
BUILD_BLOCK_RE = re.compile(
    r"<!-- regen-readme:start --section=build-examples -->\n.*?"
    r"<!-- regen-readme:end --section=build-examples -->\n*",
    re.S,
)
USAGE_BLOCK_RE = re.compile(
    r"<!-- regen-readme:start --section=usage -->\n.*?"
    r"<!-- regen-readme:end --section=usage -->\n*",
    re.S,
)
HEADING_RE = re.compile(r"^### `(?P<option>--[A-Za-z0-9-]+)")
ANSI_RE = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")

SOH = "\x01"


@dataclass(frozen=True)
class ReadmeExample:
    option: str
    display_command: str
    args: tuple[str, ...]
    stdin: str | None = None
    max_lines: int = 24
    setup: str | None = None


@dataclass(frozen=True)
class BuildExample:
    display_command: str
    output: str


def fix_message(fields: list[tuple[str, str]]) -> str:
    body = "".join(f"{tag}={value}{SOH}" for tag, value in fields)
    prefix = f"8=FIX.4.4{SOH}9={len(body.encode('ascii'))}{SOH}"
    without_checksum = prefix + body
    checksum = sum(without_checksum.encode("ascii")) % 256
    return f"{without_checksum}10={checksum:03}{SOH}\n"


INVALID_FIX = f"8=FIX.4.4{SOH}9=005{SOH}10=000{SOH}\n"
HEARTBEAT_FIX = fix_message([("35", "0"), ("49", "BUY1"), ("56", "SELL1")])
ORDER_FIX = fix_message(
    [
        ("35", "D"),
        ("49", "BUY1"),
        ("56", "SELL1"),
        ("34", "1"),
        ("52", "20260425-10:00:00.000"),
        ("11", "CL-README-1"),
        ("55", "IBM"),
        ("54", "1"),
        ("60", "20260425-10:00:00.000"),
        ("38", "100"),
        ("40", "2"),
        ("44", "50.00"),
    ]
)
EXEC_FIX = fix_message(
    [
        ("35", "8"),
        ("49", "SELL1"),
        ("56", "BUY1"),
        ("34", "1"),
        ("52", "20260425-10:00:01.000"),
        ("37", "ORD-README-1"),
        ("11", "CL-README-1"),
        ("17", "EX-README-1"),
        ("150", "0"),
        ("39", "0"),
        ("55", "IBM"),
        ("54", "1"),
        ("38", "100"),
        ("32", "0"),
        ("151", "100"),
        ("14", "0"),
        ("6", "0"),
    ]
)


def prepare_secret_file() -> None:
    EXAMPLE_DIR.mkdir(parents=True, exist_ok=True)
    source = EXAMPLE_DIR / "orders.log"
    secret = EXAMPLE_DIR / "orders.secret.log"
    if secret.exists():
        secret.unlink()
    source.write_text(f"INFO {HEARTBEAT_FIX.rstrip()} tail\n")


EXAMPLES: tuple[ReadmeExample, ...] = (
    ReadmeExample(
        option="--xml",
        display_command="fixdecoder --xml resources/FIX44.xml --fix=44 --info",
        args=("--xml", "resources/FIX44.xml", "--fix=44", "--info"),
        max_lines=14,
    ),
    ReadmeExample(
        option="--fix",
        display_command="fixdecoder --fix=FIX50SP2 --info",
        args=("--fix=FIX50SP2", "--info"),
        max_lines=14,
    ),
    ReadmeExample(
        option="--info",
        display_command="fixdecoder --info",
        args=("--info",),
        max_lines=14,
    ),
    ReadmeExample(
        option="--message",
        display_command="fixdecoder --fix=44 --message=D --column",
        args=("--fix=44", "--message=D", "--column"),
        max_lines=26,
    ),
    ReadmeExample(
        option="--component",
        display_command="fixdecoder --fix=44 --component=Instrument --column",
        args=("--fix=44", "--component=Instrument", "--column"),
        max_lines=22,
    ),
    ReadmeExample(
        option="--tag",
        display_command="fixdecoder --fix=44 --tag=44 --verbose --column",
        args=("--fix=44", "--tag=44", "--verbose", "--column"),
        max_lines=24,
    ),
    ReadmeExample(
        option="--validate",
        display_command="printf '<invalid FIX>' | fixdecoder --fix=44 --validate --nocounts --colour=no",
        args=("--fix=44", "--validate", "--nocounts", "--colour=no"),
        stdin=INVALID_FIX,
        max_lines=18,
    ),
    ReadmeExample(
        option="--secret",
        display_command="printf '<FIX log>' | fixdecoder --fix=44 --secret --nocounts --delimiter='|' --colour=no",
        args=("--fix=44", "--secret", "--nocounts", "--delimiter=|", "--colour=no"),
        stdin=HEARTBEAT_FIX,
        max_lines=20,
    ),
    ReadmeExample(
        option="--secret-files",
        display_command="fixdecoder --secret-files target/readme-examples/orders.log",
        args=("--secret-files", "target/readme-examples/orders.log"),
        setup="secret-file",
        max_lines=8,
    ),
    ReadmeExample(
        option="--colour",
        display_command="printf '<FIX log>' | fixdecoder --fix=44 --nocounts --colour=no",
        args=("--fix=44", "--nocounts", "--colour=no"),
        stdin=HEARTBEAT_FIX,
        max_lines=18,
    ),
    ReadmeExample(
        option="--delimiter",
        display_command="printf '<FIX log>' | fixdecoder --fix=44 --nocounts --delimiter=' ' --colour=no",
        args=("--fix=44", "--nocounts", "--delimiter= ", "--colour=no"),
        stdin=HEARTBEAT_FIX,
        max_lines=18,
    ),
    ReadmeExample(
        option="--nocounts",
        display_command="printf '<FIX log>' | fixdecoder --fix=44 --nocounts --colour=no",
        args=("--fix=44", "--nocounts", "--colour=no"),
        stdin=HEARTBEAT_FIX,
        max_lines=18,
    ),
    ReadmeExample(
        option="--summary",
        display_command="printf '<order FIX log>' | fixdecoder --fix=44 --summary --nocounts --paging=never --colour=no",
        args=("--fix=44", "--summary", "--nocounts", "--paging=never", "--colour=no"),
        stdin=ORDER_FIX + EXEC_FIX,
        max_lines=28,
    ),
)


def main() -> int:
    if not README.exists():
        print(f"README not found: {README}", file=sys.stderr)
        return 1

    ensure_binary()
    examples = {example.option: render_example(example) for example in EXAMPLES}
    original = README.read_text()
    updated = update_build_examples(original)
    updated = update_usage_section(updated)
    updated = update_readme(updated, examples)
    README.write_text(updated)
    print(
        f"Updated {README.relative_to(ROOT)} with {len(examples)} command output examples "
        "and Build It examples"
    )
    return 0


def ensure_binary() -> None:
    subprocess.run(
        ["cargo", "build", "--quiet", "--bin", "fixdecoder"],
        cwd=ROOT,
        check=True,
    )


def render_example(example: ReadmeExample) -> str:
    if example.setup == "secret-file":
        prepare_secret_file()

    env = os.environ.copy()
    env.pop("FIXDECODER_DEFAULT_ARGS", None)
    env["PAGER"] = "cat"
    result = subprocess.run(
        [str(FIXDECODER), *example.args],
        cwd=ROOT,
        input=example.stdin,
        text=True,
        capture_output=True,
        check=False,
        env=env,
    )
    output = result.stdout
    if result.stderr:
        output = f"{output}{result.stderr}"
    if result.returncode != 0:
        raise RuntimeError(
            f"README example failed for {example.option}: {example.display_command}\n{output}"
        )

    output = sanitise_output(output)
    output = limit_lines(output, example.max_lines)
    return format_example_block(example, output)


def sanitise_output(output: str) -> str:
    output = ANSI_RE.sub("", output)
    output = output.replace(SOH, "|")
    output = output.replace(str(ROOT) + "/", "")
    output = output.replace(str(ROOT), ".")
    output = output.replace(str(Path.home()), "~")
    return "\n".join(line.rstrip() for line in output.rstrip().splitlines())


def limit_lines(output: str, max_lines: int) -> str:
    lines = output.splitlines()
    if len(lines) <= max_lines:
        return "\n".join(lines)
    shown = lines[:max_lines]
    shown.append("...")
    return "\n".join(shown)


def render_shell_command(command: tuple[str, ...], max_lines: int | None = None) -> str:
    result = subprocess.run(
        list(command),
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    output = sanitise_output(result.stdout)
    if max_lines is not None:
        output = limit_lines(output, max_lines)
    return output


def render_build_log() -> str:
    env = os.environ.copy()
    env["CARGO_TERM_COLOR"] = "never"
    result = subprocess.run(
        ["make", "clean", "build", "scan", "coverage", "build-release"],
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
        env=env,
    )
    output = sanitise_output(result.stdout)
    if result.returncode != 0:
        raise RuntimeError(
            "README build example failed for make clean build scan coverage build-release\n"
            f"{output}"
        )
    return output


def render_build_examples() -> str:
    examples = [
        BuildExample(
            "bash --version",
            "\n".join(render_shell_command(("bash", "--version")).splitlines()[:3]),
        ),
        BuildExample(
            "rustc --version",
            render_shell_command(("rustc", "--version")),
        ),
        BuildExample(
            "git clone git@github.com:stephenlclarke/fixdecoder.git",
            "Cloning into 'fixdecoder'...\n...\n❯ cd fixdecoder",
        ),
        BuildExample(
            "make clean build scan coverage build-release",
            render_build_log(),
        ),
        BuildExample(
            "make build-release",
            "\n".join(
                [
                    "",
                    ">> Ensuring Rust toolchain and coverage tools",
                    ">> Ensuring FIX XML specs are present",
                    "    Finished `release` profile [optimized] target(s) in ...",
                ]
            ),
        ),
        BuildExample(
            "./target/release/fixdecoder --version",
            render_shell_command((str(FIXDECODER), "--version")),
        ),
        BuildExample(
            "scripts/fixdecoder --version",
            render_shell_command((str(ROOT / "scripts" / "fixdecoder"), "--version")),
        ),
    ]

    return format_build_examples(examples)


def format_build_examples(examples: list[BuildExample]) -> str:
    sections = [
        "<!-- regen-readme:start --section=build-examples -->",
        "",
        "```bash",
        format_prompted_output(examples[0]),
        "```",
        "",
        "```bash",
        format_prompted_output(examples[1]),
        "```",
        "",
        "Clone the git repo.",
        "",
        "```bash",
        format_prompted_output(examples[2]),
        "```",
        "",
        "Then build it. Debug version with clippy and code coverage.",
        "",
        "If you want local Windows executables from macOS, `make build-windows` cross-compiles "
        "`fixdecoder.exe` and `pcap2fix.exe` for `x86_64-pc-windows-gnu`.",
        "",
        "```bash",
        format_prompted_output(examples[3]),
        "```",
        "",
        "Build only the optimized release binaries.",
        "",
        "```bash",
        format_prompted_output(examples[4]),
        "```",
        "",
        "Run it (from the optimized build) and check the version details:",
        "",
        "```bash",
        format_prompted_output(examples[5]),
        "```",
        "",
        "Run the same build through the source-checkout wrapper:",
        "",
        "```bash",
        format_prompted_output(examples[6]),
        "```",
        "",
        "<!-- regen-readme:end --section=build-examples -->",
        "",
        "",
    ]
    return "\n".join(sections)


def format_usage_block() -> str:
    usage = USAGE.read_text().rstrip()
    return (
        "<!-- regen-readme:start --section=usage -->\n"
        "\n"
        "## Full Usage Examples\n"
        "\n"
        "The text below is generated from `resources/messages/usage_en.txt`, the same usage text printed after `fixdecoder --help`.\n"
        "\n"
        "```text\n"
        f"{usage}\n"
        "```\n"
        "\n"
        "<!-- regen-readme:end --section=usage -->\n\n"
    )


def format_prompted_output(example: BuildExample) -> str:
    body = f"❯ {example.display_command}"
    if example.output:
        body = f"{body}\n{example.output}"
    return body


def format_example_block(example: ReadmeExample, output: str) -> str:
    command = f"$ {example.display_command}"
    body = command if not output else f"{command}\n{output}"
    return (
        f"<!-- regen-readme:start --option={example.option} -->\n"
        "Example output:\n\n"
        "```bash\n"
        f"{body}\n"
        "```\n"
        f"<!-- regen-readme:end --option={example.option} -->\n\n"
    )


def update_build_examples(markdown: str) -> str:
    block = render_build_examples()
    if BUILD_BLOCK_RE.search(markdown):
        return BUILD_BLOCK_RE.sub(block, markdown)

    build_heading = markdown.index("# Build it")
    pcap_heading = markdown.index("\n# PCAP to FIX filter", build_heading)
    first_example = markdown.index("```bash\n❯ bash --version", build_heading)
    return f"{markdown[:first_example]}{block}{markdown[pcap_heading + 1:]}"


def update_usage_section(markdown: str) -> str:
    block = format_usage_block()
    if USAGE_BLOCK_RE.search(markdown):
        return USAGE_BLOCK_RE.sub(block, markdown)

    insertion = markdown.index("## Key options at a glance")
    return f"{markdown[:insertion]}{block}{markdown[insertion:]}"


def update_readme(markdown: str, examples: dict[str, str]) -> str:
    markdown = OPTION_BLOCK_RE.sub("", markdown)
    lines = markdown.splitlines(keepends=True)
    output: list[str] = []
    index = 0

    while index < len(lines):
        line = lines[index]
        output.append(line)
        match = HEADING_RE.match(line)
        if not match:
            index += 1
            continue

        option = match.group("option")
        if option not in examples:
            index += 1
            continue

        section_start = index + 1
        next_heading = find_next_heading(lines, section_start)
        insert_at = find_insertion_index(lines, section_start, next_heading)
        output.extend(lines[section_start:insert_at])
        ensure_blank_before_block(output)
        output.append(examples[option])
        output.extend(lines[insert_at:next_heading])
        index = next_heading

    return "".join(output)


def find_next_heading(lines: list[str], start: int) -> int:
    for index in range(start, len(lines)):
        if lines[index].startswith("### ") or lines[index].startswith("## ") or lines[index].startswith("# "):
            return index
    return len(lines)


def find_insertion_index(lines: list[str], start: int, end: int) -> int:
    for index in range(start, end):
        stripped = lines[index].strip()
        if stripped.startswith("![") or stripped == "Examples:":
            return index
    return end


def ensure_blank_before_block(output: list[str]) -> None:
    if output and output[-1].strip():
        output.append("\n")
    while len(output) >= 2 and output[-1] == "\n" and output[-2] == "\n":
        output.pop()


if __name__ == "__main__":
    raise SystemExit(main())
