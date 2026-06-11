//! code-jumper-daemon — a small local HTTP front-end for the code-jumper bash
//! launcher. Lets a browser (LocatorJS) jump to source in nvim-in-tmux via a
//! fixed address, with no per-project dev-server config.
//!
//!   code-jumper-daemon serve | install | uninstall | userscript | status

mod config;
mod install;
mod serve;
mod userscript;
mod util;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use config::Config;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("");

    match cmd {
        "serve" => {
            let cfg = match flag_value(&args[2..], "--config") {
                Some(p) => Config::load_from(std::path::Path::new(&p)).unwrap_or_else(|e| {
                    eprintln!("code-jumper-daemon: cannot read --config {p}: {e}");
                    std::process::exit(1);
                }),
                None => load_or_die(),
            };
            serve::serve(cfg);
        }
        "install" => {
            let (port, launcher) = parse_install_flags(&args[2..]);
            if let Err(e) = install::install(port, launcher) {
                eprintln!("install failed: {e}");
                std::process::exit(1);
            }
        }
        "uninstall" => {
            if let Err(e) = install::uninstall() {
                eprintln!("uninstall failed: {e}");
                std::process::exit(1);
            }
        }
        "userscript" => {
            let cfg = load_or_die();
            print!("{}", userscript::userscript(&cfg));
            eprintln!("\nLocatorJS template:\n{}", userscript::locatorjs_template(&cfg));
        }
        "status" => status(),
        "" | "-h" | "--help" | "help" => {
            eprintln!(
                "code-jumper-daemon {}\n\nusage: code-jumper-daemon <command>\n\n\
                 \x20 serve        run the HTTP daemon (used by the service)\n\
                 \x20 install      install+start the login service, print client wiring\n\
                 \x20 uninstall    stop+remove the login service\n\
                 \x20 userscript   re-print the userscript + LocatorJS template\n\
                 \x20 status       check the running daemon\n\n\
                 install flags: --port <n>  --launcher <path>",
                env!("CARGO_PKG_VERSION")
            );
            if cmd.is_empty() {
                std::process::exit(2);
            }
        }
        other => {
            eprintln!("unknown command '{other}' (try --help)");
            std::process::exit(2);
        }
    }
}

fn load_or_die() -> Config {
    Config::load().unwrap_or_else(|e| {
        eprintln!("code-jumper-daemon: no config ({e}); run `install` first");
        std::process::exit(1);
    })
}

/// Value following `flag` in `args`, if present (`--flag value`).
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}

fn parse_install_flags(args: &[String]) -> (Option<u16>, Option<String>) {
    let mut port = None;
    let mut launcher = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                if let Some(v) = args.get(i + 1).and_then(|s| s.parse().ok()) {
                    port = Some(v);
                }
                i += 2;
            }
            "--launcher" => {
                launcher = args.get(i + 1).cloned();
                i += 2;
            }
            _ => i += 1,
        }
    }
    (port, launcher)
}

fn status() {
    let cfg = load_or_die();
    let addr = format!("127.0.0.1:{}", cfg.port);
    match TcpStream::connect(&addr) {
        Ok(mut s) => {
            let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
            let req = format!("GET /health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
            if s.write_all(req.as_bytes()).is_ok() {
                let mut buf = String::new();
                let _ = s.read_to_string(&mut buf);
                let ok = buf.starts_with("HTTP/1.1 200");
                let body = buf.rsplit("\r\n\r\n").next().unwrap_or("");
                println!("{} on {addr} — {}", if ok { "UP" } else { "responding" }, body.trim());
            }
        }
        Err(e) => {
            println!("DOWN on {addr} — {e}");
            std::process::exit(1);
        }
    }
}
