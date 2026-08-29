#!/usr/bin/env python3
"""Reject a Rust toolchain version that the three pinning files do not agree on.

`rust-toolchain.toml` decides which compiler builds this repository, `Cargo.toml`
`rust-version` states the minimum it claims to support, and the `Dockerfile` builder
tag decides which compiler the image is built with. A commit that changes one and
not the others reintroduces the local/CI clippy drift the pin exists to remove.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
REQUIRED_COMPONENTS = ("rustfmt", "clippy")
TABLE = re.compile(r"^\s*\[([^\]]+)\]\s*$")
CHANNEL = re.compile(r"""^\s*channel\s*=\s*["']([^"']+)["']""")
RUST_VERSION = re.compile(r"""^\s*rust-version\s*=\s*["']([^"']+)["']""")
COMPONENTS = re.compile(r"^\s*components\s*=\s*\[([^\]]*)\]")
BUILDER = re.compile(r"^\s*FROM\s+rust:([0-9][^\s@-]*)")


def read_lines(root: Path, name: str, failures: list[str]) -> list[str] | None:
    path = root / name
    if not path.is_file():
        failures.append(f"{name}: missing under {root}; the pinned version cannot be checked")
        return None
    return path.read_text(encoding="utf-8").splitlines()


def scalar_in_table(
    lines: list[str], table: str, pattern: re.Pattern[str]
) -> tuple[str, int] | None:
    """Return the first value `pattern` captures inside `[table]`, with its line number."""
    current = ""
    for number, line in enumerate(lines, 1):
        heading = TABLE.match(line)
        if heading:
            current = heading.group(1).strip()
            continue
        if current != table:
            continue
        match = pattern.match(line)
        if match:
            return match.group(1), number
    return None


def toolchain_channel(root: Path, failures: list[str]) -> tuple[str, str] | None:
    lines = read_lines(root, "rust-toolchain.toml", failures)
    if lines is None:
        return None
    found = scalar_in_table(lines, "toolchain", CHANNEL)
    if found is None:
        failures.append("rust-toolchain.toml: no [toolchain] channel")
        return None
    channel, number = found
    components = scalar_in_table(lines, "toolchain", COMPONENTS)
    declared = (
        {value.strip().strip("\"'") for value in components[0].split(",") if value.strip()}
        if components
        else set()
    )
    for component in REQUIRED_COMPONENTS:
        if component not in declared:
            failures.append(
                f"rust-toolchain.toml:{components[1] if components else number}: "
                f"components must include {component!r}; the gate runs it"
            )
    return channel, f"rust-toolchain.toml:{number}: channel = \"{channel}\""


def cargo_rust_version(root: Path, failures: list[str]) -> tuple[str, str] | None:
    lines = read_lines(root, "Cargo.toml", failures)
    if lines is None:
        return None
    found = scalar_in_table(lines, "workspace.package", RUST_VERSION)
    if found is None:
        failures.append("Cargo.toml: no [workspace.package] rust-version")
        return None
    version, number = found
    return version, f"Cargo.toml:{number}: rust-version = \"{version}\""


def dockerfile_builder(root: Path, failures: list[str]) -> tuple[str, str] | None:
    lines = read_lines(root, "Dockerfile", failures)
    if lines is None:
        return None
    for number, line in enumerate(lines, 1):
        match = BUILDER.match(line)
        if match:
            version = match.group(1)
            return version, f"Dockerfile:{number}: FROM rust:{version}-…"
    failures.append("Dockerfile: no `FROM rust:<version>-…` builder stage")
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=ROOT,
        help="repository root to check (default: the repository this script lives in)",
    )
    arguments = parser.parse_args()
    root = arguments.root.resolve()

    failures: list[str] = []
    readings = [
        toolchain_channel(root, failures),
        cargo_rust_version(root, failures),
        dockerfile_builder(root, failures),
    ]
    if any(reading is None for reading in readings):
        print("\n".join(failures), file=sys.stderr)
        return 1

    versions = {reading[0] for reading in readings if reading is not None}
    if len(versions) > 1:
        failures.append(
            "Rust toolchain version disagreement; a bump is one commit that changes all three:"
        )
        failures.extend(f"  {reading[1]}" for reading in readings if reading is not None)
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1

    print(f"Rust toolchain pinned at {versions.pop()} in rust-toolchain.toml, Cargo.toml, Dockerfile")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
