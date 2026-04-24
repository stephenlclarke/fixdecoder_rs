#!/usr/bin/env python3
"""Regenerate the explicit FIX MsgType bucket table from official FIX pages."""

from __future__ import annotations

import argparse
import html
import re
import sys
import urllib.request
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT_DIR = Path(__file__).resolve().parents[1]
RESOURCES_DIR = ROOT_DIR / "resources"
OUTPUT_PATH = ROOT_DIR / "src" / "decoder" / "message_groups_generated.rs"

SOURCE_PAGES = {
    "Pre-Trade": "https://www.fixtrading.org/online-specification/business-area-pretrade/",
    "Trade": "https://www.fixtrading.org/online-specification/business-area-trade/",
    "Post-Trade": "https://www.fixtrading.org/online-specification/business-area-posttrade/",
    "Infrastructure": "https://www.fixtrading.org/online-specification/business-area-infrastructure/",
}

CATEGORY_TO_BUCKET = {
    ("Pre-Trade", "Indication"): "MessageBucket::Business(BusinessMessageGroup::OrderFlow)",
    ("Pre-Trade", "Quotation Negotiation"): "MessageBucket::Business(BusinessMessageGroup::QuotesPricing)",
    ("Pre-Trade", "Market Data"): "MessageBucket::Business(BusinessMessageGroup::MarketData)",
    ("Pre-Trade", "Market Structure Reference Data"): "MessageBucket::Business(BusinessMessageGroup::ReferenceDataDefinitions)",
    ("Pre-Trade", "Securities Reference Data"): "MessageBucket::Business(BusinessMessageGroup::ReferenceDataDefinitions)",
    ("Pre-Trade", "Parties Reference Data"): "MessageBucket::Business(BusinessMessageGroup::ReferenceDataDefinitions)",
    ("Pre-Trade", "Event Communication"): "MessageBucket::BusinessOther",
    ("Pre-Trade", "Parties Action"): "MessageBucket::BusinessOther",
    ("Trade", "Single General Order Handling"): "MessageBucket::Business(BusinessMessageGroup::OrderFlow)",
    ("Trade", "Order Mass Handling"): "MessageBucket::Business(BusinessMessageGroup::OrderFlow)",
    ("Trade", "Cross Order Handling"): "MessageBucket::Business(BusinessMessageGroup::OrderFlow)",
    ("Trade", "Multileg Order Handling"): "MessageBucket::Business(BusinessMessageGroup::OrderFlow)",
    ("Trade", "Program Trading"): "MessageBucket::Business(BusinessMessageGroup::OrderFlow)",
    ("Post-Trade", "Allocation"): "MessageBucket::Business(BusinessMessageGroup::PostTradeAllocation)",
    ("Post-Trade", "Confirmation"): "MessageBucket::Business(BusinessMessageGroup::PostTradeAllocation)",
    ("Post-Trade", "Settlement Instruction"): "MessageBucket::Business(BusinessMessageGroup::PostTradeAllocation)",
    ("Post-Trade", "Trade Capture Reporting"): "MessageBucket::Business(BusinessMessageGroup::PostTradeAllocation)",
    ("Post-Trade", "Registration Instruction"): "MessageBucket::Business(BusinessMessageGroup::PostTradeAllocation)",
    ("Post-Trade", "Trade Management"): "MessageBucket::Business(BusinessMessageGroup::PostTradeAllocation)",
    ("Post-Trade", "Pay Management"): "MessageBucket::Business(BusinessMessageGroup::PostTradeAllocation)",
    ("Post-Trade", "Settlement Status Management"): "MessageBucket::Business(BusinessMessageGroup::PostTradeAllocation)",
    ("Post-Trade", "Position Maintenance"): "MessageBucket::Business(BusinessMessageGroup::PositionsCollateralRisk)",
    ("Post-Trade", "Collateral Management"): "MessageBucket::Business(BusinessMessageGroup::PositionsCollateralRisk)",
    ("Post-Trade", "Margin Requirement Management"): "MessageBucket::Business(BusinessMessageGroup::PositionsCollateralRisk)",
    ("Post-Trade", "Account Reporting"): "MessageBucket::Business(BusinessMessageGroup::PositionsCollateralRisk)",
    ("Infrastructure", "Business Message Rejects"): "MessageBucket::BusinessOther",
    ("Infrastructure", "Network Status Communication"): "MessageBucket::BusinessOther",
    ("Infrastructure", "User Management"): "MessageBucket::BusinessOther",
    ("Infrastructure", "Application Sequencing"): "MessageBucket::BusinessOther",
}

MSGTYPE_OVERRIDES = {
    # Risk-limit messages live under pre-trade party categories in the FIX
    # spec, but fit this tool's risk-oriented UI bucket better.
    "CL": "MessageBucket::Business(BusinessMessageGroup::PositionsCollateralRisk)",
    "CM": "MessageBucket::Business(BusinessMessageGroup::PositionsCollateralRisk)",
    "CR": "MessageBucket::Business(BusinessMessageGroup::PositionsCollateralRisk)",
    "CS": "MessageBucket::Business(BusinessMessageGroup::PositionsCollateralRisk)",
    "CT": "MessageBucket::Business(BusinessMessageGroup::PositionsCollateralRisk)",
    "DE": "MessageBucket::Business(BusinessMessageGroup::PositionsCollateralRisk)",
    "DF": "MessageBucket::Business(BusinessMessageGroup::PositionsCollateralRisk)",
    "DG": "MessageBucket::Business(BusinessMessageGroup::PositionsCollateralRisk)",
}

BUCKET_ORDER = {
    "MessageBucket::Business(BusinessMessageGroup::OrderFlow)": 0,
    "MessageBucket::Business(BusinessMessageGroup::QuotesPricing)": 1,
    "MessageBucket::Business(BusinessMessageGroup::MarketData)": 2,
    "MessageBucket::Business(BusinessMessageGroup::PostTradeAllocation)": 3,
    "MessageBucket::Business(BusinessMessageGroup::PositionsCollateralRisk)": 4,
    "MessageBucket::Business(BusinessMessageGroup::ReferenceDataDefinitions)": 5,
    "MessageBucket::BusinessOther": 6,
}

BUCKET_HEADINGS = {
    "MessageBucket::Business(BusinessMessageGroup::OrderFlow)": "Order Flow",
    "MessageBucket::Business(BusinessMessageGroup::QuotesPricing)": "Quotes & Pricing",
    "MessageBucket::Business(BusinessMessageGroup::MarketData)": "Market Data",
    "MessageBucket::Business(BusinessMessageGroup::PostTradeAllocation)": "Post-Trade & Allocation",
    "MessageBucket::Business(BusinessMessageGroup::PositionsCollateralRisk)": "Positions, Collateral & Risk",
    "MessageBucket::Business(BusinessMessageGroup::ReferenceDataDefinitions)": "Reference Data & Definitions",
    "MessageBucket::BusinessOther": "Other Business",
}

TABLE_RE = re.compile(r"<caption>\s*Messages for ([^<]+?) Business Area\s*</caption>(.*?)</table>", re.S)
ROW_RE = re.compile(
    r"<tr[^>]*>\s*<td[^>]*>(.*?)</td>\s*<td[^>]*>(.*?)</td>\s*<td[^>]*>(.*?)</td>\s*</tr>",
    re.S,
)
TAG_RE = re.compile(r"<[^>]+>")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Regenerate src/decoder/message_groups_generated.rs from official FIX pages."
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit non-zero if the generated file is out of date",
    )
    args = parser.parse_args()

    embedded_messages = load_embedded_messages()
    parsed_rows = load_official_rows()
    rendered = render_generated_file(parsed_rows, embedded_messages)

    if args.check:
        if not OUTPUT_PATH.exists() or OUTPUT_PATH.read_text() != rendered:
            print(f"{OUTPUT_PATH} is out of date", file=sys.stderr)
            return 1
        return 0

    OUTPUT_PATH.write_text(rendered)
    print(f"Wrote {OUTPUT_PATH}")
    return 0


def load_embedded_messages() -> dict[str, str]:
    messages: dict[str, str] = {}
    for xml_path in sorted(RESOURCES_DIR.glob("FIX*.xml")):
        root = ET.parse(xml_path).getroot()
        for message in root.findall(".//message"):
            msg_type = message.attrib.get("msgtype")
            msg_cat = message.attrib.get("msgcat")
            if not msg_type or not msg_cat:
                continue
            messages[msg_type] = msg_cat
    return messages


def load_official_rows() -> dict[str, tuple[str, str, str, str]]:
    rows: dict[str, tuple[str, str, str, str]] = {}
    for area, url in SOURCE_PAGES.items():
        html_text = fetch_html(url)
        for msg_type, name, category in extract_message_rows(area, html_text):
            if msg_type in rows and rows[msg_type] != (area, category, name, url):
                raise SystemExit(
                    f"duplicate MsgType {msg_type} with conflicting rows: "
                    f"{rows[msg_type]} vs {(area, category, name, url)}"
                )
            rows[msg_type] = (area, category, name, url)
    return rows


def fetch_html(url: str) -> str:
    request = urllib.request.Request(
        url,
        headers={"User-Agent": "fixdecoder message group generator"},
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return response.read().decode("utf-8", "ignore")


def extract_message_rows(area: str, html_text: str) -> list[tuple[str, str, str]]:
    match = None
    for candidate_area, table_html in TABLE_RE.findall(html_text):
        if normalise_text(candidate_area) == area:
            match = table_html
            break
    if match is None:
        raise SystemExit(f"could not find message table for {area}")

    rows = []
    for raw_type, raw_name, raw_category in ROW_RE.findall(match):
        msg_type = clean_html_text(raw_type)
        name = clean_html_text(raw_name)
        category = clean_html_text(raw_category)
        if msg_type and name and category:
            rows.append((msg_type, name, category))
    if not rows:
        raise SystemExit(f"message table for {area} had no rows")
    return rows


def clean_html_text(value: str) -> str:
    return normalise_text(TAG_RE.sub("", html.unescape(value)))


def normalise_text(value: str) -> str:
    return " ".join(value.replace("\xa0", " ").split())


def render_generated_file(
    parsed_rows: dict[str, tuple[str, str, str, str]],
    embedded_messages: dict[str, str],
) -> str:
    app_rows = build_bucket_rows(parsed_rows, embedded_messages)
    sections: list[str] = []
    current_bucket = None

    for msg_type, bucket_expr, area, category, name in app_rows:
        heading = BUCKET_HEADINGS[bucket_expr]
        if bucket_expr != current_bucket:
            if sections:
                sections.append("")
            sections.append(f"    // {heading}")
            current_bucket = bucket_expr
        comment = f"{area} / {category} / {name}"
        sections.append(f'    ("{msg_type}", {bucket_expr}), // {comment}')

    source_lines = "\n".join(f"// - {url}" for url in SOURCE_PAGES.values())
    body = "\n".join(sections)
    return (
        "// SPDX-License-Identifier: AGPL-3.0-only\n"
        "// SPDX-FileCopyrightText: 2025 Steve Clarke <stephenlclarke@mac.com> - https://xyzzy.tools\n"
        "//\n"
        "// Generated by ci/generate_message_groups.py from official FIX Trading Community pages.\n"
        "// Do not edit manually; regenerate with `make message-groups`.\n"
        f"{source_lines}\n\n"
        "const EXPLICIT_MESSAGE_BUCKETS: &[(&str, MessageBucket)] = &[\n"
        f"{body}\n"
        "];\n"
    )


def build_bucket_rows(
    parsed_rows: dict[str, tuple[str, str, str, str]],
    embedded_messages: dict[str, str],
) -> list[tuple[str, str, str, str, str]]:
    app_rows: list[tuple[str, str, str, str, str]] = []
    missing: list[str] = []

    for msg_type, msg_cat in sorted(embedded_messages.items()):
        if msg_cat != "app":
            continue
        row = parsed_rows.get(msg_type)
        if row is None:
            missing.append(msg_type)
            continue
        area, category, name, _url = row
        bucket_expr = MSGTYPE_OVERRIDES.get(msg_type) or CATEGORY_TO_BUCKET.get((area, category))
        if bucket_expr is None:
            raise SystemExit(
                f"no bucket mapping for official category {(area, category)} used by MsgType {msg_type}"
            )
        app_rows.append((msg_type, bucket_expr, area, category, name))

    if missing:
        raise SystemExit(
            "official FIX pages did not cover all embedded application messages: "
            + ", ".join(missing)
        )

    app_rows.sort(key=lambda row: (BUCKET_ORDER[row[1]], row[0]))
    return app_rows


if __name__ == "__main__":
    raise SystemExit(main())
