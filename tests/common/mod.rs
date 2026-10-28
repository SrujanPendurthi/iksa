#![allow(dead_code)]
use std::{
    io::{Read, Write},
    net::TcpListener,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
pub fn chromium() -> PathBuf {
    std::env::var_os("CHROMIUM")
        .expect("set CHROMIUM to the pinned Chromium executable")
        .into()
}
pub fn temp_profile() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "iksa-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
    p
}

pub fn test_args(profile: &Path) -> Vec<String> {
    let mut a = vec![format!("--user-data-dir={}", profile.display())];
    a.extend(
        [
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-sync",
            "--no-pings",
            "--disable-domain-reliability",
            "--disable-field-trial-config",
            "--disable-breakpad",
            "--metrics-recording-only",
            "--no-default-browser-check",
            "--no-first-run",
            "--safebrowsing-disable-auto-update",
            "--disable-features=PreloadTopChromeWebUI,WebUIOmniboxPopup,WebUIOmniboxAimPopup",
            "--use-mock-keychain",
        ]
        .map(String::from),
    );
    a
}
#[derive(Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub hang: bool,
}
pub fn serve(routes: &[(&str, Response)]) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let routes: Vec<_> = routes
        .iter()
        .map(|(p, r)| (p.to_string(), r.clone()))
        .collect();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = [0; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]);
            let path = request.split_whitespace().nth(1).unwrap_or("/");
            let fallback = Response {
                status: 404,
                headers: vec![],
                body: String::new(),
                hang: false,
            };
            let r = routes
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, r)| r)
                .unwrap_or(&fallback);
            if r.hang {
                std::thread::spawn(move || {
                    let _ = stream.read(&mut [0; 1]);
                });
                continue;
            }
            let _ = write!(
                stream,
                "HTTP/1.1 {} OK\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: text/html\r\n",
                r.status,
                r.body.len()
            );
            for (k, v) in &r.headers {
                let _ = write!(stream, "{k}: {v}\r\n");
            }
            let _ = write!(stream, "\r\n{}", r.body);
        }
    });
    port
}

/// Transitive children from one process snapshot, excluding the root.
pub fn descendants(pid: u32) -> Vec<u32> {
    let output = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid="])
        .output()
        .unwrap();
    assert!(output.status.success());
    let pairs: Vec<(u32, u32)> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| {
            let mut cols = line.split_whitespace();
            (
                cols.next().unwrap().parse().unwrap(),
                cols.next().unwrap().parse().unwrap(),
            )
        })
        .collect();
    let mut tree = vec![pid];
    loop {
        let added: Vec<_> = pairs
            .iter()
            .filter(|(pid, parent)| tree.contains(parent) && !tree.contains(pid))
            .map(|(pid, _)| *pid)
            .collect();
        if added.is_empty() {
            break;
        }
        tree.extend(added);
    }
    tree.remove(0);
    tree
}
