# code-jumper

Click an element in the browser → jump to its source in **Neovim running inside tmux or [comux](https://github.com/marshallku/copad)**, on both **macOS and Linux**.

A replacement for the "click-to-source" experience of [LocatorJS](https://www.locatorjs.com/) and friends, but targeting an editor that lives inside a tmux pane instead of a GUI editor the OS can launch via a URL scheme. The full rationale and design live in [`docs/`](./docs); this README is the operational guide.

## How it works

```
browser click → dev-server inspector → launch-editor → $LAUNCH_EDITOR (this tool)
                                                              │
   bin/code-jumper <file> <line> <col> ───────────────────────┘
     ├─ read registry → pick the nvim whose cwd contains the file
     ├─ nvim --server <sock> --remote <file>   (open, argv-safe)
     ├─ nvim --server <sock> --remote-send '…cursor(line,col)…'
     └─ tmux select-window/select-pane/switch-client   (focus, OS-agnostic)
        or COPAD_MUX_SOCK=<sock> comux jump <token>      (comux panes)
```

Two pieces:

- **`lua/code-jumper`** — a tiny Neovim plugin. Inside tmux or comux, each nvim starts an RPC server on a known socket and writes a registry file recording `{socket, cwd, pane, mux-socket, mux}` (`pane` is the tmux pane id or the comux `$COPAD_MUX_PANE` token). This is the single source of truth for "which nvim, and where on screen."
- **`bin/code-jumper`** — a dependency-free bash launcher. Parses the file/line/col, picks the right nvim from the registry (longest matching cwd; skips dead sockets), opens the file, and focuses its tmux or comux pane.

The registry directory is resolved identically on both sides: `${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/code-jumper` (Linux uses `XDG_RUNTIME_DIR`, macOS uses `TMPDIR`). It's created `0700` and the launcher refuses to trust it if it isn't owned by you — so the shared-`/tmp` fallback can't be spoofed.

## Install

### 1. Neovim plugin

With **lazy.nvim**:

```lua
{ "marshallku/code-jumper" }
```

Or any plugin manager / a bare `runtimepath` entry. The plugin auto-registers on load; `setup()` is idempotent and a no-op outside tmux/comux, so no configuration is required. Calling `require("code-jumper").setup()` yourself is fine too.

> Requires Neovim with built-in `--server`/`--remote`/`--remote-send` (0.x+). `nvr` is **not** needed.

### 2. Shell env

Point `LAUNCH_EDITOR` at the launcher (leave `EDITOR` alone so normal terminal editing is untouched):

```bash
# ~/.zshenv / ~/.bashrc / ~/.profile
export LAUNCH_EDITOR="$HOME/dev/code-jumper/bin/code-jumper"
```

Setting `LAUNCH_EDITOR` (rather than `EDITOR`) is what makes [`launch-editor`](https://github.com/yyx990803/launch-editor)-based inspectors pass `file line col` as separate argv — see [`docs/05-launch-editor-internals.md`](./docs/05-launch-editor-internals.md).

### 3. Dev-server inspector

The launcher itself is bundler-agnostic; you only need an inspector that does a server-side launch:

| Stack | Wiring |
|---|---|
| Vite (any) | Built-in error overlay already POSTs `/__open-in-editor` via `launch-editor` — `LAUNCH_EDITOR` is enough. |
| Vite + React | `vite-plugin-react-click-to-component` or `react-dev-inspector` |
| Vite + Vue | `vite-plugin-vue-inspector` |
| Vite + Svelte | the Svelte inspector |
| webpack / Next.js | `react-dev-inspector` middleware |

## Usage

Once wired, click (or hotkey-click) an element in your dev server; the source opens in the already-running nvim and its pane gets focus. You normally never call the launcher by hand, but it accepts every common form:

```bash
code-jumper /path/app.tsx 42 7      # file line col   (launch-editor primary)
code-jumper /path/app.tsx 42        # file line
code-jumper +42 /path/app.tsx       # vim-style
code-jumper /path/app.tsx:42:7      # VS Code token
code-jumper /path/app.tsx           # bare (line/col → 1)
```

### Optional: raise the terminal's OS window

Everything works when the terminal is already the focused OS window (the normal alt-tab case). When it isn't, the launcher makes a best-effort raise via `osascript` (macOS) or `hyprctl`/`wmctrl` (Linux). This is the only OS-divergent code and is never fatal.

## Browser integration without per-project config — the daemon

The `LAUNCH_EDITOR` route above needs each dev server to expose an open-in-editor endpoint (e.g. a Vite middleware), i.e. per-project wiring. To avoid that entirely, `daemon/` is a small static Rust binary (`code-jumper-daemon`) that runs as a **per-user login service** and exposes one fixed local address. A browser inspector (e.g. [LocatorJS](https://www.locatorjs.com/)) points at that address for *every* project — no dev-server changes anywhere.

```
LocatorJS (window.open / fetch) → http://127.0.0.1:<port>/open?token=…&file=…:line:col
                                     │  code-jumper-daemon (127.0.0.1 only, token-gated)
                                     └─ spawns  bin/code-jumper --strict <file>  → nvim + tmux
```

It reuses the bash launcher as the engine — one engine, two front-ends (`LAUNCH_EDITOR` argv and HTTP).

### Build & install

```bash
cargo build --release --manifest-path daemon/Cargo.toml
./daemon/target/release/code-jumper-daemon install   # writes config + login service, prints wiring
```

`install` branches on the OS (`systemd --user` on Linux, a `launchd` LaunchAgent on macOS), generates a random token + port, and prints (1) the LocatorJS custom-template string and (2) an optional userscript. Re-print anytime with `code-jumper-daemon userscript`; check it with `code-jumper-daemon status`; remove with `uninstall`.

### Security model

A daemon that opens local files from HTTP is a new attack surface, so:

- **127.0.0.1 bind only** — never reachable off-host.
- **Random token** on `/open`, constant-time compared — blocks drive-by/CSRF from random sites (they can fire no-cors requests but don't know the token).
- **`--strict` launcher mode** — the daemon never uses the most-recent-nvim fallback, so a request for a file under *no* registered nvim cwd (e.g. `/etc/passwd`) opens nothing. The registry is the allowlist.

### Registry-dir agreement

A service's environment can differ from the tmux/shell that started nvim (notably macOS `TMPDIR`), which would make the daemon and the nvim plugin disagree on the registry path. To keep them on one path, all three components (daemon, launcher, plugin) resolve the registry dir in the same order: `CODE_JUMPER_REGISTRY_DIR` env override → the `registry_dir` value pinned in the daemon config (the single source of truth `install` writes) → `XDG_RUNTIME_DIR`/`TMPDIR`/`/tmp`. So once installed, the plugin reads the same pinned dir the daemon uses, regardless of env drift; without the daemon, the plugin still works via the default rule.

## Tests

```bash
bash tests/unit.sh                       # bash: arg parsing + registry selection + strict mode
bash tests/e2e.sh                        # bash: real tmux + nvim, register → open → focus → cleanup
cargo test --manifest-path daemon/Cargo.toml   # rust: util units + HTTP/auth integration (fake launcher)
bash tests/e2e-daemon.sh                 # full chain: HTTP → launcher → nvim, incl. strict + bad-token
```

`tests/unit.sh` needs `python3` (to mint throwaway unix sockets). The e2e scripts need `tmux` + `nvim` (and `e2e-daemon.sh` also `curl` + `python3`) and skip cleanly if a dependency is missing.

## Limitations / notes

- When both `$TMUX` and `$COPAD_MUX` are set, the nvim registers as a tmux pane. Focusing a comux pane needs `comux` on the launcher's `PATH`.
- One nvim per tmux/comux pane (per-pane socket). Multiple nvims matching the same cwd → the launcher picks the most-recently-registered live one. A `tmux display-menu` chooser is a possible follow-up.
- Registry lines are tab-separated; a project path containing a literal tab or newline is unsupported (pathological).
- Zero-config `lsof` socket discovery (for nvims started without the plugin) is intentionally **not** shipped — the plugin's registry is the only discovery path.
- The LocatorJS custom template substitutes the file path into a query string without URL-encoding it (LocatorJS limitation), so a source path containing `&`, `#`, or a space would be truncated/misparsed before the daemon sees it. The daemon percent-decodes whatever it receives, so a client that *does* encode the path works; raw bare paths with those characters do not. Such paths are rare in practice.
- The macOS `launchd` install path is implemented but untested on this Linux dev box; the `systemd --user` path is the one covered by `tests/e2e-daemon.sh`.
