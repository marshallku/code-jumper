--- code-jumper — nvim side.
---
--- Registers this Neovim instance so the `code-jumper` launcher can find it and
--- focus its tmux pane. Active only inside tmux. Writes one registry file per
--- pane containing a single tab-separated line: `socket<TAB>cwd<TAB>pane<TAB>tmux_socket`.
---
--- See docs/04-design.md for the contract; the launcher (bin/code-jumper) must
--- resolve the registry directory with the same rule used here.

local M = {}

local SEP = "\t"

--- Read `registry_dir` pinned in the daemon config, if present. This is the
--- shared source of truth that keeps the plugin and a service-managed daemon on
--- the same dir even when their envs differ (notably macOS TMPDIR).
local function config_registry_dir()
    local base = vim.env.XDG_CONFIG_HOME
    if not base or base == "" then
        base = (vim.env.HOME or "") .. "/.config"
    end
    local fd = io.open(base .. "/code-jumper/config", "r")
    if not fd then
        return nil
    end
    local result
    for line in fd:lines() do
        local v = line:match("^registry_dir=(.*)$")
        if v and v ~= "" then
            result = v:gsub("/+$", "")
            break
        end
    end
    fd:close()
    return result
end

--- Resolve the registry directory. MUST stay in sync with the launcher's
--- `cj_registry_dir`. Order: CODE_JUMPER_REGISTRY_DIR override, else the
--- daemon-config pin, else XDG_RUNTIME_DIR, else TMPDIR, else /tmp.
local function registry_dir()
    local override = vim.env.CODE_JUMPER_REGISTRY_DIR
    if override and override ~= "" then
        return (override:gsub("/+$", ""))
    end
    local pinned = config_registry_dir()
    if pinned then
        return pinned
    end
    local base = vim.env.XDG_RUNTIME_DIR
    if not base or base == "" then
        base = vim.env.TMPDIR
    end
    if not base or base == "" then
        base = "/tmp"
    end
    base = base:gsub("/+$", "")
    return base .. "/code-jumper"
end

--- Sanitize a tmux pane id (`%12`) into a filesystem-safe token (`12`).
local function sanitize(pane)
    return (pane:gsub("[^%w]", ""))
end

--- tmux socket path from $TMUX (`/path/to/sock,<pid>,<session>`), first field.
local function tmux_socket()
    local tmux = vim.env.TMUX
    if not tmux or tmux == "" then
        return ""
    end
    return tmux:match("^[^,]*") or ""
end

--- tmux server pid from $TMUX (second field). Pane ids (`%1`) are only unique
--- within one tmux server, but every server shares the same registry dir — so
--- the per-pane filenames must also carry the server id to avoid collisions
--- (and to keep one server's stale-socket cleanup from touching another's).
local function tmux_server_id()
    local tmux = vim.env.TMUX
    if not tmux or tmux == "" then
        return ""
    end
    local pid = tmux:match("^[^,]*,([^,]*)")
    return (pid or ""):gsub("[^%w]", "")
end

local function ensure_dir(dir)
    -- 0700 so the /tmp fallback can't be read/spoofed by other local users.
    vim.fn.mkdir(dir, "p", "0700")
    -- Re-assert mode in case the dir pre-existed with looser bits.
    pcall(vim.loop.fs_chmod, dir, tonumber("700", 8))
end

--- Mirror of the launcher's cj_dir_is_safe: refuse to expose a socket / write a
--- registry inside a dir another local user could control (the /tmp fallback).
--- Must hold BEFORE serverstart so an attacker-planted dir never gets a socket.
local function dir_is_safe(dir)
    local st = vim.loop.fs_lstat(dir)
    if not st or st.type ~= "directory" then
        return false
    end
    local uid = vim.loop.getuid and vim.loop.getuid() or nil
    if uid ~= nil and st.uid ~= uid then
        return false -- not owned by us
    end
    if bit.band(st.mode, tonumber("22", 8)) ~= 0 then
        return false -- group/other writable
    end
    return true
end

local state = {
    socket = nil,
    file = nil,
    pane = nil,
}

local function start_server(sock)
    -- A stale socket from a crashed nvim in the same pane makes serverstart fail.
    -- Try once; on failure, remove a dead socket and retry.
    local ok = pcall(vim.fn.serverstart, sock)
    if ok then
        return true
    end
    pcall(os.remove, sock)
    ok = pcall(vim.fn.serverstart, sock)
    return ok
end

local function write_registry()
    if not state.socket or not state.file then
        return
    end
    local line = table.concat({
        state.socket,
        vim.fn.getcwd(),
        state.pane,
        tmux_socket(),
    }, SEP) .. "\n"
    local fd = io.open(state.file, "w")
    if not fd then
        return
    end
    fd:write(line)
    fd:close()
    pcall(vim.loop.fs_chmod, state.file, tonumber("600", 8))
end

local function cleanup()
    if state.file then
        pcall(os.remove, state.file)
    end
    if state.socket then
        pcall(vim.fn.serverstop, state.socket)
    end
end

--- Register this instance: start the server and write the registry file.
local function register()
    local pane = vim.env.TMUX_PANE
    if not pane or pane == "" then
        return
    end
    local dir = registry_dir()
    ensure_dir(dir)
    if not dir_is_safe(dir) then
        -- Untrusted registry dir (e.g. pre-created by another user in /tmp):
        -- don't expose a socket or write registry data into it.
        return
    end

    -- Key on <server-pid>-<pane> so two tmux servers can't collide on `%1`.
    local server = tmux_server_id()
    local key = (server ~= "" and (server .. "-") or "") .. sanitize(pane)
    state.pane = pane
    state.socket = dir .. "/nvim-" .. key .. ".sock"
    state.file = dir .. "/pane-" .. key .. ".tsv"

    if not start_server(state.socket) then
        state.socket = nil
        return
    end
    write_registry()
end

--- @param opts table|nil reserved for future options
function M.setup(opts)
    M.opts = opts or {}

    -- Only meaningful inside tmux.
    if not vim.env.TMUX or vim.env.TMUX == "" then
        return
    end

    -- Idempotent: clearing the augroup drops any prior registration's autocmds.
    local group = vim.api.nvim_create_augroup("CodeJumper", { clear = true })

    -- If setup() runs after VimEnter already fired (lazy-loaded), register now —
    -- the VimEnter autocmd below would never fire in that case.
    if vim.v.vim_did_enter == 1 then
        register()
    else
        vim.api.nvim_create_autocmd("VimEnter", {
            group = group,
            callback = register,
        })
    end

    vim.api.nvim_create_autocmd("DirChanged", {
        group = group,
        callback = write_registry,
    })

    vim.api.nvim_create_autocmd("VimLeavePre", {
        group = group,
        callback = cleanup,
    })
end

-- Exposed for tests.
M._registry_dir = registry_dir
M._sanitize = sanitize
M._tmux_socket = tmux_socket
M._tmux_server_id = tmux_server_id
M._dir_is_safe = dir_is_safe

return M
