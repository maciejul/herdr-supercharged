#!/usr/bin/env zsh
# Run directly: zsh scripts/test-codex-herdr.zsh. Never launches a real agent.
emulate -LR zsh
set -eu
setopt pipefail

helper=${0:A:h}/codex-herdr.zsh
test_dir=$(mktemp -d "${TMPDIR:-/tmp}/codex-herdr-test.XXXXXX")
trap 'rm -rf -- "$test_dir"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir "$test_dir/bin"

cat > "$test_dir/bin/codex" <<'FAKE'
#!/usr/bin/env zsh
: > "$HERDR_WRAPPER_TEST_RECORD"
for argument in "$@"; do
    printf '%s\0' "$argument" >> "$HERDR_WRAPPER_TEST_RECORD"
done
exit "${HERDR_WRAPPER_TEST_EXIT:-0}"
FAKE
chmod +x "$test_dir/bin/codex"

integer assertions=0
assert_call() {
    local label=$1 herdr_env=$2 mode=$3 expected_exit=$4
    shift 4
    local argument actual_exit=0
    : > "$test_dir/expected"
    if [[ $mode == inject ]]; then
        printf '%s\0' --no-daemon >> "$test_dir/expected"
    fi
    for argument in "$@"; do
        printf '%s\0' "$argument" >> "$test_dir/expected"
    done
    env PATH="$test_dir/bin:$PATH" HERDR_ENV="$herdr_env" \
        HERDR_WRAPPER_TEST_RECORD="$test_dir/actual" \
        HERDR_WRAPPER_TEST_EXIT="$expected_exit" \
        zsh -f -c 'source "$1"; shift; codex "$@"' \
        wrapper-test "$helper" "$@" || actual_exit=$?
    if (( actual_exit != expected_exit )) || \
        ! cmp -s "$test_dir/expected" "$test_dir/actual"; then
        print -u2 -r -- "FAIL: $label (exit $actual_exit, expected $expected_exit)"
        return 1
    fi
    (( ++assertions ))
}

assert_call fresh 1 inject 0
assert_call prompt 1 inject 0 'Fix login without changing the API'
assert_call resume 1 inject 0 resume session-id
assert_call fork 1 inject 0 fork --last
assert_call options_before_resume 1 inject 0 -m model-name -c 'foo="bar baz"' resume session-id
assert_call option_value_is_command 1 inject 0 --model exec
assert_call option_value_is_remote 1 inject 0 --config --remote
assert_call equals_options 1 inject 0 '--config=foo="bar baz"' --model=model-name prompt
assert_call literal_options 1 inject 0 -- --remote help
assert_call literal_subcommand 1 inject 0 -- exec
assert_call resume_prompt 1 inject 0 resume session-id exec
assert_call explicit_no_daemon 1 passthrough 0 --no-daemon resume session-id
assert_call remote 1 passthrough 0 --remote unix:///tmp/example.sock
assert_call remote_equals 1 passthrough 0 resume --remote=unix:///tmp/example.sock session-id
assert_call admin_after_option 1 passthrough 0 -m model-name doctor --json
assert_call unknown_option 1 passthrough 0 --future-option value
assert_call clustered_option 1 passthrough 0 -mmodel-name
assert_call variadic_images 1 passthrough 0 --image one.png two.png prompt
assert_call missing_value 1 passthrough 0 --model
assert_call literal_shell_text 1 inject 0 '$(echo unsafe); `echo unsafe`' $'two\nlines' ''

for subcommand in agents exec e review login logout mcp plugin app-server \
    remote-control completion update doctor sandbox debug apply a queue archive \
    delete migrate-rollouts migrate-nfs-db unarchive cloud exec-server features help; do
    assert_call "subcommand_$subcommand" 1 passthrough 0 "$subcommand"
done
for flag in --help -h --version -V; do
    assert_call "flag_$flag" 1 passthrough 0 "$flag"
done
for outside in '' 0 true; do
    assert_call "outside_herdr_$outside" "$outside" passthrough 0 resume session-id
done
assert_call interactive_exit_status 1 inject 23
assert_call admin_exit_status 1 passthrough 23 doctor
assert_call remote_exit_status 1 passthrough 23 --remote unix:///tmp/example.sock

print -r -- "PASS: $assertions executable Codex-wrapper assertions; no real agents launched"
