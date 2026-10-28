#!/usr/bin/env bash
# Merge public upstream into our master; publishing is opt-in.
set -euo pipefail

push=false
case "${1:-}" in
  '') ;;
  --push) push=true ;;
  --help|-h)
    echo 'Usage: scripts/sync-upstream.sh [--push]'
    echo 'Requires clean master. Fetches origin/upstream and merges upstream/master.'
    echo 'With --push, publishes master to origin after a successful merge.'
    exit 0 ;;
  *) echo 'Unknown option. Use --help.' >&2; exit 2 ;;
esac
[[ $# -le 1 ]] || { echo 'Too many arguments.' >&2; exit 2; }
cd "$(dirname "${BASH_SOURCE[0]}")/.."
fail() { echo "$*" >&2; exit 1; }

[[ "$(git branch --show-current)" == master ]] || fail 'Switch to master before syncing.'
[[ -z "$(git status --porcelain)" ]] || fail 'Commit or stash changes (including untracked files) before syncing.'
for state in MERGE_HEAD CHERRY_PICK_HEAD REVERT_HEAD rebase-merge rebase-apply; do
  [[ ! -e "$(git rev-parse --git-path "$state")" ]] || fail 'Finish or abort the current Git operation before syncing.'
done

check_remote() {
  local remote=$1 expected=$2 mode url
  for mode in fetch push; do
    if [[ "$mode" == push ]]; then
      url=$(git remote get-url --push --all "$remote")
    else
      url=$(git remote get-url --all "$remote")
    fi
    case "$url" in
      "https://github.com/$expected"|"https://github.com/$expected.git"|"git@github.com:$expected.git"|"ssh://git@github.com/$expected.git") ;;
      *) fail "Unexpected $mode URL for $remote; expected github.com/$expected." ;;
    esac
  done
}
check_remote origin maciejul/herdr-supercharged
check_remote upstream herdrdev/herdr

git fetch --no-tags origin refs/heads/master:refs/remotes/origin/master
# Incorporate updates from origin without silently reconciling divergent local work.
if ! git merge-base --is-ancestor origin/master HEAD; then
  git merge --ff-only origin/master
fi
git fetch --no-tags upstream refs/heads/master:refs/remotes/upstream/master
if ! git merge --no-edit upstream/master; then
  echo 'Sync stopped. Resolve conflicts and commit, or run git merge --abort. Nothing was pushed.' >&2
  exit 1
fi
if "$push"; then
  git push origin refs/heads/master:refs/heads/master
else
  echo 'Merged locally. Review and test, then publish with: git push origin master'
fi
