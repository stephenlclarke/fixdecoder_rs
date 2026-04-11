#!/usr/bin/env python3
"""
Normalise a Cobertura XML report for SonarCloud import.

The Rust coverage sensor is strict about line numbers being within the current
checked-out file. Some `cargo llvm-cov` reports can also contain entries for
external Rust stdlib sources that do not exist in CI workspaces. This helper
removes those entries and drops any line hits that fall outside the current
file length.
"""

from __future__ import annotations

import sys
import xml.etree.ElementTree as ET
from pathlib import Path


def file_line_count(path: Path) -> int:
    with path.open(encoding="utf-8") as fh:
        return sum(1 for _ in fh)


def resolve_path(base_dir: Path, filename: str) -> Path:
    path = Path(filename)
    if path.is_absolute():
        return path
    return base_dir / path


def normalise_report(path: Path) -> tuple[int, int]:
    tree = ET.parse(path)
    root = tree.getroot()
    base_dir = Path.cwd()
    removed_classes = 0
    removed_lines = 0

    for classes in root.findall(".//classes"):
        for class_elem in list(classes.findall("class")):
            filename = class_elem.get("filename", "")
            resolved = resolve_path(base_dir, filename)
            if not resolved.exists():
                classes.remove(class_elem)
                removed_classes += 1
                continue

            max_line = file_line_count(resolved)
            lines_elem = class_elem.find("lines")
            if lines_elem is None:
                continue

            for line_elem in list(lines_elem.findall("line")):
                try:
                    number = int(line_elem.get("number", "0"))
                except ValueError:
                    lines_elem.remove(line_elem)
                    removed_lines += 1
                    continue

                if number < 1 or number > max_line:
                    lines_elem.remove(line_elem)
                    removed_lines += 1

            if not list(lines_elem):
                classes.remove(class_elem)
                removed_classes += 1

    tree.write(path, encoding="utf-8", xml_declaration=True)
    return removed_classes, removed_lines


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: normalise_cobertura.py <coverage.xml>", file=sys.stderr)
        return 1

    report_path = Path(sys.argv[1])
    if not report_path.exists():
        print(f"coverage report not found: {report_path}", file=sys.stderr)
        return 1

    removed_classes, removed_lines = normalise_report(report_path)
    print(
        f"normalised Cobertura report: removed {removed_classes} classes and {removed_lines} line entries"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
