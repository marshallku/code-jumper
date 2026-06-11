//! Generates the client-side wiring printed to the user: the LocatorJS custom
//! link template (with token+port filled in) and an optional userscript that
//! turns LocatorJS's `window.open` into a no-flash `fetch`.

use crate::config::Config;

/// The LocatorJS custom link template. Paste into the extension's custom editor
/// setting. The token lives in extension settings, not page DOM.
pub fn locatorjs_template(cfg: &Config) -> String {
    format!(
        "http://127.0.0.1:{}/open?token={}&file=${{projectPath}}${{filePath}}:${{line}}:${{column}}",
        cfg.port, cfg.token
    )
}

/// A generic, token-agnostic userscript: it just forwards whatever daemon URL
/// LocatorJS hands to window.open as a fire-and-forget fetch (no tab, no nav).
/// Install once per browser; works for every project.
pub fn userscript(cfg: &Config) -> String {
    format!(
        "// ==UserScript==\n\
         // @name        code-jumper\n\
         // @namespace   code-jumper\n\
         // @match       http://localhost:*/*\n\
         // @match       http://127.0.0.1:*/*\n\
         // @run-at      document-start\n\
         // @grant       none\n\
         // ==/UserScript==\n\
         (function () {{\n\
         \x20 const DAEMON = \"127.0.0.1:{port}/open\";\n\
         \x20 const _open = window.open.bind(window);\n\
         \x20 window.open = function (url, ...rest) {{\n\
         \x20   if (typeof url === \"string\" && url.includes(DAEMON)) {{\n\
         \x20     fetch(url, {{ mode: \"no-cors\" }});\n\
         \x20     return null;\n\
         \x20   }}\n\
         \x20   return _open(url, ...rest);\n\
         \x20 }};\n\
         }})();\n",
        port = cfg.port
    )
}

/// Human-facing wiring instructions, printed after install / on `userscript`.
pub fn print_client_snippet(cfg: &Config) {
    println!("\n── code-jumper client wiring ─────────────────────────────");
    println!("\n1) LocatorJS → Settings → custom editor template:\n");
    println!("   {}", locatorjs_template(cfg));
    println!("\n2) (optional, removes the tab flash) install this userscript once:\n");
    println!("{}", userscript(cfg));
    println!("Without the userscript it still works — the daemon returns a");
    println!("self-closing tab. Re-print anytime: `code-jumper-daemon userscript`.");
    println!("──────────────────────────────────────────────────────────");
}
