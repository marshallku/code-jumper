//! The HTTP front-end. Binds 127.0.0.1 only, authenticates /open with the
//! config token, and shells out to the bash launcher (in --strict mode) for the
//! actual nvim/tmux work. Fire-and-forget: the response does not block on nvim.

use std::process::Command;
use std::sync::Arc;
use std::thread;

use tiny_http::{Header, Method, Request, Response, Server};

use crate::config::Config;
use crate::util::{ct_eq, parse_query};

const WORKERS: usize = 4;

fn cors<R: std::io::Read>(resp: Response<R>) -> Response<R> {
    resp.with_header(Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap())
        .with_header(
            Header::from_bytes(&b"Access-Control-Allow-Private-Network"[..], &b"true"[..]).unwrap(),
        )
        .with_header(
            Header::from_bytes(&b"Access-Control-Allow-Methods"[..], &b"GET, OPTIONS"[..]).unwrap(),
        )
        .with_header(Header::from_bytes(&b"Access-Control-Allow-Headers"[..], &b"*"[..]).unwrap())
}

pub fn serve(cfg: Config) {
    if cfg.token.is_empty() || cfg.launcher.is_empty() {
        eprintln!("code-jumper-daemon: config missing token/launcher; run `install` first");
        std::process::exit(1);
    }
    let addr = format!("127.0.0.1:{}", cfg.port);
    let server = match Server::http(&addr) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("code-jumper-daemon: failed to bind {addr}: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("code-jumper-daemon: listening on http://{addr}");
    let cfg = Arc::new(cfg);

    let mut handles = Vec::new();
    for _ in 0..WORKERS {
        let server = server.clone();
        let cfg = cfg.clone();
        handles.push(thread::spawn(move || {
            while let Ok(req) = server.recv() {
                handle(req, &cfg);
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
}

fn handle(req: Request, cfg: &Config) {
    if *req.method() == Method::Options {
        let _ = req.respond(cors(Response::empty(204)));
        return;
    }
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    match path {
        "/health" => {
            let body = format!("code-jumper-daemon {}", env!("CARGO_PKG_VERSION"));
            let _ = req.respond(cors(Response::from_string(body)));
        }
        "/open" => handle_open(req, query, cfg),
        _ => {
            let _ = req.respond(Response::from_string("not found").with_status_code(404));
        }
    }
}

fn handle_open(req: Request, query: &str, cfg: &Config) {
    let mut token = String::new();
    let mut file = String::new();
    for (k, v) in parse_query(query) {
        match k.as_str() {
            "token" => token = v,
            "file" => file = v,
            _ => {}
        }
    }

    if !ct_eq(token.as_bytes(), cfg.token.as_bytes()) {
        let _ = req.respond(cors(Response::from_string("forbidden").with_status_code(403)));
        return;
    }
    if file.is_empty() {
        let _ = req.respond(cors(Response::from_string("missing file").with_status_code(400)));
        return;
    }

    // Spawn the launcher in --strict mode (no most-recent fallback: a request for
    // a file under no registered nvim cwd opens nothing). Reap in a side thread so
    // exited children don't become zombies, without blocking the response.
    let mut cmd = Command::new(&cfg.launcher);
    cmd.arg("--strict").arg(&file);
    if let Some(rd) = &cfg.registry_dir {
        cmd.env("CODE_JUMPER_REGISTRY_DIR", rd);
    }
    match cmd.spawn() {
        Ok(mut child) => {
            thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => eprintln!("code-jumper-daemon: spawn launcher failed: {e}"),
    }

    // A navigated tab (Sec-Fetch-Dest: document) gets a self-closing page so it
    // doesn't linger; a fetch() gets an empty 204.
    let is_document = req.headers().iter().any(|h| {
        h.field.equiv("Sec-Fetch-Dest") && h.value.as_str().eq_ignore_ascii_case("document")
    });
    if is_document {
        let resp = Response::from_string("<!doctype html><script>window.close()</script>opened")
            .with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
            );
        let _ = req.respond(cors(resp));
    } else {
        let _ = req.respond(cors(Response::empty(204)));
    }
}
