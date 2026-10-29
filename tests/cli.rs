mod common;
use common::TempDir;
use iksa::{
    config::Config,
    ledger::{Ledger, State},
};
use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
    process::{Command, Output, Stdio},
};
fn cmd(h: &TempDir) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_iksa"));
    c.env("IKSA_HOME", &h.0).env("IKSA_MOCK_KEYCHAIN", "1");
    c
}
fn run(h: &TempDir, args: &[&str]) -> Output {
    cmd(h).args(args).output().unwrap()
}
fn ok(o: Output) -> String {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap()
}
fn task(h: &TempDir, json: &str) -> String {
    let p = h.0.join("task.json");
    std::fs::write(&p, json).unwrap();
    p.to_str().unwrap().to_owned()
}
const TASK: &str = r#"{"title":"hello","steps":[]}"#;
fn add(h: &TempDir) -> i64 {
    ok(run(h, &["add", &task(h, TASK)])).trim().parse().unwrap()
}
#[test]
fn add_and_list() {
    let h = TempDir::new();
    let id = add(&h);
    let s = ok(run(&h, &["list"]));
    assert!(s.contains(&id.to_string()) && s.contains("hello") && s.contains("Queued"));
}
#[test]
fn bad_task_json_exits_nonzero() {
    let h = TempDir::new();
    add(&h);
    let p = task(&h, r#"{"title":"bad","steps":[{"op":"fly"}]}"#);
    let o = run(&h, &["add", &p]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("error:"));
    assert_eq!(
        Ledger::open(h.0.join("ledger.db"))
            .unwrap()
            .list()
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn bad_config_exits_nonzero() {
    let h = TempDir::new();
    let mut c = Config::default();
    c.providers[0].url_template = "https://example.com".into();
    std::fs::write(h.0.join("config.json"), serde_json::to_vec(&c).unwrap()).unwrap();
    let o = run(&h, &["list"]);
    assert_eq!(o.status.code(), Some(1));
    let s = String::from_utf8_lossy(&o.stderr);
    assert!(s.contains("DuckDuckGo") && s.contains("{q}"), "{s}");
}
#[test]
fn unknown_config_field_shows_reason() {
    let h = TempDir::new();
    let mut c = serde_json::to_value(Config::default()).unwrap();
    c["bogus"] = 1.into();
    std::fs::write(h.0.join("config.json"), c.to_string()).unwrap();
    let o = run(&h, &["list"]);
    assert_eq!(o.status.code(), Some(1));
    let s = String::from_utf8_lossy(&o.stderr);
    assert!(s.contains("config.json") && s.contains("bogus"), "{s}");
}
#[test]
fn existing_home_tightened_to_0700() {
    let h = TempDir::new();
    std::fs::set_permissions(&h.0, std::fs::Permissions::from_mode(0o755)).unwrap();
    ok(run(&h, &["list"]));
    assert_eq!(
        std::fs::metadata(&h.0).unwrap().permissions().mode() & 0o777,
        0o700
    );
}
#[test]
fn add_from_stdin() {
    let h = TempDir::new();
    let mut c = cmd(&h)
        .args(["add", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin.take().unwrap().write_all(TASK.as_bytes()).unwrap();
    ok(c.wait_with_output().unwrap());
    assert!(ok(run(&h, &["list"])).contains("hello"));
}
#[test]
fn add_secret_file_warns() {
    let h = TempDir::new();
    let p = task(
        &h,
        r##"{"title":"secret","steps":[{"op":"type","selector":"#x","text":"private","secret":true}]}"##,
    );
    let o = run(&h, &["add", &p]);
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("contains a secret"));
}
#[test]
fn run_after_crash_recovers_then_gcs() {
    let h = TempDir::new();
    let id = add(&h);
    {
        let l = Ledger::open(h.0.join("ledger.db")).unwrap();
        l.transition(id, State::Executing).unwrap();
    }
    let o = cmd(&h)
        .env("CHROMIUM", "/nonexistent")
        .arg("run")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    let s = String::from_utf8_lossy(&o.stderr);
    assert!(s.contains("launching Chromium /nonexistent"), "{s}");
    assert!(!s.contains("executing"), "{s}");
    assert!(ok(run(&h, &["list"])).contains("Queued"));
}
#[test]
fn chromium_launch_failure_keeps_task_queued() {
    let h = TempDir::new();
    add(&h);
    let o = cmd(&h)
        .env("CHROMIUM", "/nonexistent")
        .arg("run")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(ok(run(&h, &["list"])).contains("Queued"));
}
#[test]
fn run_without_setup_says_run_setup() {
    let h = TempDir::new();
    let o = cmd(&h).env_remove("CHROMIUM").arg("run").output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("iksa setup"));
}
#[test]
#[ignore]
fn e2e_report() {
    let h = TempDir::new();
    let id = ok(run(&h, &["add", "examples/hn.json"]));
    ok(run(&h, &["run"]));
    let report = ok(run(&h, &["report", id.trim()]));
    assert!(
        report.contains("## Step 2") && report.contains("Committed"),
        "{report}"
    );
    let o = Command::new("du")
        .args(["-sk"])
        .arg(h.0.join("cas"))
        .output()
        .unwrap();
    assert!(o.status.success());
    let s = String::from_utf8(o.stdout).unwrap();
    eprintln!("e2e CAS du -sk: {s}");
    assert!(s.split_whitespace().next().unwrap().parse::<u64>().unwrap() < 500);
}
