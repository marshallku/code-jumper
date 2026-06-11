//! Install/uninstall the daemon as a per-user login service. The only
//! OS-divergent code in the project lives here, isolated to one match.

use std::fs;
use std::io;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{self, Config, DEFAULT_PORT};
use crate::userscript::print_client_snippet;
use crate::util::xml_escape;

fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
}

fn home_join(rel: &str) -> PathBuf {
    PathBuf::from(home()).join(rel)
}

/// Default launcher path, computed from the crate's compile-time location:
/// `<repo>/daemon` → `<repo>/bin/code-jumper`. Robust regardless of where the
/// built binary ends up (unlike current_exe sibling guessing).
fn default_launcher() -> String {
    let manifest = env!("CARGO_MANIFEST_DIR"); // <repo>/daemon
    PathBuf::from(manifest)
        .parent()
        .map(|repo| repo.join("bin/code-jumper"))
        .unwrap_or_else(|| PathBuf::from("code-jumper"))
        .to_string_lossy()
        .into_owned()
}

fn pick_port() -> u16 {
    for p in DEFAULT_PORT..DEFAULT_PORT + 100 {
        if TcpListener::bind(("127.0.0.1", p)).is_ok() {
            return p;
        }
    }
    DEFAULT_PORT
}

fn current_uid() -> Option<String> {
    let out = Command::new("id").arg("-u").output().ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

pub fn install(port_override: Option<u16>, launcher_override: Option<String>) -> io::Result<()> {
    let mut cfg = Config::load().unwrap_or(Config {
        port: 0,
        token: String::new(),
        launcher: String::new(),
        registry_dir: None,
    });

    if cfg.token.is_empty() {
        cfg.token = config::gen_token()?;
    }
    cfg.port = port_override.unwrap_or(if cfg.port != 0 { cfg.port } else { pick_port() });
    cfg.launcher = launcher_override.unwrap_or(if !cfg.launcher.is_empty() {
        cfg.launcher.clone()
    } else {
        default_launcher()
    });
    // Pin the registry dir as resolved HERE (interactive shell) so the service
    // env can't drift away from where the nvim plugin writes.
    cfg.registry_dir = Some(config::resolve_registry_dir());
    cfg.save()?;

    if !Path::new(&cfg.launcher).exists() {
        eprintln!(
            "warning: launcher not found at {} — set it with `install --launcher <path>`",
            cfg.launcher
        );
    }

    let exe = std::env::current_exe()?;
    match std::env::consts::OS {
        "linux" => install_systemd(&exe, &cfg)?,
        "macos" => install_launchd(&exe, &cfg)?,
        other => eprintln!("unsupported OS '{other}': run `{} serve` yourself", exe.display()),
    }

    print_client_snippet(&cfg);
    Ok(())
}

fn install_systemd(exe: &Path, cfg: &Config) -> io::Result<()> {
    let dir = home_join(".config/systemd/user");
    fs::create_dir_all(&dir)?;
    let unit = dir.join("code-jumper.service");
    // Escape values that go inside the double-quoted ExecStart / Environment:
    // backslash and quote for the quoting itself, `%` because it's a systemd
    // specifier. Backslash first so we don't double-escape.
    let esc = |s: String| s.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%");
    let exe_s = esc(exe.display().to_string());
    let rd = esc(cfg.registry_dir.clone().unwrap_or_default());
    // Pin the exact config path so the service reads the file install just wrote,
    // independent of the service env's XDG_CONFIG_HOME.
    let cfg_s = esc(config::config_path().display().to_string());
    let content = format!(
        "[Unit]\n\
         Description=code-jumper daemon (browser -> nvim-in-tmux)\n\n\
         [Service]\n\
         ExecStart=\"{exe_s}\" serve --config \"{cfg_s}\"\n\
         Restart=on-failure\n\
         Environment=\"CODE_JUMPER_REGISTRY_DIR={rd}\"\n\n\
         [Install]\n\
         WantedBy=default.target\n",
    );
    fs::write(&unit, content)?;
    let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
    let status = Command::new("systemctl")
        .args(["--user", "enable", "--now", "code-jumper.service"])
        .status();
    match status {
        Ok(s) if s.success() => eprintln!("systemd --user service started: {}", unit.display()),
        _ => eprintln!(
            "wrote {} but `systemctl --user enable --now` failed; start it manually",
            unit.display()
        ),
    }
    Ok(())
}

fn install_launchd(exe: &Path, cfg: &Config) -> io::Result<()> {
    let dir = home_join("Library/LaunchAgents");
    fs::create_dir_all(&dir)?;
    let plist = dir.join("com.code-jumper.daemon.plist");
    let rd = cfg.registry_dir.clone().unwrap_or_default();
    // ProgramArguments uses one <string> per arg, so paths need no shell quoting,
    // only XML escaping.
    let content = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\"><dict>\n\
         \x20 <key>Label</key><string>com.code-jumper.daemon</string>\n\
         \x20 <key>ProgramArguments</key><array><string>{exe}</string><string>serve</string><string>--config</string><string>{cfg}</string></array>\n\
         \x20 <key>EnvironmentVariables</key><dict><key>CODE_JUMPER_REGISTRY_DIR</key><string>{rd}</string></dict>\n\
         \x20 <key>RunAtLoad</key><true/>\n\
         \x20 <key>KeepAlive</key><true/>\n\
         </dict></plist>\n",
        exe = xml_escape(&exe.display().to_string()),
        cfg = xml_escape(&config::config_path().display().to_string()),
        rd = xml_escape(&rd),
    );
    fs::write(&plist, content)?;

    // Prefer modern bootstrap; fall back to legacy load -w.
    let bootstrapped = current_uid().and_then(|uid| {
        Command::new("launchctl")
            .args(["bootstrap", &format!("gui/{uid}"), &plist.to_string_lossy()])
            .status()
            .ok()
            .filter(|s| s.success())
    });
    if bootstrapped.is_none() {
        let _ = Command::new("launchctl").args(["load", "-w", &plist.to_string_lossy()]).status();
    }
    eprintln!("launchd agent installed: {}", plist.display());
    Ok(())
}

pub fn uninstall() -> io::Result<()> {
    match std::env::consts::OS {
        "linux" => {
            let _ = Command::new("systemctl")
                .args(["--user", "disable", "--now", "code-jumper.service"])
                .status();
            let unit = home_join(".config/systemd/user/code-jumper.service");
            if unit.exists() {
                fs::remove_file(&unit)?;
            }
            let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
            eprintln!("removed systemd --user service");
        }
        "macos" => {
            let plist = home_join("Library/LaunchAgents/com.code-jumper.daemon.plist");
            if let Some(uid) = current_uid() {
                let _ = Command::new("launchctl")
                    .args(["bootout", &format!("gui/{uid}/com.code-jumper.daemon")])
                    .status();
            }
            let _ = Command::new("launchctl").args(["unload", "-w", &plist.to_string_lossy()]).status();
            if plist.exists() {
                fs::remove_file(&plist)?;
            }
            eprintln!("removed launchd agent");
        }
        other => eprintln!("unsupported OS '{other}'"),
    }
    Ok(())
}
