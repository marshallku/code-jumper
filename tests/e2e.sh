#!/usr/bin/env bash
# End-to-end test: a real tmux session with a real Neovim running the plugin in
# a pane. Verifies the load-bearing path:
#   1. nvim registers itself (registry tsv + live socket)
#   2. launcher opens the right file at the right line/col — even when nvim is in
#      insert mode (the <C-\><C-N> + RPC-ordering guarantee codex flagged)
#   3. launcher focuses the nvim pane
#
# Uses an isolated tmux server socket so it can't disturb the user's tmux.
set -u

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
LAUNCHER="$REPO/bin/code-jumper"

command -v tmux >/dev/null || { echo "SKIP: tmux not installed"; exit 0; }
command -v nvim >/dev/null || { echo "SKIP: nvim not installed"; exit 0; }

WORK="$(mktemp -d)"
TMUX_SOCK="$WORK/tmux.sock"
SESS="cjtest"
# Isolate the registry dir so we don't read the user's real nvims.
export XDG_RUNTIME_DIR="$WORK/xdg"
mkdir -p "$XDG_RUNTIME_DIR"
# Isolate XDG_CONFIG_HOME (empty) so the registry resolution can't pick up a real
# ~/.config/code-jumper/config pin and diverge from this test's XDG_RUNTIME_DIR.
export XDG_CONFIG_HOME="$WORK/cfg"
mkdir -p "$XDG_CONFIG_HOME"
REG="$XDG_RUNTIME_DIR/code-jumper"

TM=(tmux -S "$TMUX_SOCK")

cleanup() {
    "${TM[@]}" kill-server 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

fail=0
ok()  { printf 'ok   - %s\n' "$1"; }
bad() { printf 'FAIL - %s\n' "$1" >&2; fail=$((fail + 1)); }

# --- project + source file -----------------------------------------------
PROJ="$WORK/proj"
mkdir -p "$PROJ"
SRC="$PROJ/app.tsx"
printf 'line1\nline2\nline3\nline4\nline5 has a target col here\n' >"$SRC"

# --- minimal init that prepends the repo so plugin/ auto-loads -----------
INIT="$WORK/init.lua"
cat >"$INIT" <<EOF
vim.opt.runtimepath:prepend("$REPO")
EOF

# --- start a tmux session, then nvim in the pane -------------------------
"${TM[@]}" new-session -d -s "$SESS" -x 200 -y 50
# Start nvim inside the pane (real pty), cwd = project so cwd-matching works.
"${TM[@]}" send-keys -t "$SESS" "cd '$PROJ' && exec nvim -u '$INIT' '$SRC'" Enter

# --- wait for registration ----------------------------------------------
TSV=""
for _ in $(seq 1 50); do
    TSV=$(ls "$REG"/pane-*.tsv 2>/dev/null | head -1 || true)
    [ -n "$TSV" ] && break
    sleep 0.1
done
[ -n "$TSV" ] || { bad "nvim never wrote a registry file"; echo "1 failures"; exit 1; }
ok "registry file written: $(basename "$TSV")"

IFS=$'\t' read -r SOCK CWD PANE TMUXS <"$TSV"
[ -S "$SOCK" ] && ok "live socket present" || bad "socket not a live socket: $SOCK"
[ "$CWD" = "$PROJ" ] && ok "cwd recorded = project" || bad "cwd mismatch: $CWD != $PROJ"

# Helper to query the running nvim.
q() { nvim --server "$SOCK" --remote-expr "$1" 2>/dev/null; }

# --- put nvim in INSERT mode to exercise the mode-reset path -------------
nvim --server "$SOCK" --remote-send 'i' 2>/dev/null
sleep 0.2

# --- run the launcher: open SRC at line 5 col 11 -------------------------
"$LAUNCHER" "$SRC" 5 11
sleep 0.4

cur_file="$(q 'expand("%:p")')"
cur_line="$(q 'line(".")')"
cur_col="$(q 'col(".")')"
cur_mode="$(q 'mode()')"

[ "$cur_file" = "$SRC" ] && ok "opened correct file" || bad "wrong file: $cur_file"
[ "$cur_line" = "5" ]    && ok "cursor on line 5"    || bad "wrong line: $cur_line"
[ "$cur_col" = "11" ]    && ok "cursor on col 11"    || bad "wrong col: $cur_col"
[ "$cur_mode" = "n" ]    && ok "back in normal mode (insert was reset)" || bad "wrong mode: $cur_mode"

# --- stale-socket skip: a dead longest-cwd match must be ignored ---------
# Plant a registry entry with a DEEPER cwd than the real nvim but a dead socket;
# the launcher must skip it (liveness probe) and fall back to the live nvim.
if command -v python3 >/dev/null; then
    DEEP="$PROJ/packages/app"
    mkdir -p "$DEEP"
    DEEPSRC="$DEEP/x.ts"
    printf 'a\nb\nc\n' >"$DEEPSRC"
    DEADSOCK="$REG/nvim-dead.sock"
    python3 -c 'import socket,sys; s=socket.socket(socket.AF_UNIX); s.bind(sys.argv[1])' "$DEADSOCK"
    printf '%s\t%s\t%s\t%s\n' "$DEADSOCK" "$DEEP" "%9" "$TMUX_SOCK" >"$REG/pane-dead.tsv"

    "$LAUNCHER" "$DEEPSRC" 3 1
    sleep 0.4
    if [ "$(q 'expand("%:p")')" = "$DEEPSRC" ] && [ "$(q 'line(".")')" = "3" ]; then
        ok "skipped dead longest-cwd entry, opened via live nvim"
    else
        bad "stale-socket fallback failed: file=$(q 'expand("%:p")') line=$(q 'line(".")')"
    fi
    rm -f "$REG/pane-dead.tsv" "$DEADSOCK"
fi

# --- focus: nvim's pane should be the active one -------------------------
# Split a second pane (becomes active), then assert the launcher refocuses nvim's.
"${TM[@]}" split-window -t "$SESS" -d # -d: don't change active away from nvim... so flip it first
"${TM[@]}" select-pane -t "$SESS" -D 2>/dev/null || "${TM[@]}" last-pane -t "$SESS" 2>/dev/null || true
active_before="$("${TM[@]}" display -p -t "$SESS" '#{pane_id}')"
"$LAUNCHER" "$SRC" 2 1
sleep 0.2
active_after="$("${TM[@]}" display -p -t "$SESS" '#{pane_id}')"
if [ "$active_after" = "$PANE" ]; then
    ok "launcher focused nvim pane ($PANE)"
else
    bad "focus failed: active=$active_after expected=$PANE (was $active_before)"
fi

# --- nvim refuses an untrusted registry dir ------------------------------
safe_good="$(nvim --headless -c "lua io.write(tostring(dofile('$REPO/lua/code-jumper/init.lua')._dir_is_safe('$REG')))" -c 'qa!' 2>/dev/null)"
[ "$safe_good" = "true" ] && ok "dir_is_safe true for our 0700 dir" || bad "dir_is_safe wrong for good dir: $safe_good"
BADREG="$WORK/badreg"; mkdir -p "$BADREG"; chmod 0777 "$BADREG"
safe_bad="$(nvim --headless -c "lua io.write(tostring(dofile('$REPO/lua/code-jumper/init.lua')._dir_is_safe('$BADREG')))" -c 'qa!' 2>/dev/null)"
[ "$safe_bad" = "false" ] && ok "dir_is_safe false for world-writable dir" || bad "dir_is_safe wrong for bad dir: $safe_bad"

# --- shutdown nvim, confirm cleanup removes the registry file ------------
nvim --server "$SOCK" --remote-send '<C-\><C-N>:qa!<CR>' 2>/dev/null
for _ in $(seq 1 30); do [ -e "$TSV" ] || break; sleep 0.1; done
[ ! -e "$TSV" ] && ok "registry cleaned on VimLeave" || bad "registry file left behind"

if [ "$fail" -eq 0 ]; then
    echo "all e2e checks passed"
else
    echo "$fail failures"
fi
[ "$fail" -eq 0 ]
