#!/usr/bin/env bash
set -euo pipefail
repo_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_dir"
export PATH="$HOME/.cargo/bin:$PATH"
if [[ -z "${ZIG:-}" ]] && ! command -v zig >/dev/null 2>&1; then
  export ZIG="$HOME/.local/share/herdr-build/zig-x86_64-linux-0.16.0/zig"
fi
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_INCREMENTAL=0
exec cargo build --locked --bin herdr
