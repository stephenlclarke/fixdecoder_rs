#!/usr/bin/env python3
"""Generate a small corpus of valid FIX 4.4 repeating-group examples."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

ROOT_DIR = Path(__file__).resolve().parents[1]
OUTPUT_DIR = ROOT_DIR / "resources" / "examples" / "repeating_groups"
SOH = "\x01"

GENERATOR = "ci/generate_repeating_group_samples.py"

SAMPLES = [
    {
        "name": "new_order_single_parties",
        "title": "NewOrderSingle with Parties and nested PartySubIDs",
        "description": (
            "Exercises NoPartyIDs(453) with two party instances and a nested "
            "NoPartySubIDs(802) group on the first party."
        ),
        "sources": [
            "https://www.fixtrading.org/standards/tagvalue-online/",
            "https://fiximate.fixtrading.org/legacy/en/FIX.4.4/body_49484950.html?find=PartyID",
            "https://fiximate.fixtrading.org/legacy/en/FIX.4.4/body_495268.html?find=Side",
        ],
        "fields": [
            ("35", "D"),
            ("49", "BUY1"),
            ("56", "SELL1"),
            ("34", "1"),
            ("52", "20260424-10:00:00.000"),
            ("11", "ORD-1001"),
            ("453", "2"),
            ("448", "DEUTDEFF"),
            ("447", "B"),
            ("452", "1"),
            ("802", "1"),
            ("523", "ACC-12345"),
            ("803", "10"),
            ("448", "CLIENT01"),
            ("447", "D"),
            ("452", "5"),
            ("55", "IBM"),
            ("54", "1"),
            ("60", "20260424-10:00:00.000"),
            ("40", "2"),
            ("44", "185.25"),
        ],
    },
    {
        "name": "new_order_single_preallocs",
        "title": "NewOrderSingle with PreAllocGrp",
        "description": (
            "Exercises NoAllocs(78) inside PreAllocGrp with two allocation entries."
        ),
        "sources": [
            "https://fiximate.fixtrading.org/legacy/en/FIX.4.4/body_495268.html?find=Text",
            "https://fiximate.fixtrading.org/legacy/en/FIX.4.4/body_50485157.html?find=AllocQty",
        ],
        "fields": [
            ("35", "D"),
            ("49", "BUY1"),
            ("56", "SELL1"),
            ("34", "2"),
            ("52", "20260424-10:00:01.000"),
            ("11", "ORD-1002"),
            ("70", "BLOCK-ALLOC-1"),
            ("78", "2"),
            ("79", "ACC-ALPHA"),
            ("80", "600"),
            ("79", "ACC-BETA"),
            ("80", "400"),
            ("55", "IBM"),
            ("54", "1"),
            ("60", "20260424-10:00:01.000"),
            ("38", "1000"),
            ("40", "2"),
            ("44", "185.25"),
        ],
    },
    {
        "name": "allocation_instruction_orders",
        "title": "AllocationInstruction with OrdAllocGrp",
        "description": (
            "Exercises NoOrders(73) with two source orders contributing to one allocation."
        ),
        "sources": [
            "https://fiximate.fixtrading.org/legacy/en/FIX.4.4/body_495774.html?find=AllocID",
            "https://fiximate.fixtrading.org/legacy/en/FIX.5.0SP2/body_50485154.html",
        ],
        "fields": [
            ("35", "J"),
            ("49", "SELL1"),
            ("56", "BUYSIDE1"),
            ("34", "4"),
            ("52", "20260424-10:00:03.000"),
            ("70", "ALLOC-2001"),
            ("71", "0"),
            ("626", "5"),
            ("857", "1"),
            ("73", "2"),
            ("11", "ORD-1001"),
            ("37", "BRK-9001"),
            ("38", "600"),
            ("11", "ORD-1002"),
            ("37", "BRK-9002"),
            ("38", "400"),
            ("54", "1"),
            ("55", "IBM"),
            ("53", "1000"),
            ("6", "185.27"),
            ("75", "20260424"),
        ],
    },
    {
        "name": "market_data_snapshot_full_refresh",
        "title": "MarketDataSnapshotFullRefresh with MDFullGrp",
        "description": (
            "Exercises NoMDEntries(268) with one bid and one offer entry."
        ),
        "sources": [
            "https://fiximate.fixtrading.org/legacy/en/FIX.4.4/body_514887.html?find=MDFullGrp",
            "https://fiximate.fixtrading.org/legacy/en/FIX.4.4/body_50485149.html?find=NoMDEntries",
            "https://www.fixtrading.org/standards/json-online/",
        ],
        "fields": [
            ("35", "W"),
            ("49", "MDVENUE"),
            ("56", "CLIENT1"),
            ("34", "3"),
            ("52", "20260424-10:00:02.000"),
            ("262", "REQ-1"),
            ("55", "IBM"),
            ("268", "2"),
            ("269", "0"),
            ("270", "185.25"),
            ("271", "500"),
            ("272", "20260424"),
            ("273", "10:00:02.000"),
            ("269", "1"),
            ("270", "185.30"),
            ("271", "400"),
            ("272", "20260424"),
            ("273", "10:00:02.000"),
        ],
    },
]


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Regenerate the checked-in repeating-group FIX examples."
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit non-zero if the checked-in outputs are out of date",
    )
    args = parser.parse_args()

    expected_files = render_outputs()
    if args.check:
        current_files = load_current_generated_files()
        if current_files != expected_files:
            print("Repeating-group samples are out of date", file=sys.stderr)
            return 1
        return 0

    write_outputs(expected_files)
    print(
        f"Wrote {len(expected_files)} generated repeating-group sample files under {OUTPUT_DIR}"
    )
    return 0


def render_outputs() -> dict[Path, str]:
    expected_files: dict[Path, str] = {}
    manifest_entries = []
    aggregate_lines = [
        "# FIX repeating-group sample corpus",
        f"# Generated by {GENERATOR}",
        "",
    ]

    for sample in SAMPLES:
        relative_path = OUTPUT_DIR / f"{sample['name']}.fix"
        content = render_sample_file(sample)
        expected_files[relative_path] = content
        manifest_entries.append(
            {
                "file": relative_path.name,
                "title": sample["title"],
                "description": sample["description"],
                "sources": sample["sources"],
            }
        )
        aggregate_lines.extend(
            [
                f"# {sample['title']}",
                f"# {sample['description']}",
                *[f"# Source: {url}" for url in sample["sources"]],
                encode_fix_message(sample["fields"]),
                "",
            ]
        )

    manifest = {
        "generated_by": GENERATOR,
        "generated_files": manifest_entries,
    }
    expected_files[OUTPUT_DIR / "manifest.json"] = json.dumps(manifest, indent=2) + "\n"
    expected_files[OUTPUT_DIR / "all.fixlog"] = "\n".join(aggregate_lines).rstrip() + "\n"
    return expected_files


def render_sample_file(sample: dict[str, object]) -> str:
    lines = [
        f"# {sample['title']}",
        f"# {sample['description']}",
        *[f"# Source: {url}" for url in sample["sources"]],  # type: ignore[index]
        "",
        encode_fix_message(sample["fields"]),  # type: ignore[index]
        "",
    ]
    return "\n".join(lines)


def encode_fix_message(fields: list[tuple[str, str]]) -> str:
    body = "".join(f"{tag}={value}{SOH}" for tag, value in fields)
    prefix = f"8=FIX.4.4{SOH}9={len(body.encode('ascii'))}{SOH}"
    without_checksum = prefix + body
    checksum = sum(without_checksum.encode("ascii")) % 256
    return without_checksum + f"10={checksum:03}{SOH}"


def load_current_generated_files() -> dict[Path, str]:
    current: dict[Path, str] = {}
    if not OUTPUT_DIR.exists():
        return current
    for path in OUTPUT_DIR.rglob("*"):
        if not path.is_file() or path.name == "README.md":
            continue
        current[path] = path.read_text()
    return current


def write_outputs(expected_files: dict[Path, str]) -> None:
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    current_files = load_current_generated_files()
    for path in current_files:
        if path not in expected_files:
            path.unlink()
    for path, content in expected_files.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)


if __name__ == "__main__":
    raise SystemExit(main())
