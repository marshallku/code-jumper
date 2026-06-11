#!/usr/bin/env bash
# Full-stack e2e for the daemon: HTTP -> bash launcher -> real nvim in real tmux.
# Proves the whole chain plus the two daemon-specific guarantees:
#   - strict mode: a file under NO registered nvim cwd (e.g. /etc/passwd) opens nothing
#   - bad token is rejected (403) and never reaches nvim
set -u

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
LAUNCHER="$REPO/bin/code-jumper"
DAEMON="$REPO/daemon/target/debug/code-jumper-daemon"

command -v tmux >/dev/null || { echo "SKIP: tmux not installed"; exit 0; }
command -v nvim >/dev/null || { echo "SKIP: nvim not installed"; exit 0; }
command -v curl >/dev/null || { echo "SKIP: curl not installed"; exit 0; }
command -v python3 >/dev/null || { echo "SKIP: python3 not installed"; exit 0; }

# Build the daemon (debug) if missing.
if [ ! -x "$DAEMON" ]; then
    ( cd "$REPO/daemon" && cargo build ) >/dev/null 2>&1 || { echo "SKIP: cargo build failed"; exit 0; }
fi

WORK="$(mktemp -d)"
TMUX_SOCK="$WORK/tmux.sock"
SESS="cjd"
export XDG_RUNTIME_DIR="$WORK/xdg"
mkdir -p "$XDG_RUNTIME_DIR"
# Isolate XDG_CONFIG_HOME so the plugin's config-pin tier can't read a real
# ~/.config/code-jumper/config; here the plugin uses XDG_RUNTIME_DIR (= the
# daemon's pinned registry_dir), which is what we want to test.
export XDG_CONFIG_HOME="$WORK/cfg"
mkdir -p "$XDG_CONFIG_HOME"
REG="$XDG_RUNTIME_DIR/code-jumper"
TOKEN="testtoken0000000000000000000000000000000000000000000000000000beef"
PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
TM=(tmux -S "$TMUX_SOCK")

DAEMON_PID=""
cleanup() {
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null
    "${TM[@]}" kill-server 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

fail=0
ok()  { printf 'ok   - %s\n' "$1"; }
bad() { printf 'FAIL - %s\n' "$1" >&2; fail=$((fail + 1)); }

# --- project + source -----------------------------------------------------
PROJ="$WORK/proj"; mkdir -p "$PROJ"
SRC="$PROJ/app.tsx"
printf 'l1\nl2\nl3\nl4\nl5 target col here\n' >"$SRC"
INIT="$WORK/init.lua"; printf 'vim.opt.runtimepath:prepend("%s")\n' "$REPO" >"$INIT"

# --- nvim in a real tmux pane --------------------------------------------
"${TM[@]}" new-session -d -s "$SESS" -x 200 -y 50
"${TM[@]}" send-keys -t "$SESS" "cd '$PROJ' && exec nvim -u '$INIT' '$SRC'" Enter

TSV=""
for _ in $(seq 1 50); do TSV=$(ls "$REG"/pane-*.tsv 2>/dev/null | head -1 || true); [ -n "$TSV" ] && break; sleep 0.1; done
[ -n "$TSV" ] || { bad "nvim never registered"; echo "1 failures"; exit 1; }
IFS=$'\t' read -r SOCK CWD PANE TMUXS <"$TSV"
ok "nvim registered ($CWD)"
q() { nvim --server "$SOCK" --remote-expr "$1" 2>/dev/null; }

# --- daemon config + start -----------------------------------------------
CFG="$WORK/daemon-config"
printf 'port=%s\ntoken=%s\nlauncher=%s\nregistry_dir=%s\n' "$PORT" "$TOKEN" "$LAUNCHER" "$REG" >"$CFG"
# Use the explicit --config path the installed service uses (service-representative).
"$DAEMON" serve --config "$CFG" >"$WORK/daemon.log" 2>&1 &
DAEMON_PID=$!

for _ in $(seq 1 50); do curl -s "http://127.0.0.1:$PORT/health" >/dev/null && break; sleep 0.1; done
if curl -s "http://127.0.0.1:$PORT/health" | grep -q code-jumper-daemon; then
    ok "daemon /health up on $PORT"
else
    bad "daemon did not come up (see $WORK/daemon.log)"; echo "$fail failures"; exit 1
fi

# --- happy path: open SRC at 5:6 via HTTP ---------------------------------
code=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/open?token=$TOKEN&file=$SRC:5:6")
sleep 0.5
[ "$code" = "204" ] && ok "/open returned 204" || bad "/open status: $code"
[ "$(q 'expand("%:p")')" = "$SRC" ] && ok "nvim opened SRC" || bad "wrong file: $(q 'expand("%:p")')"
[ "$(q 'line(".")')" = "5" ] && ok "cursor on line 5" || bad "wrong line: $(q 'line(".")')"

# --- security: bad token rejected, never reaches nvim ---------------------
code=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/open?token=WRONG&file=$SRC:2:1")
sleep 0.3
[ "$code" = "403" ] && ok "bad token -> 403" || bad "bad token status: $code"
[ "$(q 'line(".")')" = "5" ] && ok "bad token did not move cursor" || bad "cursor moved on bad token"

# --- security: strict mode refuses a file under no registered cwd ---------
code=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/open?token=$TOKEN&file=/etc/passwd:1:1")
sleep 0.5
# HTTP is 204 (fire-and-forget) but nvim must NOT have opened /etc/passwd.
cur="$(q 'expand("%:p")')"
[ "$cur" = "$SRC" ] && ok "strict: /etc/passwd NOT opened (still on SRC)" || bad "strict breach: opened $cur"

if [ "$fail" -eq 0 ]; then echo "all daemon e2e checks passed"; else echo "$fail failures"; fi
[ "$fail" -eq 0 ]
