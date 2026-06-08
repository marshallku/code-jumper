# code-jumper

Click an element in the browser → jump to its source in **Neovim running inside tmux**, on both **macOS and Linux**.

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
```

Two pieces:

- **`lua/code-jumper`** — a tiny Neovim plugin. Inside tmux, each nvim starts an RPC server on a known socket and writes a registry file recording `{socket, cwd, pane, tmux-socket}`. This is the single source of truth for "which nvim, and where on screen."
- **`bin/code-jumper`** — a dependency-free bash launcher. Parses the file/line/col, picks the right nvim from the registry (longest matching cwd; skips dead sockets), opens the file, and focuses its tmux pane.

The registry directory is resolved identically on both sides: `${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/code-jumper` (Linux uses `XDG_RUNTIME_DIR`, macOS uses `TMPDIR`). It's created `0700` and the launcher refuses to trust it if it isn't owned by you — so the shared-`/tmp` fallback can't be spoofed.

## Install

### 1. Neovim plugin

With **lazy.nvim**:

```lua
{ "marshallku/code-jumper" }
```

Or any plugin manager / a bare `runtimepath` entry. The plugin auto-registers on load; `setup()` is idempotent and a no-op outside tmux, so no configuration is required. Calling `require("code-jumper").setup()` yourself is fine too.

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

## Tests

```bash
bash tests/unit.sh   # arg parsing + registry selection (no tmux/nvim needed)
bash tests/e2e.sh    # real tmux pane + real nvim: register → open → focus → cleanup
```

`tests/unit.sh` needs `python3` (to mint throwaway unix sockets). `tests/e2e.sh` needs `tmux` + `nvim` and skips cleanly if either is missing.

## Limitations / notes

- One nvim per tmux pane (per-pane socket). Multiple nvims matching the same cwd → the launcher picks the most-recently-registered live one. A `tmux display-menu` chooser is a possible follow-up.
- Registry lines are tab-separated; a project path containing a literal tab or newline is unsupported (pathological).
- Zero-config `lsof` socket discovery (for nvims started without the plugin) is intentionally **not** shipped — the plugin's registry is the only discovery path.
