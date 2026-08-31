#!/usr/bin/env bash
# Scan the complete repository history through the fail-closed Rust gate verb.
set -euo pipefail
repository_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"
cargo xtask check-secrets
