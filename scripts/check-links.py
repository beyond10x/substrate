#!/usr/bin/env python3
"""Reject machine-local Markdown links and broken repository-relative targets."""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path
from urllib.parse import unquote, urlsplit


ROOT = Path(__file__).resolve().parent.parent
REPOSITORY_ROOT = next(
    (
        candidate
        for candidate in (ROOT, *ROOT.parents)
        if (candidate / "scripts/check-monorepo.sh").is_file()
    ),
    ROOT,
)
LINK = re.compile(r"!?\[[^\]]*\]\(([^)]+)\)")
EXTERNAL = {"http", "https", "mailto"}
LOCAL_PREFIXES = ("/", "~/", "file://", "vscode://")


def target_text(raw: str) -> str:
    value = raw.strip()
    if value.startswith("<") and ">" in value:
        return value[1 : value.index(">")]
    return value.split(maxsplit=1)[0]


def main() -> int:
    failures: list[str] = []
    listed = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "--", "*.md"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    for relative in sorted(filter(None, listed.stdout.splitlines())):
        document = ROOT / relative
        if not document.is_file():
            continue
        source = document.read_text(encoding="utf-8")
        for line_number, line in enumerate(source.splitlines(), 1):
            for match in LINK.finditer(line):
                target = target_text(match.group(1))
                if not target or target.startswith("#"):
                    continue
                parsed = urlsplit(target)
                if target.startswith(LOCAL_PREFIXES) or re.match(r"^[a-zA-Z]:[\\/]", target):
                    failures.append(
                        f"{document.relative_to(ROOT)}:{line_number}: machine-local link: {target}"
                    )
                    continue
                if parsed.scheme:
                    if parsed.scheme not in EXTERNAL:
                        failures.append(
                            f"{document.relative_to(ROOT)}:{line_number}: unsupported link scheme: {target}"
                        )
                    continue
                path_text = unquote(parsed.path)
                if not path_text:
                    continue
                resolved = (document.parent / path_text).resolve()
                try:
                    resolved.relative_to(REPOSITORY_ROOT)
                except ValueError:
                    failures.append(
                        f"{document.relative_to(ROOT)}:{line_number}: link escapes repository: {target}"
                    )
                    continue
                if not resolved.exists():
                    failures.append(
                        f"{document.relative_to(ROOT)}:{line_number}: missing link target: {target}"
                    )

    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print("markdown links are repository-portable")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
