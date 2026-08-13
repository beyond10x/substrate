#!/usr/bin/env python3
"""Validate ADR identity, frontmatter, index agreement, and supersession links."""

from __future__ import annotations

import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
ADR = re.compile(r"^(?P<number>[0-9]{4})-(?P<slug>.+)\.md$")
DATE = re.compile(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$")
HEADING = re.compile(r"^# ADR (?P<number>[0-9]{4}):\s+.+$")
INDEX_ROW = re.compile(
    r"^\| \[(?P<number>[0-9]{4})\]\((?P<file>[0-9]{4}-[^)]+\.md)\)"
    r" \| .+ \| (?P<status>[^|]+?) \|$"
)
STATUSES = {"accepted", "superseded"}


def frontmatter(path: Path, failures: list[str]) -> tuple[dict[str, str], list[str]]:
    lines = path.read_text(encoding="utf-8").splitlines()
    relative = path.relative_to(ROOT)
    if not lines or lines[0] != "---":
        failures.append(f"{relative}: missing opening YAML frontmatter fence")
        return {}, lines
    try:
        end = lines.index("---", 1)
    except ValueError:
        failures.append(f"{relative}: missing closing YAML frontmatter fence")
        return {}, lines
    fields: dict[str, str] = {}
    for line in lines[1:end]:
        key, separator, value = line.partition(":")
        if separator:
            fields[key.strip()] = value.strip()
    return fields, lines[end + 1 :]


def main() -> int:
    failures: list[str] = []
    records: dict[str, tuple[Path, str, str]] = {}

    for path in sorted((ROOT / "adr").glob("*.md")):
        match = ADR.match(path.name)
        if not match:
            continue
        number = match.group("number")
        if number in records:
            failures.append(
                f"adr/{path.name}: duplicate ADR number {number}; also used by {records[number][0].name}"
            )
            continue
        fields, body = frontmatter(path, failures)
        status = fields.get("status", "")
        if status not in STATUSES:
            failures.append(f"adr/{path.name}: status must be one of {sorted(STATUSES)}")
        if not DATE.fullmatch(fields.get("date", "")):
            failures.append(f"adr/{path.name}: date must use YYYY-MM-DD")
        heading = next((line for line in body if line.strip()), "")
        heading_match = HEADING.fullmatch(heading)
        if not heading_match or heading_match.group("number") != number:
            failures.append(f"adr/{path.name}: first heading must be '# ADR {number}: …'")
        text = "\n".join(body)
        records[number] = (path, status, text)

    index: dict[str, tuple[str, str]] = {}
    readme = ROOT / "adr" / "README.md"
    for line_number, line in enumerate(readme.read_text(encoding="utf-8").splitlines(), 1):
        match = INDEX_ROW.fullmatch(line)
        if not match:
            continue
        number, filename, status_text = (
            match.group("number"),
            match.group("file"),
            match.group("status").strip(),
        )
        if number in index:
            failures.append(f"adr/README.md:{line_number}: duplicate ADR row {number}")
        index[number] = (filename, status_text)

    for number, (path, status, text) in records.items():
        if number not in index:
            failures.append(f"adr/README.md: missing row for ADR {number}")
            continue
        filename, status_text = index[number]
        if filename != path.name:
            failures.append(f"adr/README.md: ADR {number} links {filename}, expected {path.name}")
        if status == "accepted" and status_text != "accepted":
            failures.append(f"adr/README.md: ADR {number} status disagrees with frontmatter")
        if status == "superseded":
            match = re.fullmatch(r"superseded by ([0-9]{4})", status_text)
            if not match:
                failures.append(
                    f"adr/README.md: superseded ADR {number} must say 'superseded by NNNN'"
                )
                continue
            successor = match.group(1)
            if successor not in records or records[successor][1] != "accepted":
                failures.append(f"adr/README.md: ADR {number} successor {successor} is not accepted")
            forward = re.compile(rf"superseded by \[ADR {successor}\]\({successor}-[^)]+\.md\)", re.I)
            back = re.compile(rf"supersedes \[ADR {number}\]\({number}-[^)]+\.md\)", re.I)
            if not forward.search(text):
                failures.append(f"adr/{path.name}: missing linked 'superseded by ADR {successor}'")
            if successor in records and not back.search(records[successor][2]):
                failures.append(
                    f"adr/{records[successor][0].name}: missing linked 'supersedes ADR {number}'"
                )

    for number in sorted(set(index) - set(records)):
        failures.append(f"adr/README.md: row {number} has no ADR file")

    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print(f"ADR index and {len(records)} records are consistent")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
