#!/usr/bin/env bash
# Run our development binary in its own named session.
set -euo pipefail
script_path=$(readlink -f "${BASH_SOURCE[0]}")
repo_dir=$(cd "$(dirname "$script_path")/.." && pwd)
binary="$repo_dir/target/debug/herdr"
if [[ "${1:-}" == --build ]]; then
  "$repo_dir/scripts/build-test.sh"
  shift
fi
if [[ ! -x "$binary" ]]; then
  echo "Test binary missing. Run: herdr-test --build" >&2
  exit 1
fi
exec env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH -u HERDR_SESSION \
  "$binary" --session herdr-test "$@"
