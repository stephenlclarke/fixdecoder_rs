#!/usr/bin/env python3
"""
Patch the workspace package versions (and Cargo.lock if present).

Usage:
    python bump_version.py <current_version> <new_version>
"""

from __future__ import annotations

import re
import sys
from pathlib import Path


def bump_patch(version: str) -> str:
    """Return the next patch version from a semver string."""
    try:
        major, minor, patch = map(int, version.split("."))
    except ValueError as exc:
        raise ValueError(f"Invalid semver version: {version}") from exc
    return f"{major}.{minor}.{patch + 1}"


WORKSPACE_PACKAGES = {
    "fixdecoder": Path("Cargo.toml"),
    "pcap2fix": Path("pcap2fix/Cargo.toml"),
}


def update_manifest(path: Path, cur: str, new: str) -> bool:
    """Update one workspace package manifest."""
    text = path.read_text(encoding="utf-8")
    pattern = rf'^version\s*=\s*"{re.escape(cur)}"'
    updated, count = re.subn(pattern, f'version = "{new}"', text, count=1, flags=re.M)
    if count == 0:
        print(f"failed to find version {cur} in {path}", file=sys.stderr)
        return False
    path.write_text(updated, encoding="utf-8")
    return True


def update_lockfile(cur: str, new: str) -> bool:
    """Update every workspace package version in Cargo.lock, if it exists."""
    lock_path = Path("Cargo.lock")
    if not lock_path.exists():
        return True

    text = lock_path.read_text(encoding="utf-8")
    for package in WORKSPACE_PACKAGES:
        pattern = rf'(?m)^name = "{re.escape(package)}"\nversion = "{re.escape(cur)}"'
        replacement = f'name = "{package}"\nversion = "{new}"'
        text, count = re.subn(pattern, replacement, text, count=1)
        if count == 0:
            print(
                f"failed to find {package} version {cur} in Cargo.lock",
                file=sys.stderr,
            )
            return False

    lock_path.write_text(text, encoding="utf-8")
    return True


def main() -> int:
    """Bump the version in Cargo.toml from current to next (auto-increment if missing)."""
    if len(sys.argv) < 2:
        print("usage: bump_version.py <current> [next]", file=sys.stderr)
        return 1

    cur = sys.argv[1]
    new = sys.argv[2] if len(sys.argv) > 2 else bump_patch(cur)
    for path in WORKSPACE_PACKAGES.values():
        if not update_manifest(path, cur, new):
            return 1

    if not update_lockfile(cur, new):
        return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
