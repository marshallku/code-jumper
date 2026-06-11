//! Daemon configuration: a single `key=value` file (mode 0600) under the
//! per-user config dir. No TOML dependency — the format is three lines.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub const DEFAULT_PORT: u16 = 17654;

#[derive(Clone, Debug)]
pub struct Config {
    pub port: u16,
    pub token: String,
    /// Absolute path to the bash launcher (bin/code-jumper) — the nvim/tmux engine.
    pub launcher: String,
    /// Registry dir as resolved in the INTERACTIVE shell at install time. Pinned
    /// here so the service-managed daemon reads the same dir the nvim plugin wrote
    /// to, even when the service env differs (notably macOS TMPDIR).
    pub registry_dir: Option<String>,
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
}

pub fn config_dir() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/.config", home()));
    PathBuf::from(base).join("code-jumper")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config")
}

/// Read the `registry_dir` pinned in the (existing) config — the shared source
/// of truth, also consulted by the bash launcher and lua plugin.
fn config_registry_pin() -> Option<String> {
    let s = fs::read_to_string(config_path()).ok()?;
    for line in s.lines() {
        if let Some(v) = line.trim().strip_prefix("registry_dir=") {
            let v = v.trim_end_matches('/');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Resolve the registry dir using the SAME order as the bash launcher and lua
/// plugin: CODE_JUMPER_REGISTRY_DIR override, else the existing config pin (so a
/// reinstall is stable and doesn't move the dir out from under running nvims),
/// else XDG_RUNTIME_DIR, else TMPDIR, else /tmp (+ `/code-jumper`).
pub fn resolve_registry_dir() -> String {
    if let Ok(o) = std::env::var("CODE_JUMPER_REGISTRY_DIR") {
        if !o.is_empty() {
            return o.trim_end_matches('/').to_string();
        }
    }
    if let Some(pinned) = config_registry_pin() {
        return pinned;
    }
    let mut base = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("TMPDIR").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "/tmp".into());
    while base.ends_with('/') {
        base.pop();
    }
    format!("{base}/code-jumper")
}

/// 32 random bytes as hex, sourced from /dev/urandom (zero-dependency RNG,
/// portable on Linux + macOS).
pub fn gen_token() -> io::Result<String> {
    let mut f = fs::File::open("/dev/urandom")?;
    let mut buf = [0u8; 32];
    f.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

impl Config {
    pub fn load() -> io::Result<Config> {
        Config::load_from(&config_path())
    }

    /// Load from an explicit path. The service passes `serve --config <path>` so
    /// it reads the SAME file `install` wrote, regardless of the service env's
    /// XDG_CONFIG_HOME (which can differ from the installer's shell).
    pub fn load_from(path: &Path) -> io::Result<Config> {
        let s = fs::read_to_string(path)?;
        let mut cfg = Config {
            port: DEFAULT_PORT,
            token: String::new(),
            launcher: String::new(),
            registry_dir: None,
        };
        for line in s.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim().to_string();
                match k.trim() {
                    "port" => cfg.port = v.parse().unwrap_or(DEFAULT_PORT),
                    "token" => cfg.token = v,
                    "launcher" => cfg.launcher = v,
                    "registry_dir" => cfg.registry_dir = Some(v),
                    _ => {}
                }
            }
        }
        Ok(cfg)
    }

    pub fn save(&self) -> io::Result<()> {
        let dir = config_dir();
        fs::create_dir_all(&dir)?;
        let mut body = String::new();
        body.push_str(&format!("port={}\n", self.port));
        body.push_str(&format!("token={}\n", self.token));
        body.push_str(&format!("launcher={}\n", self.launcher));
        if let Some(rd) = &self.registry_dir {
            body.push_str(&format!("registry_dir={rd}\n"));
        }
        // The token is the daemon's auth boundary, so the file must never be
        // even transiently group/other-readable.
        write_private(&config_path(), &body)
    }
}

/// Write `body` to `path` privately: create a 0600 temp file (mode applied at
/// open, so umask can only tighten it — never world-readable), verify the mode
/// actually stuck, and atomically rename into place. Fails (does not silently
/// proceed) if a private mode cannot be enforced.
fn write_private(path: &std::path::Path, body: &str) -> io::Result<()> {
    use std::io::Write;
    // Unique temp name + create_new (O_EXCL): we must be the creator, so the
    // 0600 mode is actually applied (it is ignored when opening an existing file)
    // and a pre-planted symlink/file can't be reused. Clear our own stale temp
    // from a crashed run first.
    let tmp = path.with_file_name(format!(".config.{}.tmp", std::process::id()));
    let _ = fs::remove_file(&tmp);

    #[cfg(unix)]
    let mut f = {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?
    };
    #[cfg(not(unix))]
    let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;

    f.write_all(body.as_bytes())?;
    f.sync_all().ok();
    drop(f);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&tmp)?.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            let _ = fs::remove_file(&tmp);
            return Err(io::Error::other(format!(
                "refusing to write token config: mode {mode:o} is not private (0600)"
            )));
        }
    }

    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn write_private_is_0600_even_over_loose_existing() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("cj-cfg-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config");
        // Pre-existing world-readable file must end up private after write.
        fs::write(&p, "old").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();

        write_private(&p, "token=secret\n").unwrap();

        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "mode is {mode:o}");
        assert_eq!(fs::read_to_string(&p).unwrap(), "token=secret\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_registry_dir_order() {
        // 1) env override wins, trailing slashes stripped
        std::env::set_var("CODE_JUMPER_REGISTRY_DIR", "/tmp/cjr//");
        assert_eq!(resolve_registry_dir(), "/tmp/cjr");
        std::env::remove_var("CODE_JUMPER_REGISTRY_DIR");

        // 2) config pin used when env unset (mirrors bash/lua)
        let dir = std::env::temp_dir().join(format!("cj-resolve-{}", std::process::id()));
        fs::create_dir_all(dir.join("code-jumper")).unwrap();
        fs::write(dir.join("code-jumper/config"), "registry_dir=/pinned//x/\n").unwrap();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        assert_eq!(resolve_registry_dir(), "/pinned//x");
        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = fs::remove_dir_all(&dir);
    }
}
