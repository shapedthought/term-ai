# term-ai zsh integration
#
# Records each command and its exit code so `term-ai --fix` can suggest
# a corrected command after a failure.
#
# Install: add this line to your ~/.zshrc
#   source /path/to/term-ai/shell-integrations/zsh/term-ai.zsh

_TERM_AI_STATE_DIR="${TERM_AI_STATE_DIR:-$HOME/.term-ai}"

_term_ai_preexec() {
    _TERM_AI_LAST_CMD=$1
}

_term_ai_precmd() {
    local exit_code=$?
    [[ -z $_TERM_AI_LAST_CMD ]] && return
    # Don't record term-ai itself, or --fix would try to fix its own invocation
    if [[ $_TERM_AI_LAST_CMD == term-ai* || $_TERM_AI_LAST_CMD == *"/term-ai"* ]]; then
        _TERM_AI_LAST_CMD=""
        return
    fi
    mkdir -p "$_TERM_AI_STATE_DIR"
    {
        print -r -- "$exit_code"
        print -r -- "$_TERM_AI_LAST_CMD"
    } >| "$_TERM_AI_STATE_DIR/last_command"
    _TERM_AI_LAST_CMD=""
}

autoload -Uz add-zsh-hook
add-zsh-hook preexec _term_ai_preexec
add-zsh-hook precmd _term_ai_precmd
