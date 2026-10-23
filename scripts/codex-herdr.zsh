# Source from ~/.zshrc; requires a Codex version that supports --no-daemon.
# Herdr hooks need the current pane's environment. A shared Codex daemon may
# retain HERDR_* values from the pane that originally started it.
# This wrapper affects interactive launches only; `command codex` bypasses it.
function codex {
    emulate -L zsh

    if [[ ${HERDR_ENV:-} != 1 ]]; then
        command codex "$@"
        return $?
    fi

    local argument
    local value_pending=0
    local positional_seen=0
    for argument in "$@"; do
        if (( value_pending )); then
            value_pending=0
            continue
        fi

        case "$argument" in
            --)
                # Everything following -- is literal input, including words
                # that would otherwise name a subcommand or option.
                break
                ;;
            --remote|--remote=*|--no-daemon|-h|--help|-V|--version)
                command codex "$@"
                return $?
                ;;
            -c|--config|-m|--model|--local-provider|-p|--profile|-s|--sandbox|\
            -C|--cd|--add-dir|-a|--ask-for-approval|--enable|--disable|\
            --remote-auth-token-env)
                value_pending=1
                ;;
            --config=*|--model=*|--local-provider=*|--profile=*|--sandbox=*|\
            --cd=*|--add-dir=*|--ask-for-approval=*|--enable=*|--disable=*|\
            --remote-auth-token-env=*)
                ;;
            --oss|--approve-for-me|--dangerously-bypass-approvals-and-sandbox|\
            --dangerously-bypass-hook-trust|--strict-config|--worktree|\
            --search|--no-alt-screen|--last|--all)
                ;;
            -*)
                # Unknown/clustered options and variadic image arguments are
                # ambiguous without Codex's own parser. Preserve them exactly.
                command codex "$@"
                return $?
                ;;
            *)
                if (( ! positional_seen )); then
                    positional_seen=1
                    case "$argument" in
                        resume|fork) ;;
                        agents|exec|e|review|login|logout|mcp|plugin|app-server|\
                        remote-control|completion|update|doctor|sandbox|debug|\
                        apply|a|queue|archive|delete|migrate-rollouts|\
                        migrate-nfs-db|unarchive|cloud|exec-server|features|help)
                            command codex "$@"
                            return $?
                            ;;
                        *) ;; # The first positional argument is a prompt.
                    esac
                fi
                ;;
        esac
    done

    if (( value_pending )); then
        # Let Codex diagnose an option missing its value without modification.
        command codex "$@"
    else
        command codex --no-daemon "$@"
    fi
}
