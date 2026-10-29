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
    std::fs::create_dir_all("target/codex").unwrap();
    std::fs::write(format!("target/codex/ps-tree-{pid}.log"), &output.stdout).unwrap();
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
/// All-or-nothing process tree phys_footprint, with raw evidence retained.
pub fn tree_footprint_kib(pid: u32) -> u64 {
    let dir = std::env::current_dir().unwrap().join("target/codex");
    std::fs::create_dir_all(&dir).unwrap();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut pids = vec![pid];
    pids.extend(descendants(pid));
    std::fs::write(
        dir.join(format!("footprint-{stamp}-pids.json")),
        serde_json::to_vec(&pids).unwrap(),
    )
    .unwrap();
    let mut bytes = 0u64;
    let mut errors = vec![];
    for child in pids {
        let output = std::process::Command::new("footprint")
            .args(["-p", &child.to_string(), "-f", "bytes"])
            .output();
        match output {
            Ok(o) => {
                let raw = format!(
                    "{}{}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                );
                std::fs::write(dir.join(format!("footprint-{stamp}-{child}.log")), &raw).unwrap();
                let value = raw.lines().find_map(|line| {
                    line.trim()
                        .strip_prefix("phys_footprint:")
                        .and_then(|v| v.trim().strip_suffix(" B"))
                        .and_then(|n| n.parse::<u64>().ok())
                });
                match (o.status.success(), value) {
                    (true, Some(n)) => bytes += n,
                    _ => errors.push(format!(
                        "PID {child}: status {} missing/inaccessible phys_footprint",
                        o.status
                    )),
                }
            }
            Err(e) => errors.push(format!("PID {child}: {e}")),
        }
    }
    assert!(
        errors.is_empty(),
        "BLOCKED footprint: {}; no partial sum",
        errors.join("; ")
    );
    println!(
        "footprint root={pid} raw stamp={stamp} total={bytes} bytes, {} KiB",
        bytes / 1024
    );
    bytes / 1024
}
fn network_host(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let authority = if let Some((scheme, rest)) = raw.split_once("://") {
        if !matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "ws" | "wss" | "ftp"
        ) {
            return None;
        }
        rest.split(['/', '?', '#']).next()?
    } else {
        raw
    };
    let authority = authority.rsplit('@').next()?;
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split_once(']')?.0
    } else if authority.parse::<std::net::IpAddr>().is_ok() {
        authority
    } else {
        authority.split(':').next()?
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || host == "localhost" || host.ends_with(".localhost") {
        return None;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if ip.is_loopback() {
            return None;
        }
    }
    Some(host)
}
pub fn netlog_hosts(path: &Path) -> Vec<String> {
    let raw = std::fs::read(path).unwrap();
    let log: serde_json::Value =
        serde_json::from_slice(&raw).expect("BLOCKED netlog invalid or unflushed JSON");
    let events = log["events"]
        .as_array()
        .expect("BLOCKED netlog has no events");
    assert!(!events.is_empty(), "BLOCKED empty netlog capture");
    let mut hosts = std::collections::BTreeSet::new();
    fn walk(v: &serde_json::Value, hosts: &mut std::collections::BTreeSet<String>) {
        match v {
            serde_json::Value::Object(o) => {
                for (k, v) in o {
                    if matches!(k.as_str(), "url" | "host" | "address") {
                        if let Some(s) = v.as_str() {
                            if let Some(h) = network_host(s) {
                                hosts.insert(h);
                            }
                        }
                    }
                    walk(v, hosts)
                }
            }
            serde_json::Value::Array(a) => {
                for v in a {
                    walk(v, hosts)
                }
            }
            _ => (),
        }
    }
    for event in events {
        walk(&event["params"], &mut hosts);
    }
    hosts.into_iter().collect()
}
#[test]
fn netlog_parser_covers_all_events_and_network_forms() {
    let path = std::env::current_dir().unwrap().join(format!(
        "target/codex/netlog-parser-{}.json",
        std::process::id()
    ));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path,r#"{"events":[{"params":{"url":"https://Example.COM:443/a","host":"localhost:80","address":"[::1]:443"}},{"params":{"url":"chrome://newtab/","nested":{"host":"remote.test:443","address":"[2001:db8::1]:443"}}},{"params":{"address":"0.0.0.0:9","host":"127.0.0.2:80"}}]}"#).unwrap();
    assert_eq!(
        netlog_hosts(&path),
        vec!["0.0.0.0", "2001:db8::1", "example.com", "remote.test"]
    );
}
