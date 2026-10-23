#!/usr/bin/env bash
# Replace the user-local release binary without interrupting running servers.
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/install-private-release.sh [vMAJOR.MINOR.PATCH-supercharged.NUMBER]

Defaults to v0.9.5-supercharged.4. Requires authenticated GitHub CLI access to
Maciejul/herdr-supercharged and Linux x86_64 with glibc 2.39 or newer.
Installs to ~/.local/bin/herdr, keeping a timestamped backup beside it.
Does not stop servers, change configuration, or install agent integrations.
EOF
}

if [[ "${1:-}" == --help || "${1:-}" == -h ]]; then
  usage
  exit 0
fi
tag=${1:-v0.9.5-supercharged.4}
if (( $# > 1 )) || [[ ! "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+-supercharged\.[0-9]+$ ]]; then
  usage >&2
  exit 1
fi
if [[ $(uname -s) != Linux || $(uname -m) != x86_64 ]]; then
  echo 'This private release installer supports Linux x86_64 only (including WSL).' >&2
  exit 1
fi
for tool in gh sha256sum getconf sort mktemp install; do
  command -v "$tool" >/dev/null || { echo "Missing required command: $tool" >&2; exit 1; }
done
glibc=$(getconf GNU_LIBC_VERSION) || exit 1
if [[ "$glibc" != glibc\ * ]] || [[ $(printf '%s\n' 2.39 "${glibc#glibc }" | sort -V | head -n1) != 2.39 ]]; then
  echo "Requires glibc 2.39 or newer; found: $glibc" >&2
  exit 1
fi

destination="$HOME/.local/bin/herdr"
if [[ -L "$destination" || ( -e "$destination" && ! -f "$destination" ) ]]; then
  echo "Refusing to replace a symlink or non-regular file: $destination" >&2
  exit 1
fi
download_dir=$(mktemp -d)
staged=''
cleanup() {
  rm -rf -- "$download_dir"
  if [[ -n "$staged" ]]; then rm -f -- "$staged"; fi
}
trap cleanup EXIT

echo "Downloading $tag from Maciejul/herdr-supercharged..."
gh release download "$tag" --repo Maciejul/herdr-supercharged \
  --dir "$download_dir" \
  --pattern herdr-linux-x86_64 --pattern SHA256SUMS --pattern build-metadata.json
(
  cd "$download_dir"
  # Require checksums for both downloaded assets before running the executable.
  for asset in herdr-linux-x86_64 build-metadata.json; do
    if ! grep -Eq "^[[:xdigit:]]{64} [ *]${asset//./\\.}$" SHA256SUMS; then
      echo "Missing checksum for $asset" >&2
      exit 1
    fi
  done
  sha256sum --check --strict SHA256SUMS
)
chmod +x "$download_dir/herdr-linux-x86_64"
version=$("$download_dir/herdr-linux-x86_64" --version)
if [[ "$version" != "herdr ${tag#v}" ]]; then
  echo "Unexpected release version: $version (expected herdr ${tag#v})" >&2
  exit 1
fi

mkdir -p "$(dirname "$destination")"
staged=$(mktemp "${destination}.new.XXXXXX")
install -m755 "$download_dir/herdr-linux-x86_64" "$staged"
if [[ -f "$destination" ]]; then
  backup=$(mktemp "${destination}.backup-$(date +%Y%m%d-%H%M%S).XXXXXX")
  cp -p -- "$destination" "$backup"
  printf 'Previous executable saved to: %s\n' "$backup"
fi
mv -f -- "$staged" "$destination"
staged=''
printf 'Installed %s at %s\n' "$version" "$destination"
cat <<'EOF'

Running servers still use their existing executable. When active work is finished,
run `herdr server stop`, then `herdr` from an external terminal. Stopping a server
ends its live pane processes. Restart named sessions separately when ready.
Refresh Claude/Codex integrations and restart agent sessions to load new hooks.
Use this script for private upgrades: `herdr update` still targets upstream.
EOF
