#!/usr/bin/env python3
"""Generate valid FIX Appendix D sample logs from the official FIX matrices."""

from __future__ import annotations

import argparse
import json
import re
import sys
import urllib.request
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from html.parser import HTMLParser
from pathlib import Path

ROOT_DIR = Path(__file__).resolve().parents[1]
RESOURCES_DIR = ROOT_DIR / "resources"
FIX44_XML_PATH = RESOURCES_DIR / "FIX44.xml"
OUTPUT_DIR = RESOURCES_DIR / "examples" / "appendix_d"
SOURCE_URL = "https://www.fixtrading.org/online-specification/order-state-changes/"
SOURCE_PDF_URL = (
    "https://www.fixtrading.org/wp-content/uploads/download-manager-files/"
    "FIX-Latest-as-of-EP284-Order-State-Changes.pdf"
)
SOH = "\x01"

HEADING_RE = re.compile(r"^[A-Z]\.\d\.[a-z]")
MESSAGE_RE = re.compile(
    r"^(New Order|Cancel Request|Replace Request|Status Request|Cancel Reject|Execution)"
    r"(?:\s*\(([^)]*)\))?$"
)
EXEC_REF_RE = re.compile(r"^([A-Za-z0-9]+)(?:\s*\(([^)]+)\))?$")


@dataclass(frozen=True)
class ParsedRow:
    time_label: str
    optional: bool
    message_received: str
    message_sent: str
    exec_type: str
    ord_status: str
    order_qty: str
    cash_order_qty: str
    cum_qty: str
    leaves_qty: str
    last_qty: str
    last_px: str
    avg_px: str
    day_order_qty: str
    day_cum_qty: str
    price: str
    exec_id_ref: str
    comment: str


@dataclass(frozen=True)
class ScenarioTable:
    section: str
    title: str
    scenario_id: str
    slug: str
    rows: list[ParsedRow]


@dataclass(frozen=True)
class ScenarioVariant:
    section: str
    title: str
    scenario_id: str
    slug: str
    variant: str
    variant_note: str
    rows: list[ParsedRow]


@dataclass
class ScenarioState:
    section: str
    title: str
    slug: str
    tif: str
    ord_type: str
    currency: str
    symbol: str
    side: str
    exec_inst: str | None
    stop_px: str | None
    default_price: str
    include_cash_order_qty: bool
    order_id_by_token: dict[str, str]
    price_by_order_id: dict[str, str]
    orig_qty_by_order_id: dict[str, str]
    cash_qty_by_order_id: dict[str, str]
    sender_seq: dict[tuple[str, str], int]
    exec_ids: dict[str, str]
    generated_exec_index: int


@dataclass(frozen=True)
class Fix44Metadata:
    enum_map: dict[str, dict[str, str]]
    message_order: dict[str, list[str]]
    message_allowed_tags: dict[str, set[str]]


class AppendixDHtmlParser(HTMLParser):
    """Extract scenario tables from the official order-state HTML page."""

    def __init__(self) -> None:
        super().__init__()
        self._active_heading: str | None = None
        self._heading_text: list[str] = []
        self._section = "general"

        self._current_h5 = ""
        self._in_table = False
        self._rows: list[tuple[list[str], bool]] = []
        self._row: list[str] = []
        self._cell: list[str] = []
        self._in_cell = False
        self._row_has_em = False
        self._tables: list[tuple[str, str, list[tuple[list[str], bool]]]] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag in {"h1", "h2", "h5"}:
            self._active_heading = tag
            self._heading_text = []
        elif tag == "table":
            self._in_table = True
            self._rows = []
        elif tag == "tr" and self._in_table:
            self._row = []
            self._row_has_em = False
        elif tag in {"td", "th"} and self._in_table:
            self._in_cell = True
            self._cell = []
        elif tag == "br" and self._in_cell:
            self._cell.append(" ")
        elif tag == "em" and self._in_table:
            self._row_has_em = True

    def handle_endtag(self, tag: str) -> None:
        if tag == self._active_heading:
            text = clean_text("".join(self._heading_text))
            if tag in {"h1", "h2"}:
                if "Order State Change Matrices for Exchanges" in text:
                    self._section = "exchange"
                elif "General Order State Change Matrices" in text:
                    self._section = "general"
                if "Scenarios for State Transitions for Exchanges" in text:
                    self._section = "exchange"
                elif "General Scenarios for State Transitions" in text:
                    self._section = "general"
            elif tag == "h5":
                self._current_h5 = text
            self._active_heading = None
            self._heading_text = []
        elif tag in {"td", "th"} and self._in_cell:
            self._row.append(clean_text("".join(self._cell)))
            self._in_cell = False
            self._cell = []
        elif tag == "tr" and self._in_table and self._row:
            self._rows.append((self._row, self._row_has_em))
            self._row = []
        elif tag == "table" and self._in_table:
            if self._current_h5 and HEADING_RE.match(self._current_h5) and self._rows:
                self._tables.append((self._section, self._current_h5, self._rows))
            self._in_table = False
            self._rows = []

    def handle_data(self, data: str) -> None:
        if self._active_heading:
            self._heading_text.append(data)
        elif self._in_cell:
            self._cell.append(data)

    @property
    def tables(self) -> list[tuple[str, str, list[tuple[list[str], bool]]]]:
        return self._tables


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Regenerate valid FIX Appendix D sample logs from the official FIX matrices.",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit non-zero if the checked-in Appendix D samples are out of date",
    )
    args = parser.parse_args()

    html_text = fetch_text(SOURCE_URL)
    tables = parse_scenario_tables(html_text)
    expected_files = render_outputs(tables)

    if args.check:
        current_files = load_current_generated_files()
        if current_files != expected_files:
            print("Appendix D samples are out of date", file=sys.stderr)
            return 1
        return 0

    write_outputs(expected_files)
    print(f"Wrote {len(expected_files)} generated Appendix D sample files under {OUTPUT_DIR}")
    return 0


def fetch_text(url: str) -> str:
    request = urllib.request.Request(
        url,
        headers={"User-Agent": "fixdecoder Appendix D sample generator"},
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return response.read().decode("utf-8", "ignore")


def parse_scenario_tables(html_text: str) -> list[ScenarioTable]:
    parser = AppendixDHtmlParser()
    parser.feed(html_text)

    seen: dict[tuple[str, str], int] = {}
    tables: list[ScenarioTable] = []

    for section, title, rows in parser.tables:
        first_row = rows[0][0]
        if first_row and clean_text(first_row[0]).lower() == "ref":
            continue
        scenario_id = title.split(" ", 1)[0]
        suffix = seen.get((section, scenario_id), 0) + 1
        seen[(section, scenario_id)] = suffix

        title_without_id = clean_text(title.split(" ", 1)[1] if " " in title else title)
        slug = slugify(f"{scenario_id}_{title_without_id}")
        if suffix > 1:
            slug = f"{slug}_{suffix}"

        header = [normalise_header(cell) for cell in rows[0][0]]
        parsed_rows = [parse_row(header, row_cells, optional) for row_cells, optional in rows[1:]]
        tables.append(
            ScenarioTable(
                section=section,
                title=title,
                scenario_id=scenario_id,
                slug=slug,
                rows=parsed_rows,
            )
        )

    return tables


def normalise_header(header: str) -> str:
    lowered = clean_text(header).lower()
    if lowered.startswith("message received"):
        return "message_received"
    if lowered.startswith("message sent"):
        return "message_sent"
    if lowered == "time":
        return "time_label"
    if lowered == "exectype":
        return "exec_type"
    if lowered == "ordstatus":
        return "ord_status"
    if lowered == "orderqty":
        return "order_qty"
    if lowered == "cash orderqty":
        return "cash_order_qty"
    if lowered == "cumqty":
        return "cum_qty"
    if lowered == "leavesqty":
        return "leaves_qty"
    if lowered == "lastqty":
        return "last_qty"
    if lowered == "lastpx":
        return "last_px"
    if lowered == "avgpx":
        return "avg_px"
    if lowered == "day orderqty":
        return "day_order_qty"
    if lowered == "day cumqty":
        return "day_cum_qty"
    if lowered == "price":
        return "price"
    if lowered in {"execid (exec refid)", "execid(exec refid)", "execid", "execid (exec refid)"}:
        return "exec_id_ref"
    if lowered == "comment":
        return "comment"
    raise ValueError(f"unhandled Appendix D header: {header}")


def parse_row(headers: list[str], row_cells: list[str], optional: bool) -> ParsedRow:
    data = dict.fromkeys(headers, "")
    for idx, header in enumerate(headers):
        data[header] = row_cells[idx] if idx < len(row_cells) else ""

    return ParsedRow(
        time_label=data.get("time_label", ""),
        optional=optional,
        message_received=data.get("message_received", ""),
        message_sent=data.get("message_sent", ""),
        exec_type=data.get("exec_type", ""),
        ord_status=data.get("ord_status", ""),
        order_qty=data.get("order_qty", ""),
        cash_order_qty=data.get("cash_order_qty", ""),
        cum_qty=data.get("cum_qty", ""),
        leaves_qty=data.get("leaves_qty", ""),
        last_qty=data.get("last_qty", ""),
        last_px=data.get("last_px", ""),
        avg_px=data.get("avg_px", ""),
        day_order_qty=data.get("day_order_qty", ""),
        day_cum_qty=data.get("day_cum_qty", ""),
        price=data.get("price", ""),
        exec_id_ref=data.get("exec_id_ref", ""),
        comment=data.get("comment", ""),
    )


def render_outputs(tables: list[ScenarioTable]) -> dict[Path, str]:
    expected_files: dict[Path, str] = {}
    manifest_entries = []
    aggregate_lines = [
        "# FIX Appendix D sample corpus",
        "# Generated by ci/generate_appendix_d_samples.py",
        f"# Source HTML: {SOURCE_URL}",
        f"# Source PDF: {SOURCE_PDF_URL}",
        "",
    ]

    variants: list[ScenarioVariant] = []
    for table in tables:
        main_rows = [row for row in table.rows if not row.optional]
        variants.append(
            ScenarioVariant(
                section=table.section,
                title=table.title,
                scenario_id=table.scenario_id,
                slug=table.slug,
                variant="main",
                variant_note="primary path from the non-italic rows",
                rows=main_rows,
            )
        )

        alt_count = 0
        for index, row in enumerate(table.rows):
            if not row.optional:
                continue
            alt_count += 1
            prefix = [candidate for candidate in table.rows[:index] if not candidate.optional]
            variants.append(
                ScenarioVariant(
                    section=table.section,
                    title=table.title,
                    scenario_id=table.scenario_id,
                    slug=table.slug,
                    variant=f"alt{alt_count:02}",
                    variant_note=row.comment or f"alternate branch at {row.time_label}",
                    rows=prefix + [row],
                )
            )

    for variant in variants:
        relative_path = sample_path_for_variant(variant)
        content = render_variant_file(variant)
        expected_files[relative_path] = content

        manifest_entries.append(
            {
                "file": str(relative_path.relative_to(OUTPUT_DIR)),
                "section": variant.section,
                "scenario_id": variant.scenario_id,
                "title": ascii_text(variant.title),
                "variant": variant.variant,
                "variant_note": ascii_text(variant.variant_note),
                "message_count": sum(
                    1 for row in variant.rows if row.message_received or row.message_sent
                ),
            }
        )

        aggregate_lines.extend(
            [
                f"# {variant.section.upper()} {variant.scenario_id} {variant.variant}: "
                f"{ascii_text(variant.title)}",
                f"# Variant note: {ascii_text(variant.variant_note)}",
                *content.splitlines(),
                "",
            ]
        )

    manifest = {
        "source_html": SOURCE_URL,
        "source_pdf": SOURCE_PDF_URL,
        "generated_files": sorted(manifest_entries, key=lambda item: item["file"]),
    }
    expected_files[OUTPUT_DIR / "manifest.json"] = json.dumps(manifest, indent=2) + "\n"
    expected_files[OUTPUT_DIR / "all.fixlog"] = "\n".join(aggregate_lines).rstrip() + "\n"
    return expected_files


def render_variant_file(variant: ScenarioVariant) -> str:
    state = initial_state(variant)
    lines = [
        f"# {variant.section.upper()} {variant.scenario_id} {variant.variant}",
        f"# {ascii_text(variant.title)}",
        f"# Variant note: {ascii_text(variant.variant_note)}",
        f"# Source: {SOURCE_URL}",
        "",
    ]

    for row in variant.rows:
        lines.extend(render_row(state, row))

    return "\n".join(lines).rstrip() + "\n"


def initial_state(variant: ScenarioVariant) -> ScenarioState:
    lower_title = variant.title.lower()
    tif = "0"
    if "good till cancel" in lower_title or variant.scenario_id.startswith("H."):
        tif = "1"
    elif "immediate or cancel" in lower_title:
        tif = "3"
    elif "fill or kill" in lower_title:
        tif = "4"

    ord_type = "2"
    stop_px = None
    if "held waiting for activation" in lower_title:
        ord_type = "3"
        stop_px = "49.50"

    currency = "USD"
    default_price = "50.00"
    include_cash_order_qty = "cashorderqty" in lower_title.replace(" ", "")
    if include_cash_order_qty:
        currency = "EUR"
        default_price = "20.00"

    exec_inst = None
    if "cancel if not best" in lower_title:
        exec_inst = "Z"
        default_price = "56.00"

    return ScenarioState(
        section=variant.section,
        title=variant.title,
        slug=variant.slug,
        tif=tif,
        ord_type=ord_type,
        currency=currency,
        symbol="IBM",
        side="1",
        exec_inst=exec_inst,
        stop_px=stop_px,
        default_price=default_price,
        include_cash_order_qty=include_cash_order_qty,
        order_id_by_token={},
        price_by_order_id={},
        orig_qty_by_order_id={},
        cash_qty_by_order_id={},
        sender_seq={},
        exec_ids={},
        generated_exec_index=0,
    )


def render_row(state: ScenarioState, row: ParsedRow) -> list[str]:
    lines = [f"# {ascii_text(row.time_label)}: {ascii_text(row.comment or describe_row(row))}"]

    received = parse_message_ref(row.message_received)
    sent = parse_message_ref(row.message_sent)

    if received:
        lines.append(build_message(state, row, received, incoming=True))
    if sent:
        lines.append(build_message(state, row, sent, incoming=False))
    return lines + [""]


def describe_row(row: ParsedRow) -> str:
    if row.message_received:
        return row.message_received
    if row.message_sent:
        return row.message_sent
    return "scenario note"


def parse_message_ref(raw: str) -> tuple[str, list[str]] | None:
    text = clean_text(raw)
    if not text:
        return None
    match = MESSAGE_RE.match(text)
    if not match:
        return None
    kind = match.group(1)
    ids = [token.strip() for token in (match.group(2) or "").split(",") if token.strip()]
    return kind, ids


def build_message(
    state: ScenarioState,
    row: ParsedRow,
    ref: tuple[str, list[str]],
    *,
    incoming: bool,
) -> str:
    kind, ids = ref
    if kind == "New Order":
        return build_new_order(state, row, ids)
    if kind == "Cancel Request":
        return build_cancel_request(state, row, ids)
    if kind == "Replace Request":
        return build_replace_request(state, row, ids)
    if kind == "Status Request":
        return build_status_request(state, row, ids)
    if kind == "Cancel Reject":
        return build_cancel_reject(state, row, ids)
    if kind == "Execution":
        return build_execution_report(state, row, ids, incoming=incoming)
    raise ValueError(f"unsupported Appendix D message kind: {kind}")


def build_new_order(state: ScenarioState, row: ParsedRow, ids: list[str]) -> str:
    token = ids[0] if ids else "VOICE"
    order_id = ensure_order(state, token, replacement_for=None)
    price = order_price(state, row, order_id)
    qty = normalise_qty(row.order_qty)
    cash_qty = normalise_qty(row.cash_order_qty)

    fields = base_fields(state, "D", "BUY1", "SELL1")
    if token != "VOICE":
        fields.append(("11", cl_ord_id(state, token)))
    fields.extend(
        [
            ("21", "1"),
            ("55", state.symbol),
            ("54", state.side),
            ("60", timestamp_for(row.time_label)),
        ]
    )
    if qty:
        fields.append(("38", qty))
        state.orig_qty_by_order_id[order_id] = qty
    if cash_qty:
        fields.append(("152", cash_qty))
        state.cash_qty_by_order_id[order_id] = cash_qty
    fields.append(("40", state.ord_type))
    if state.ord_type in {"2", "4"}:
        fields.append(("44", price))
    if state.stop_px:
        fields.append(("99", state.stop_px))
    fields.append(("59", state.tif))
    fields.append(("15", state.currency))
    if state.exec_inst:
        fields.append(("18", state.exec_inst))
    if "PossResend=Y" in row.comment:
        fields.append(("97", "Y"))
    return encode_fix_message(fields)


def build_cancel_request(state: ScenarioState, row: ParsedRow, ids: list[str]) -> str:
    new_token = ids[0]
    orig_token = ids[1] if len(ids) > 1 else ids[0]
    order_id = ensure_order(state, new_token, replacement_for=orig_token)
    fields = base_fields(state, "F", "BUY1", "SELL1")
    fields.extend(
        [
            ("41", cl_ord_id(state, orig_token)),
            ("37", order_id),
            ("11", cl_ord_id(state, new_token)),
            ("55", state.symbol),
            ("54", state.side),
            ("60", timestamp_for(row.time_label)),
        ]
    )

    qty = normalise_qty(row.order_qty) or state.orig_qty_by_order_id.get(order_id, "")
    if qty:
        fields.append(("38", qty))
    return encode_fix_message(fields)


def build_replace_request(state: ScenarioState, row: ParsedRow, ids: list[str]) -> str:
    new_token = ids[0]
    orig_token = ids[1] if len(ids) > 1 else ids[0]
    order_id = ensure_order(state, new_token, replacement_for=orig_token)
    price = order_price(state, row, order_id)
    qty = normalise_qty(row.order_qty) or state.orig_qty_by_order_id.get(order_id, "")

    fields = base_fields(state, "G", "BUY1", "SELL1")
    fields.extend(
        [
            ("41", cl_ord_id(state, orig_token)),
            ("37", order_id),
            ("11", cl_ord_id(state, new_token)),
            ("55", state.symbol),
            ("54", state.side),
            ("60", timestamp_for(row.time_label)),
            ("38", qty),
            ("40", state.ord_type),
        ]
    )
    if state.ord_type in {"2", "4"}:
        fields.append(("44", price))
    if state.stop_px:
        fields.append(("99", state.stop_px))
    fields.append(("59", state.tif))
    fields.append(("15", state.currency))

    state.orig_qty_by_order_id[order_id] = qty
    return encode_fix_message(fields)


def build_status_request(state: ScenarioState, row: ParsedRow, ids: list[str]) -> str:
    token = ids[0]
    order_id = state.order_id_by_token.get(token, "NONE")
    fields = base_fields(state, "H", "BUY1", "SELL1")
    fields.extend(
        [
            ("37", order_id),
            ("11", cl_ord_id(state, token)),
            ("790", f"STAT-{state.slug}-{safe_token(token)}-{seq_for_status(row.time_label)}"),
            ("55", state.symbol),
            ("54", state.side),
        ]
    )
    return encode_fix_message(fields)


def build_cancel_reject(state: ScenarioState, row: ParsedRow, ids: list[str]) -> str:
    token = ids[0]
    orig_token = ids[1] if len(ids) > 1 else token
    order_id = state.order_id_by_token.get(orig_token, "NONE")
    response_to = "2" if "replace" in row.comment.lower() or "replace" in state.title.lower() else "1"

    fields = base_fields(state, "9", "SELL1", "BUY1")
    fields.extend(
        [
            ("37", order_id if order_id != "" else "NONE"),
            ("11", cl_ord_id(state, token)),
            ("41", cl_ord_id(state, orig_token)),
            ("39", ord_status_code(row.ord_status)),
            ("434", response_to),
        ]
    )

    cxl_rej_reason = infer_cancel_reject_reason(row.comment)
    if cxl_rej_reason:
        fields.append(("102", cxl_rej_reason))

    text_value = explicit_text(row.comment)
    if text_value:
        fields.append(("58", text_value))
    return encode_fix_message(fields)


def build_execution_report(
    state: ScenarioState,
    row: ParsedRow,
    ids: list[str],
    *,
    incoming: bool,
) -> str:
    del incoming  # Appendix D execution rows are sell-side reports in practice.

    current_token = ids[0] if ids else "VOICE"
    previous_token = ids[1] if len(ids) > 1 else None
    order_id = ensure_order(state, current_token, replacement_for=previous_token)
    price = order_price(state, row, order_id)
    qty = normalise_qty(row.order_qty) or state.orig_qty_by_order_id.get(order_id, "")
    cash_qty = normalise_qty(row.cash_order_qty) or state.cash_qty_by_order_id.get(order_id, "")
    last_px = infer_last_px(row, price)
    avg_px = infer_avg_px(row, last_px, price)

    exec_type_label = normalise_exec_label(row.exec_type, row.comment)
    fields = base_fields(state, "8", "SELL1", "BUY1")
    fields.append(("37", "NONE" if order_id == "" else order_id))
    if current_token != "VOICE":
        fields.append(("11", cl_ord_id(state, current_token)))
    if previous_token:
        fields.append(("41", cl_ord_id(state, previous_token)))
    fields.append(("17", exec_id_for(state, row.exec_id_ref)))
    if exec_ref_for(row.exec_id_ref):
        fields.append(("19", exec_ref_for(row.exec_id_ref)))
    fields.extend(
        [
            ("150", exec_type_code(exec_type_label)),
            ("39", ord_status_code(row.ord_status)),
            ("55", state.symbol),
            ("54", state.side),
        ]
    )

    if qty:
        fields.append(("38", qty))
        state.orig_qty_by_order_id[order_id] = qty
    if cash_qty:
        fields.append(("152", cash_qty))
        state.cash_qty_by_order_id[order_id] = cash_qty
    if state.ord_type in {"2", "4"}:
        fields.extend([("40", state.ord_type), ("44", price)])
    elif state.ord_type == "3":
        fields.extend([("40", "3"), ("99", state.stop_px or "49.50")])
    else:
        fields.append(("40", state.ord_type))

    maybe_add(fields, "151", normalise_qty(row.leaves_qty))
    maybe_add(fields, "14", normalise_qty(row.cum_qty))
    maybe_add(fields, "32", normalise_qty(row.last_qty))
    maybe_add(fields, "31", last_px)
    maybe_add(fields, "6", avg_px)
    maybe_add(fields, "424", normalise_qty(row.day_order_qty))
    maybe_add(fields, "425", normalise_qty(row.day_cum_qty))

    if "workingindicator = n" in row.comment.lower():
        fields.append(("636", "N"))
    elif "workingindicator = y" in row.comment.lower() or "triggered by" in row.exec_type.lower():
        fields.append(("636", "Y"))

    restatement_reason = infer_restatement_reason(exec_type_label, row.comment, state.title)
    if restatement_reason:
        fields.append(("378", restatement_reason))

    ord_rej_reason = infer_order_reject_reason(row.comment, state.title)
    if ord_rej_reason:
        fields.append(("103", ord_rej_reason))

    text_value = explicit_text(row.comment)
    if not text_value and exec_type_label in {"REJECTED", "ORDER_STATUS", "STOPPED"}:
        text_value = trimmed_comment(row.comment)
    if text_value:
        fields.append(("58", text_value))

    return encode_fix_message(fields)


def base_fields(state: ScenarioState, msg_type: str, sender: str, target: str) -> list[tuple[str, str]]:
    return [
        ("35", msg_type),
        ("49", sender),
        ("56", target),
        ("34", str(next_seq(state, sender, target))),
        ("52", timestamp_for_sequence(state, sender, target)),
    ]


def next_seq(state: ScenarioState, sender: str, target: str) -> int:
    key = (sender, target)
    state.sender_seq[key] = state.sender_seq.get(key, 0) + 1
    return state.sender_seq[key]


def timestamp_for_sequence(state: ScenarioState, sender: str, target: str) -> str:
    seq = state.sender_seq[(sender, target)]
    base_date = "20240101" if state.section == "general" else "20240103"
    seconds = seq - 1
    hour = 9 + seconds // 3600
    minute = 30 + (seconds % 3600) // 60
    second = seconds % 60
    minute += hour // 60
    hour = 9 + minute // 60
    minute %= 60
    return f"{base_date}-{hour:02}:{minute:02}:{second:02}.000"


def timestamp_for(time_label: str) -> str:
    day = 0
    step = 1
    match = re.match(r"Day\s*(\d+),\s*(\d+)", time_label, re.I)
    if match:
        day = int(match.group(1)) - 1
        step = int(match.group(2))
    else:
        digits = re.findall(r"\d+", time_label)
        if digits:
            step = int(digits[-1])
    return f"202401{1 + day:02}-09:30:{step:02}.000"


def seq_for_status(time_label: str) -> str:
    return re.sub(r"[^0-9]", "", time_label) or "1"


def ensure_order(state: ScenarioState, token: str, replacement_for: str | None) -> str:
    if token == "VOICE":
        return f"ORD-{state.slug}-VOICE"
    if replacement_for and replacement_for in state.order_id_by_token:
        order_id = state.order_id_by_token[replacement_for]
    else:
        order_id = state.order_id_by_token.get(token, f"ORD-{state.slug}")
    state.order_id_by_token[token] = order_id
    return order_id


def order_price(state: ScenarioState, row: ParsedRow, order_id: str) -> str:
    if normalise_price(row.price):
        price = normalise_price(row.price)
    else:
        price = state.price_by_order_id.get(order_id, state.default_price)
    state.price_by_order_id[order_id] = price
    return price


def exec_id_for(state: ScenarioState, exec_ref: str) -> str:
    token = exec_token(exec_ref)
    if token:
        if token not in state.exec_ids:
            state.exec_ids[token] = f"EX-{state.slug}-{safe_token(token)}"
        return state.exec_ids[token]
    state.generated_exec_index += 1
    generated = f"AUTO{state.generated_exec_index:03}"
    state.exec_ids[generated] = f"EX-{state.slug}-{generated}"
    return state.exec_ids[generated]


def exec_ref_for(exec_ref: str) -> str | None:
    token = exec_ref_token(exec_ref)
    if not token:
        return None
    return f"EX-REF-{safe_token(token)}"


def exec_token(exec_ref: str) -> str | None:
    text = clean_text(exec_ref)
    if not text:
        return None
    match = EXEC_REF_RE.match(text)
    return match.group(1) if match else None


def exec_ref_token(exec_ref: str) -> str | None:
    text = clean_text(exec_ref)
    if not text:
        return None
    match = EXEC_REF_RE.match(text)
    return match.group(2) if match and match.group(2) else None


def cl_ord_id(state: ScenarioState, token: str) -> str:
    return f"CL-{state.slug}-{safe_token(token)}"


def safe_token(token: str) -> str:
    return slugify(token).upper().replace("-", "_")


def encode_fix_message(fields: list[tuple[str, str]]) -> str:
    fields = canonicalise_fields(fields)
    body = "".join(f"{tag}={value}{SOH}" for tag, value in fields)
    prefix = f"8=FIX.4.4{SOH}9={len(body.encode('ascii'))}{SOH}"
    without_checksum = prefix + body
    checksum = sum(without_checksum.encode("ascii")) % 256
    return without_checksum + f"10={checksum:03}{SOH}"


def maybe_add(fields: list[tuple[str, str]], tag: str, value: str | None) -> None:
    if value not in {None, ""}:
        fields.append((tag, value))


def normalise_qty(value: str) -> str:
    text = clean_text(value).replace(" ", "")
    return text if re.fullmatch(r"[0-9]+(?:\.[0-9]+)?", text) else ""


def normalise_price(value: str) -> str:
    text = clean_text(value).replace(" ", "")
    return text if re.fullmatch(r"[0-9]+(?:\.[0-9]+)?", text) else ""


def infer_last_px(row: ParsedRow, fallback: str) -> str:
    if normalise_price(row.last_px):
        return normalise_price(row.last_px)
    comment = row.comment
    for pattern in [r"LastPx\s*=\s*([0-9.]+)", r"@\s*([0-9.]+)"]:
        match = re.search(pattern, comment, re.I)
        if match:
            return normalise_price(match.group(1))
    return fallback if row.exec_type.strip().lower() in {"trade", "trade correct", "stopped"} else ""


def infer_avg_px(row: ParsedRow, fallback_last_px: str, fallback_price: str) -> str:
    if normalise_price(row.avg_px):
        return normalise_price(row.avg_px)
    match = re.search(r"AvgPx\s*=\s*([0-9.]+)", row.comment, re.I)
    if match:
        return normalise_price(match.group(1))
    if normalise_qty(row.cum_qty):
        if fallback_last_px:
            return fallback_last_px
        if normalise_price(fallback_price):
            return normalise_price(fallback_price)
    return "0"


def normalise_exec_label(exec_type: str, comment: str) -> str:
    cleaned = clean_text(exec_type).upper().replace(" ", "_").replace("-", "_")
    if cleaned == "TRIGGERED_BY_(TRADING_SYSTEM)":
        return "NEW"
    if not cleaned and "nothing done" in comment.lower():
        return "ORDER_STATUS"
    return cleaned


def exec_type_code(label: str) -> str:
    aliases = {
        "REPLACE": "REPLACED",
    }
    return load_enum_map("ExecType")[aliases.get(label, label)]


def ord_status_code(label: str) -> str:
    cleaned = clean_text(label).upper().replace(" ", "_").replace("-", "_")
    cleaned = cleaned.replace("DONE_FOR_DAY", "DONE_FOR_DAY")
    return load_enum_map("OrdStatus")[cleaned]


def infer_restatement_reason(exec_type_label: str, comment: str, title: str) -> str | None:
    lowered = f"{title} {comment}".lower()
    if exec_type_label != "RESTATED":
        if "trading halt" in lowered and "cancel" in lowered:
            return "6"
        if "not best" in lowered:
            return "9"
        return None
    if "corporate action" in lowered or "stock split" in lowered:
        return "0"
    if "renewal" in lowered or "restated(renewed)" in lowered or "renewed" in lowered:
        return "1"
    if "verbal" in lowered:
        return "2"
    if "partial decline" in lowered:
        return "5"
    return "99"


def infer_order_reject_reason(comment: str, title: str) -> str | None:
    lowered = f"{title} {comment}".lower()
    if "duplicate order" in lowered or "duplicate clordid" in lowered:
        return "6"
    if "duplicate of a verbal order" in lowered or "verbally submitted" in lowered:
        return "7"
    if "unknown order" in lowered:
        return "5"
    return None


def infer_cancel_reject_reason(comment: str) -> str | None:
    lowered = comment.lower()
    if "unknown order" in lowered:
        return "1"
    if "pending status" in lowered:
        return "3"
    if "too late" in lowered or "fill has occurred" in lowered:
        return "0"
    return None


def explicit_text(comment: str) -> str | None:
    cleaned = ascii_text(comment)
    cleaned = cleaned.replace("Text=", "Text = ")
    for pattern in [r'Text\s*=\s*"([^"]+)"', r"Text\s*=\s*'([^']+)'"]:
        match = re.search(pattern, cleaned, re.I)
        if match:
            return match.group(1)
    return None


def trimmed_comment(comment: str) -> str | None:
    text = ascii_text(comment)
    if not text:
        return None
    if len(text) > 120:
        return text[:117].rstrip() + "..."
    return text


def sample_path_for_variant(variant: ScenarioVariant) -> Path:
    filename = f"{slugify(variant.scenario_id)}_{variant.variant}_{variant.slug}.fix"
    return OUTPUT_DIR / variant.section / filename


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
    for directory in [OUTPUT_DIR / "general", OUTPUT_DIR / "exchange"]:
        directory.mkdir(parents=True, exist_ok=True)

    current_files = load_current_generated_files()
    for path in current_files:
        if path not in expected_files:
            path.unlink()

    for path, content in expected_files.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)


def load_enum_map(field_name: str) -> dict[str, str]:
    return load_fix44_metadata().enum_map[field_name]


def canonicalise_fields(fields: list[tuple[str, str]]) -> list[tuple[str, str]]:
    msg_type = next((value for tag, value in fields if tag == "35"), None)
    if not msg_type:
        return fields

    metadata = load_fix44_metadata()
    allowed_tags = metadata.message_allowed_tags.get(msg_type)
    order = metadata.message_order.get(msg_type)
    if not allowed_tags or not order:
        return fields

    order_index = {tag: idx for idx, tag in enumerate(order)}
    ordered: list[tuple[int, int, tuple[str, str]]] = []

    for position, field in enumerate(fields):
        tag, _ = field
        if tag not in allowed_tags:
            continue
        ordered.append((order_index.get(tag, len(order)), position, field))

    ordered.sort(key=lambda item: (item[0], item[1]))
    return [field for _, _, field in ordered]


def load_fix44_metadata() -> Fix44Metadata:
    if hasattr(load_fix44_metadata, "_cache"):
        return load_fix44_metadata._cache  # type: ignore[attr-defined]

    root = ET.parse(FIX44_XML_PATH).getroot()
    fields_root = root.find("fields")
    header = root.find("header")
    trailer = root.find("trailer")
    messages_root = root.find("messages")
    components_root = root.find("components")
    if fields_root is None or header is None or trailer is None or messages_root is None:
        raise RuntimeError("FIX44.xml is missing required sections")

    name_to_tag: dict[str, str] = {}
    enum_map: dict[str, dict[str, str]] = {}
    for field in fields_root.findall("field"):
        name = field.attrib.get("name")
        number = field.attrib.get("number")
        if not name or not number:
            continue
        name_to_tag[name] = number
        enum_map[name] = {
            value.attrib["description"]: value.attrib["enum"]
            for value in field.findall("value")
            if "description" in value.attrib and "enum" in value.attrib
        }

    components = {}
    if components_root is not None:
        components = {
            component.attrib["name"]: component
            for component in components_root.findall("component")
            if "name" in component.attrib
        }

    header_tags = expand_container_tags(header, components, name_to_tag)
    trailer_tags = expand_container_tags(trailer, components, name_to_tag)
    message_order: dict[str, list[str]] = {}
    message_allowed_tags: dict[str, set[str]] = {}

    for message in messages_root.findall("message"):
        msg_type = message.attrib.get("msgtype")
        if not msg_type:
            continue
        tags = dedupe_tags(
            header_tags + expand_container_tags(message, components, name_to_tag) + trailer_tags
        )
        message_order[msg_type] = tags
        message_allowed_tags[msg_type] = set(tags)

    metadata = Fix44Metadata(
        enum_map=enum_map,
        message_order=message_order,
        message_allowed_tags=message_allowed_tags,
    )
    load_fix44_metadata._cache = metadata  # type: ignore[attr-defined]
    return metadata


def expand_container_tags(
    container: ET.Element,
    components: dict[str, ET.Element],
    name_to_tag: dict[str, str],
) -> list[str]:
    tags: list[str] = []
    for child in list(container):
        name = child.attrib.get("name")
        if child.tag == "field" and name:
            tag = name_to_tag.get(name)
            if tag:
                tags.append(tag)
        elif child.tag == "component" and name and name in components:
            tags.extend(expand_container_tags(components[name], components, name_to_tag))
        elif child.tag == "group" and name:
            tag = name_to_tag.get(name)
            if tag:
                tags.append(tag)
            tags.extend(expand_container_tags(child, components, name_to_tag))
    return tags


def dedupe_tags(tags: list[str]) -> list[str]:
    seen: set[str] = set()
    deduped: list[str] = []
    for tag in tags:
        if tag in seen:
            continue
        seen.add(tag)
        deduped.append(tag)
    return deduped


def slugify(text: str) -> str:
    ascii_only = ascii_text(text).lower()
    return re.sub(r"[^a-z0-9]+", "_", ascii_only).strip("_")


def clean_text(text: str) -> str:
    return " ".join(ascii_text(text).split())


def ascii_text(text: str) -> str:
    return (
        text.replace("\xa0", " ")
        .replace("’", "'")
        .replace("‘", "'")
        .replace("“", '"')
        .replace("”", '"')
        .replace("–", "-")
        .replace("—", "-")
        .replace("…", "...")
        .replace("×", "x")
    )


if __name__ == "__main__":
    raise SystemExit(main())
