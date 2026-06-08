#!/usr/bin/env bash
# Unit tests for bin/code-jumper pure functions (arg parsing, path matching,
# registry selection). No tmux/nvim required. Sourced, so cj_main never runs.
set -u

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../bin/code-jumper
source "$HERE/../bin/code-jumper"

# The real cj_sock_alive probes a listening nvim; these unit tests exercise the
# *selection* logic with non-listening fixture sockets, so stub liveness to mean
# "socket node exists" (real RPC liveness is covered by tests/e2e.sh).
cj_sock_alive() { [ -S "$1" ]; }

pass=0
fail=0

check() {
    local desc="$1" want="$2" got="$3"
    if [ "$want" = "$got" ]; then
        pass=$((pass + 1))
    else
        fail=$((fail + 1))
        printf 'FAIL: %s\n  want: [%s]\n  got:  [%s]\n' "$desc" "$want" "$got" >&2
    fi
}

# --- cj_parse_args: every caller form ------------------------------------
parse_case() {
    cj_parse_args "$@"
    printf '%s|%s|%s' "$CJ_FILE" "$CJ_LINE" "$CJ_COL"
}

check "3-argv file line col"   "/a/b.tsx|12|5"      "$(parse_case /a/b.tsx 12 5)"
check "2-argv file line"       "/a/b.tsx|12|1"      "$(parse_case /a/b.tsx 12)"
check "vim +line file"         "/a/b.tsx|30|1"      "$(parse_case +30 /a/b.tsx)"
check "token file:line:col"    "/a/b.tsx|12|5"      "$(parse_case /a/b.tsx:12:5)"
check "token file:line"        "/a/b.tsx|12|1"      "$(parse_case /a/b.tsx:12)"
check "bare file"              "/a/b.tsx|1|1"       "$(parse_case /a/b.tsx)"
# Colon inside path must survive (only numeric suffixes are peeled).
check "path with colon + line" "/a/wei:rd.tsx|9|1"  "$(parse_case /a/wei:rd.tsx:9)"
check "non-numeric line falls back" "/a/b.tsx|1|1"  "$(parse_case /a/b.tsx foo)"

# --- cj_cwd_contains: path-boundary matching -----------------------------
cwd_case() { cj_cwd_contains "$1" "$2" && echo yes || echo no; }
check "exact cwd match"        "yes" "$(cwd_case /home/u/proj /home/u/proj)"
check "child file match"       "yes" "$(cwd_case /home/u/proj /home/u/proj/src/a.ts)"
check "trailing slash cwd"     "yes" "$(cwd_case /home/u/proj/ /home/u/proj/src/a.ts)"
check "sibling prefix no-match" "no" "$(cwd_case /home/u/proj /home/u/project2/a.ts)"

# --- cj_select: longest cwd wins, dead sockets skipped -------------------
TMPDIR_T="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_T"' EXIT
REG="$TMPDIR_T/reg"
mkdir -p "$REG"

# Create real unix sockets so [ -S ] passes, via a tiny python listener... but
# simpler: use `nc`-less approach — a unix socket can be made with python.
mksock() { python3 -c 'import socket,sys; s=socket.socket(socket.AF_UNIX); s.bind(sys.argv[1])' "$1"; }

SOCK_OUTER="$REG/nvim-1.sock"; mksock "$SOCK_OUTER"
SOCK_INNER="$REG/nvim-2.sock"; mksock "$SOCK_INNER"
printf '%s\t%s\t%s\t%s\n' "$SOCK_OUTER" "/home/u/proj"        "%1" "/tmp/tmux-1/default" >"$REG/pane-1.tsv"
printf '%s\t%s\t%s\t%s\n' "$SOCK_INNER" "/home/u/proj/packages/app" "%2" "/tmp/tmux-1/default" >"$REG/pane-2.tsv"

sel="$(cj_select /home/u/proj/packages/app/src/x.ts "$REG")"
check "longest cwd wins" "$SOCK_INNER	%2	/tmp/tmux-1/default" "$sel"

sel2="$(cj_select /home/u/proj/lib/y.ts "$REG")"
check "shallower file -> outer" "$SOCK_OUTER	%1	/tmp/tmux-1/default" "$sel2"

# Dead socket (file present in tsv but socket removed) is skipped.
rm -f "$SOCK_INNER"
sel3="$(cj_select /home/u/proj/packages/app/src/x.ts "$REG")"
check "dead inner socket falls back to outer" "$SOCK_OUTER	%1	/tmp/tmux-1/default" "$sel3"

# No live nvim at all -> empty.
rm -f "$SOCK_OUTER"
sel4="$(cj_select /home/u/proj/src/x.ts "$REG")"
check "no live socket -> empty" "" "$sel4"

# --- summary -------------------------------------------------------------
printf '\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
