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
CAPABILITY_BLOCK_RE = re.compile(
    r"<!-- regen-readme:start --section=capabilities -->\n.*?"
    r"<!-- regen-readme:end --section=capabilities -->\n*",
    re.S,
)
HEADING_RE = re.compile(r"^### `(?P<option>--[A-Za-z0-9-]+)")
ANSI_RE = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")
LONG_OPTION_RE = re.compile(r"(?<![\w-])--([A-Za-z][A-Za-z0-9-]*)")
MESSAGE_CODE_NAME_RE = re.compile(r"(?m)^\s*([A-Za-z0-9]{1,4})\s*:\s*([A-Za-z][A-Za-z0-9_]*)")
MESSAGE_NAME_CODE_RE = re.compile(r"(?m)^\s*([A-Za-z][A-Za-z0-9_]*)\s+\(([A-Za-z0-9]{1,4})\)")
COMPONENT_NAME_RE = re.compile(r"\b([A-Z][A-Za-z0-9_]*(?:Grp|Data|Instructions|Parties|Instrument|Trailer|Header|Hop)?)\b")
TAG_FIELD_RE = re.compile(r"(?m)^\s*([0-9]+)\s*:\s*([A-Za-z][A-Za-z0-9_]*)")

SOH = "\x01"
GROUP_TAG_CANDIDATES = ("453", "78", "802", "539", "804", "268")
PREFERRED_COMPONENTS = ("PreAllocGrp", "Parties", "Instrument")


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


@dataclass(frozen=True)
class CapabilitySnapshot:
    help_text: str
    options: frozenset[str]
    message_code: str
    message_name: str
    component_name: str
    group_tag: str
    group_name: str


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


def main() -> int:
    if not README.exists():
        print(f"README not found: {README}", file=sys.stderr)
        return 1

    ensure_binary()
    capabilities = discover_capabilities()
    readme_examples = build_readme_examples(capabilities)
    examples = {example.option: render_example(example) for example in readme_examples}
    original = README.read_text()
    updated = update_build_examples(original)
    updated = update_usage_section(updated, capabilities)
    updated = update_capability_section(updated, capabilities)
    updated = update_readme(updated, examples)
    README.write_text(updated)
    print(
        f"Updated {README.relative_to(ROOT)} with {len(examples)} generated command examples, "
        "capability discovery, usage, and Build It examples"
    )
    return 0


def ensure_binary() -> None:
    subprocess.run(
        ["cargo", "build", "--quiet", "--bin", "fixdecoder"],
        cwd=ROOT,
        check=True,
    )


def discover_capabilities() -> CapabilitySnapshot:
    help_text = run_fixdecoder(("--help",))
    options = frozenset(f"--{option}" for option in sorted(set(LONG_OPTION_RE.findall(help_text))))
    colour_args = ("--colour=no",) if "--colour" in options else ()

    message_output = run_fixdecoder(("--fix=44", "--message", "--column", *colour_args))
    message_code, message_name = choose_message(parse_messages(message_output))

    component_output = run_fixdecoder(("--fix=44", "--component", "--column", *colour_args))
    component_name = choose_component(parse_components(component_output))

    group_tag, group_name = choose_group_tag(options, colour_args)
    return CapabilitySnapshot(
        help_text=sanitise_output(help_text),
        options=options,
        message_code=message_code,
        message_name=message_name,
        component_name=component_name,
        group_tag=group_tag,
        group_name=group_name,
    )


def run_fixdecoder(args: tuple[str, ...], stdin: str | None = None) -> str:
    env = os.environ.copy()
    env.pop("FIXDECODER_DEFAULT_ARGS", None)
    env["PAGER"] = "cat"
    result = subprocess.run(
        [str(FIXDECODER), *args],
        cwd=ROOT,
        input=stdin,
        text=True,
        capture_output=True,
        check=False,
        env=env,
    )
    output = f"{result.stdout}{result.stderr}"
    if result.returncode != 0:
        raise RuntimeError(f"fixdecoder discovery failed for {' '.join(args)}\n{output}")
    return output


def parse_messages(output: str) -> list[tuple[str, str]]:
    clean = sanitise_output(output)
    found: list[tuple[str, str]] = []
    found.extend((match.group(1), match.group(2)) for match in MESSAGE_CODE_NAME_RE.finditer(clean))
    found.extend((match.group(2), match.group(1)) for match in MESSAGE_NAME_CODE_RE.finditer(clean))
    return dedupe_pairs(found)


def parse_components(output: str) -> list[str]:
    clean = sanitise_output(output)
    ignored = {"Session", "Admin", "Business", "Order", "Flow", "Pricing"}
    names = [
        match.group(1)
        for match in COMPONENT_NAME_RE.finditer(clean)
        if match.group(1) not in ignored and len(match.group(1)) > 2
    ]
    return dedupe_names(names)


def choose_message(messages: list[tuple[str, str]]) -> tuple[str, str]:
    for code, name in messages:
        if code == "D" or name == "NewOrderSingle":
            return code, name
    if messages:
        return messages[0]
    return "D", "NewOrderSingle"


def choose_component(components: list[str]) -> str:
    for preferred in PREFERRED_COMPONENTS:
        if preferred in components:
            return preferred
    return components[0] if components else "Instrument"


def choose_group_tag(options: frozenset[str], colour_args: tuple[str, ...]) -> tuple[str, str]:
    if "--tag" not in options:
        return "453", "NoPartyIDs"
    for tag in GROUP_TAG_CANDIDATES:
        output = run_fixdecoder(("--fix=44", f"--tag={tag}", "--verbose", "--column", *colour_args))
        clean = sanitise_output(output)
        match = TAG_FIELD_RE.search(clean)
        if match and ("NUMINGROUP" in clean or match.group(2).startswith("No")):
            return match.group(1), match.group(2)
    return "453", "NoPartyIDs"


def dedupe_pairs(pairs: list[tuple[str, str]]) -> list[tuple[str, str]]:
    seen: set[tuple[str, str]] = set()
    result: list[tuple[str, str]] = []
    for pair in pairs:
        if pair in seen:
            continue
        seen.add(pair)
        result.append(pair)
    return result


def dedupe_names(names: list[str]) -> list[str]:
    seen: set[str] = set()
    result: list[str] = []
    for name in names:
        if name in seen:
            continue
        seen.add(name)
        result.append(name)
    return result


def build_readme_examples(capabilities: CapabilitySnapshot) -> tuple[ReadmeExample, ...]:
    examples: list[ReadmeExample] = []
    colour_args = ("--colour=no",) if "--colour" in capabilities.options else ()
    quiet_args = ("--nocounts",) if "--nocounts" in capabilities.options else ()

    def add_if_supported(option: str, example: ReadmeExample) -> None:
        if option in capabilities.options:
            examples.append(example)

    add_if_supported(
        "--xml",
        ReadmeExample(
            option="--xml",
            display_command="fixdecoder --xml resources/FIX44.xml --fix=44 --info",
            args=("--xml", "resources/FIX44.xml", "--fix=44", "--info"),
            max_lines=14,
        ),
    )
    add_if_supported(
        "--fix",
        ReadmeExample(
            option="--fix",
            display_command="fixdecoder --fix=FIX50SP2 --info",
            args=("--fix=FIX50SP2", "--info"),
            max_lines=14,
        ),
    )
    add_if_supported(
        "--info",
        ReadmeExample(
            option="--info",
            display_command="fixdecoder --info",
            args=("--info",),
            max_lines=14,
        ),
    )
    add_if_supported(
        "--message",
        ReadmeExample(
            option="--message",
            display_command=f"fixdecoder --fix=44 --message={capabilities.message_code} --column",
            args=("--fix=44", f"--message={capabilities.message_code}", "--column", *colour_args),
            max_lines=26,
        ),
    )
    add_if_supported(
        "--component",
        ReadmeExample(
            option="--component",
            display_command=f"fixdecoder --fix=44 --component={capabilities.component_name} --column",
            args=("--fix=44", f"--component={capabilities.component_name}", "--column", *colour_args),
            max_lines=22,
        ),
    )
    add_if_supported(
        "--tag",
        ReadmeExample(
            option="--tag",
            display_command=f"fixdecoder --fix=44 --tag={capabilities.group_tag} --verbose --column",
            args=("--fix=44", f"--tag={capabilities.group_tag}", "--verbose", "--column", *colour_args),
            max_lines=24,
        ),
    )
    add_if_supported(
        "--validate",
        ReadmeExample(
            option="--validate",
            display_command="printf '<invalid FIX>' | fixdecoder --fix=44 --validate --nocounts --colour=no",
            args=("--fix=44", "--validate", *quiet_args, *colour_args),
            stdin=INVALID_FIX,
            max_lines=18,
        ),
    )
    add_if_supported(
        "--secret",
        ReadmeExample(
            option="--secret",
            display_command="printf '<FIX log>' | fixdecoder --fix=44 --secret --nocounts --delimiter='|' --colour=no",
            args=("--fix=44", "--secret", *quiet_args, "--delimiter=|", *colour_args),
            stdin=HEARTBEAT_FIX,
            max_lines=20,
        ),
    )
    add_if_supported(
        "--secret-files",
        ReadmeExample(
            option="--secret-files",
            display_command="fixdecoder --secret-files target/readme-examples/orders.log",
            args=("--secret-files", "target/readme-examples/orders.log"),
            setup="secret-file",
            max_lines=8,
        ),
    )
    add_if_supported(
        "--colour",
        ReadmeExample(
            option="--colour",
            display_command="printf '<FIX log>' | fixdecoder --fix=44 --nocounts --colour=no",
            args=("--fix=44", *quiet_args, *colour_args),
            stdin=HEARTBEAT_FIX,
            max_lines=18,
        ),
    )
    add_if_supported(
        "--delimiter",
        ReadmeExample(
            option="--delimiter",
            display_command="printf '<FIX log>' | fixdecoder --fix=44 --nocounts --delimiter=' ' --colour=no",
            args=("--fix=44", *quiet_args, "--delimiter= ", *colour_args),
            stdin=HEARTBEAT_FIX,
            max_lines=18,
        ),
    )
    add_if_supported(
        "--nocounts",
        ReadmeExample(
            option="--nocounts",
            display_command="printf '<FIX log>' | fixdecoder --fix=44 --nocounts --colour=no",
            args=("--fix=44", *quiet_args, *colour_args),
            stdin=HEARTBEAT_FIX,
            max_lines=18,
        ),
    )
    add_if_supported(
        "--summary",
        ReadmeExample(
            option="--summary",
            display_command="printf '<order FIX log>' | fixdecoder --fix=44 --summary --nocounts --paging=never --colour=no",
            args=("--fix=44", "--summary", *quiet_args, "--paging=never", *colour_args),
            stdin=ORDER_FIX + EXEC_FIX,
            max_lines=28,
        ),
    )
    return tuple(examples)


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


def format_usage_block(capabilities: CapabilitySnapshot) -> str:
    usage = capabilities.help_text.rstrip()
    return (
        "<!-- regen-readme:start --section=usage -->\n"
        "\n"
        "## Full Usage Examples\n"
        "\n"
        "The text below is generated by running this implementation's `fixdecoder --help`.\n"
        "\n"
        "```text\n"
        f"{usage}\n"
        "```\n"
        "\n"
        "<!-- regen-readme:end --section=usage -->\n\n"
    )


def format_capability_block(capabilities: CapabilitySnapshot) -> str:
    options = ", ".join(f"`{option}`" for option in sorted(capabilities.options))
    message_output = limit_lines(
        sanitise_output(
            run_fixdecoder(
                ("--fix=44", f"--message={capabilities.message_code}", "--column", *colour_args(capabilities))
            )
        ),
        18,
    )
    component_output = limit_lines(
        sanitise_output(
            run_fixdecoder(
                ("--fix=44", f"--component={capabilities.component_name}", "--column", *colour_args(capabilities))
            )
        ),
        16,
    )
    group_output = limit_lines(
        sanitise_output(
            run_fixdecoder(
                ("--fix=44", f"--tag={capabilities.group_tag}", "--verbose", "--column", *colour_args(capabilities))
            )
        ),
        12,
    )
    return "\n".join(
        [
            "<!-- regen-readme:start --section=capabilities -->",
            "",
            "## Generated Capability Snapshot",
            "",
            "This snapshot is generated by `make regen-readme` by running this implementation's binary and reflects the options and dictionary surface currently available in this repository.",
            "",
            f"- Supported long options: {options}",
            f"- Sample message discovered from the dictionary: `{capabilities.message_name} ({capabilities.message_code})`",
            f"- Sample component discovered from the dictionary: `{capabilities.component_name}`",
            f"- Sample repeating group tag discovered from the dictionary: `{capabilities.group_name} ({capabilities.group_tag})`",
            "",
            "```bash",
            f"$ fixdecoder --fix=44 --message={capabilities.message_code} --column",
            message_output,
            "```",
            "",
            "```bash",
            f"$ fixdecoder --fix=44 --component={capabilities.component_name} --column",
            component_output,
            "```",
            "",
            "```bash",
            f"$ fixdecoder --fix=44 --tag={capabilities.group_tag} --verbose --column",
            group_output,
            "```",
            "",
            "<!-- regen-readme:end --section=capabilities -->",
            "",
            "",
        ]
    )


def colour_args(capabilities: CapabilitySnapshot) -> tuple[str, ...]:
    return ("--colour=no",) if "--colour" in capabilities.options else ()


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


def update_usage_section(markdown: str, capabilities: CapabilitySnapshot) -> str:
    block = format_usage_block(capabilities)
    if USAGE_BLOCK_RE.search(markdown):
        return USAGE_BLOCK_RE.sub(block, markdown)

    insertion = markdown.index("## Key options at a glance")
    return f"{markdown[:insertion]}{block}{markdown[insertion:]}"


def update_capability_section(markdown: str, capabilities: CapabilitySnapshot) -> str:
    block = format_capability_block(capabilities)
    if CAPABILITY_BLOCK_RE.search(markdown):
        return CAPABILITY_BLOCK_RE.sub(block, markdown)

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
