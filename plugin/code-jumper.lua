-- Auto-load shim: a plain `runtimepath`/lazy import registers this nvim with
-- code-jumper using defaults. setup() is idempotent and a no-op outside tmux,
-- so calling it again from user config with options is safe.
if vim.g.loaded_code_jumper then
    return
end
vim.g.loaded_code_jumper = true

require("code-jumper").setup()
