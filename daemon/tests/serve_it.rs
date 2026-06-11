//! Integration test: run the real daemon binary against a FAKE launcher that
//! records its argv. Asserts the HTTP contract (auth, validation, decoding) and
//! that the launcher is invoked with exactly `--strict <decoded-file>` — and NOT
//! invoked on a bad token. No nvim/tmux needed (the bash launcher is e2e-tested
//! separately).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_code-jumper-daemon");
const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

static SEQ: AtomicU32 = AtomicU32::new(0);

struct Fixture {
    tmp: PathBuf,
    record: PathBuf,
    port: u16,
    child: Child,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

fn free_port() -> u16 {
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    l.local_addr().unwrap().port()
}

fn setup() -> Fixture {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let tmp = std::env::temp_dir().join(format!("cj-daemon-test-{}-{}", std::process::id(), n));
    let cfgdir = tmp.join("code-jumper");
    std::fs::create_dir_all(&cfgdir).unwrap();

    let record = tmp.join("record.txt");
    // Fake launcher: append its full argv to the record file.
    let launcher = tmp.join("fake-launcher");
    std::fs::write(
        &launcher,
        format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n", record.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let port = free_port();
    let cfg_file = cfgdir.join("config");
    std::fs::write(
        &cfg_file,
        format!("port={port}\ntoken={TOKEN}\nlauncher={}\n", launcher.display()),
    )
    .unwrap();

    // Use the explicit --config path the service uses (no XDG_CONFIG_HOME), so
    // this also exercises Config::load_from.
    let child = Command::new(BIN)
        .arg("serve")
        .arg("--config")
        .arg(&cfg_file)
        .spawn()
        .expect("spawn daemon");

    // Wait until the port accepts connections.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    Fixture { tmp, record, port, child }
}

fn http_get(port: u16, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).unwrap();
    let mut buf = String::new();
    let _ = s.read_to_string(&mut buf);
    let status: u16 = buf
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = buf.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

fn read_record(f: &Fixture) -> String {
    std::fs::read_to_string(&f.record).unwrap_or_default()
}

/// Wait briefly for the fire-and-forget launcher to write the record.
fn await_record(f: &Fixture) -> String {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        let r = read_record(f);
        if !r.trim().is_empty() {
            return r;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    read_record(f)
}

#[test]
fn health_is_unauthenticated() {
    let f = setup();
    let (status, body) = http_get(f.port, "/health");
    assert_eq!(status, 200, "health status");
    assert!(body.contains("code-jumper-daemon"), "health body: {body}");
}

#[test]
fn valid_token_invokes_launcher_strict_with_decoded_file() {
    let f = setup();
    // file=/x/y.tsx:3:4 percent-encoded
    let path = format!("/open?token={TOKEN}&file=%2Fx%2Fy.tsx%3A3%3A4");
    let (status, _) = http_get(f.port, &path);
    assert_eq!(status, 204, "open status");
    let rec = await_record(&f);
    assert_eq!(rec.trim(), "--strict /x/y.tsx:3:4", "launcher argv: {rec:?}");
}

#[test]
fn bad_token_is_forbidden_and_does_not_invoke_launcher() {
    let f = setup();
    let path = "/open?token=WRONG&file=%2Fx%2Fy.tsx%3A3%3A4";
    let (status, _) = http_get(f.port, path);
    assert_eq!(status, 403, "bad token status");
    // give any (erroneous) spawn a moment, then assert nothing was recorded
    std::thread::sleep(Duration::from_millis(300));
    assert!(read_record(&f).trim().is_empty(), "launcher must not run on bad token");
}

#[test]
fn missing_file_is_bad_request() {
    let f = setup();
    let path = format!("/open?token={TOKEN}");
    let (status, _) = http_get(f.port, &path);
    assert_eq!(status, 400, "missing file status");
    std::thread::sleep(Duration::from_millis(200));
    assert!(read_record(&f).trim().is_empty(), "launcher must not run without file");
}
