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
    c.env("IKSA_HOME", &h.0)
        .env("IKSA_MOCK_KEYCHAIN", "1")
        .env("IKSA_NOTIFY", "off");
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
    add(&h);
    let o = cmd(&h).env_remove("CHROMIUM").arg("run").output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("iksa setup"));
}
#[test]
#[ignore]
fn e2e_report() {
    let h = TempDir::new();
    let id = ok(run(&h, &["add", "examples/hn.json"]));
    let out = ok(run(&h, &["run"]));
    assert!(out.contains(&format!("{}\tCommitted", id.trim())), "{out}");
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
#[test]
fn second_run_is_refused() {
    use std::os::fd::AsRawFd;
    let h = TempDir::new();
    let lock = std::fs::File::create(h.0.join("run.lock")).unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    let o = cmd(&h)
        .arg("run")
        .env("CHROMIUM", "/nonexistent")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("another iksa run is active"), "{err}");
}
#[test]
fn home_required() {
    let o = Command::new(env!("CARGO_BIN_EXE_iksa"))
        .arg("list")
        .env_remove("HOME")
        .env_remove("IKSA_HOME")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("set HOME or IKSA_HOME"), "{err}");
}
#[test]
fn bad_id_is_usage_error() {
    let h = TempDir::new();
    let o = run(&h, &["approve", "x"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("usage:"));
}

#[test]
fn add_from_tty_explains() {
    let h = TempDir::new();
    let o = cmd(&h)
        .args(["add", "-"])
        .stdin(std::fs::File::open("/dev/null").unwrap())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(o.stderr).unwrap().trim(),
        "error: no task JSON on stdin"
    );
}
#[test]
fn run_with_empty_queue() {
    let h = TempDir::new();
    let o = cmd(&h)
        .arg("run")
        .env("CHROMIUM", "/nonexistent")
        .output()
        .unwrap();
    assert_eq!(ok(o).trim(), "no queued tasks");
}

#[test]
fn secret_set_uses_fake_keychain() {
    let h = TempDir::new();
    let fake = TempDir::new();
    for (args, path, value) in [
        (vec!["key", "set"], "iksa/anthropic", "fake-api-key"),
        (vec!["secret", "set", "pw"], "iksa-secret/pw", "S3cr3t-xyz"),
    ] {
        let mut child = cmd(&h)
            .args(args)
            .env("IKSA_KEYCHAIN_FAKE", &fake.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(format!("{value}\n").as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!String::from_utf8_lossy(&out.stdout).contains(value));
        assert!(!String::from_utf8_lossy(&out.stderr).contains(value));
        let path = fake.0.join(path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), value);
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn ask_without_model_refuses() {
    let h = TempDir::new();
    let o = cmd(&h)
        .args(["ask", "Read", "the", "page"])
        .env("CHROMIUM", "/nonexistent")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(o.stderr).unwrap().trim(),
        "error: configure \"model\" in config.json"
    );
    assert!(
        Ledger::open(h.0.join("ledger.db"))
            .unwrap()
            .list()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn ask_queues_goal_task() {
    let h = TempDir::new();
    let mut cfg = Config::default();
    cfg.model = Some(serde_json::from_value(serde_json::json!({"provider":"anthropic","endpoint":"http://127.0.0.1:1","name":"test-model"})).unwrap());
    std::fs::write(h.0.join("config.json"), serde_json::to_vec(&cfg).unwrap()).unwrap();
    let words = [
        "Open",
        "https://example.com/",
        "and",
        "read",
        "the",
        "title",
    ];
    let o = cmd(&h)
        .arg("ask")
        .args(words)
        .env("CHROMIUM", "/nonexistent")
        .output()
        .unwrap();
    let id: i64 = ok(o).trim().parse().unwrap();
    let ledger = Ledger::open(h.0.join("ledger.db")).unwrap();
    let goal = "Open https://example.com/ and read the title";
    let queued = ledger.get(id).unwrap();
    assert_eq!(queued.goal.as_deref(), Some(goal));
    assert_eq!(queued.title, goal);
    assert_eq!(queued.state, State::Queued);
    assert!(queued.steps.is_empty());
    assert_eq!(queued.cursor, 0);
    let scripted = add(&h);
    let list = ok(run(&h, &["list"]));
    assert!(
        list.lines()
            .any(|line| line.starts_with(&format!("{id}\t[goal] "))
                && line.contains(goal)
                && line.ends_with("Queued")),
        "{list}"
    );
    assert!(
        list.lines()
            .any(|line| line.starts_with(&format!("{scripted}\thello\t"))
                && !line.contains("[goal]")),
        "{list}"
    );
    for args in [vec!["ask"], vec!["ask", "   "]] {
        let o = run(&h, &args);
        assert_eq!(o.status.code(), Some(2));
        let usage = String::from_utf8(o.stderr).unwrap();
        assert!(usage.contains("ask <goal"), "{usage}");
        assert!(
            usage.contains("key set") && usage.contains("secret set"),
            "{usage}"
        );
    }
    assert_eq!(ledger.list().unwrap().len(), 2);
}

#[test]
fn inbox_accept_dismiss() {
    let h = TempDir::new();
    assert_eq!(ok(run(&h, &["inbox"])), "inbox empty\n");
    let ledger = Ledger::open(h.0.join("ledger.db")).unwrap();
    let first = ledger.suggest("read news").unwrap().unwrap();
    let second = ledger.suggest("check weather").unwrap().unwrap();
    let first_id = first.to_string();
    let second_id = second.to_string();
    assert_eq!(
        ok(run(&h, &["inbox"])),
        format!("{first}\tread news\n{second}\tcheck weather\n")
    );
    assert_eq!(
        ok(cmd(&h)
            .arg("run")
            .env("CHROMIUM", "/nonexistent")
            .output()
            .unwrap()),
        "no queued tasks\n"
    );
    let out = run(&h, &["accept", &first_id]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        "error: configure \"model\" in config.json\n"
    );
    assert_eq!(ledger.get(first).unwrap().state, State::Suggested);
    let mut cfg = Config::default();
    cfg.model = Some(serde_json::from_value(serde_json::json!({"provider":"anthropic","endpoint":"http://127.0.0.1:1","name":"test-model"})).unwrap());
    std::fs::write(h.0.join("config.json"), serde_json::to_vec(&cfg).unwrap()).unwrap();
    ok(run(&h, &["accept", &first_id]));
    assert_eq!(ledger.get(first).unwrap().state, State::Queued);
    let list = ok(run(&h, &["list"]));
    assert!(
        list.lines()
            .any(|line| line == format!("{first}\t[goal] read news\tQueued")),
        "{list}"
    );
    assert_eq!(
        ok(run(&h, &["inbox"])),
        format!("{second}\tcheck weather\n")
    );
    std::fs::write(
        h.0.join("config.json"),
        serde_json::to_vec(&Config::default()).unwrap(),
    )
    .unwrap();
    ok(run(&h, &["dismiss", &second_id]));
    assert_eq!(ledger.get(second).unwrap().state, State::Dismissed);
    assert_eq!(ok(run(&h, &["inbox"])), "inbox empty\n");
    for args in [["accept", "abc"], ["dismiss", "abc"]] {
        let out = run(&h, &args);
        assert_eq!(out.status.code(), Some(2));
        let usage = String::from_utf8(out.stderr).unwrap();
        assert!(
            usage.contains("inbox")
                && usage.contains("accept <id>")
                && usage.contains("dismiss <id>"),
            "{usage}"
        );
    }
    assert_eq!(run(&h, &["dismiss", &first_id]).status.code(), Some(1));
    assert_eq!(ledger.get(first).unwrap().state, State::Queued);
}

fn schedule_model(h: &TempDir) {
    let mut cfg = Config::default();
    cfg.model = Some(serde_json::from_value(serde_json::json!({"provider":"anthropic","endpoint":"http://127.0.0.1:1","name":"test-model"})).unwrap());
    std::fs::write(h.0.join("config.json"), serde_json::to_vec(&cfg).unwrap()).unwrap();
}
#[test]
fn schedule_add_list_rm() {
    let h = TempDir::new();
    schedule_model(&h);
    let before = iksa::audit::now_ms();
    let added = ok(run(&h, &["schedule", "add", "every 2h", "check", "hn"]));
    let after = iksa::audit::now_ms();
    let ledger = Ledger::open(h.0.join("ledger.db")).unwrap();
    let schedules = ledger.schedules().unwrap();
    assert_eq!(schedules.len(), 1);
    let s = &schedules[0];
    assert_eq!(s.goal, "check hn");
    assert!(!s.auto);
    assert!((before + 7_200_000..=after + 7_200_000).contains(&s.next_ms));
    let c = iksa::schedule::local(s.next_ms);
    let date = format!(
        "{:04}-{:02}-{:02} {:02}:{:02} local",
        c.year, c.month, c.day, c.hour, c.minute
    );
    assert_eq!(added, format!("{}\tnext {date}\n", s.id));
    let id = s.id.to_string();
    assert_eq!(
        ok(run(&h, &["schedule", "list"])),
        format!("{id}\tevery 2h\t{date}\tsuggest\t0/0 accepted\tcheck hn\n")
    );
    ok(run(&h, &["schedule", "set", &id, "auto"]));
    assert!(ok(run(&h, &["schedule", "list"])).contains("\tauto\t"));
    ok(run(&h, &["schedule", "set", &id, "suggest"]));
    assert!(!ledger.schedules().unwrap()[0].auto);
    ok(run(
        &h,
        &["schedule", "add", "--auto", "0 9 * * 1", "weekly", "digest"],
    ));
    assert!(ledger.schedules().unwrap()[1].auto);
    ok(run(&h, &["schedule", "rm", &id]));
    assert_eq!(ok(run(&h, &["schedule", "list"])).lines().count(), 1);
    let out = run(&h, &["schedule", "rm", "9"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        "error: schedule 9 not found\n"
    );
    assert!(
        ledger.list().unwrap().is_empty(),
        "adding schedules must not run their goals"
    );
    assert!(!h.0.join("profile").exists());
}
#[test]
fn bad_spec_is_usage_error() {
    let h = TempDir::new();
    let out = run(&h, &["schedule", "add", "every 5m", "x"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("configure \"model\""));
    schedule_model(&h);
    for (spec, reason) in [
        ("every 5s", "every 5s"),
        ("every 4m", "every 4m"),
        ("0 0 30 2 *", "never fires"),
        ("every 18446744073709551615d", "invalid interval"),
    ] {
        let out = run(&h, &["schedule", "add", spec, "x"]);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{spec}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(reason),
            "{spec}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    for args in [
        vec!["schedule", "add"],
        vec!["schedule", "add", "every 5m"],
        vec!["schedule", "add", "--auto", "every 5m", " "],
        vec!["schedule", "set", "abc", "auto"],
        vec!["schedule", "set", "1", "other"],
        vec!["schedule", "rm", "abc"],
    ] {
        let out = run(&h, &args);
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains("usage:"));
    }
    assert!(
        Ledger::open(h.0.join("ledger.db"))
            .unwrap()
            .schedules()
            .unwrap()
            .is_empty()
    );
}

struct DaemonChild(std::process::Child);
impl Drop for DaemonChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn daemon_wait(mut predicate: impl FnMut() -> bool, seconds: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    while !predicate() {
        assert!(
            std::time::Instant::now() < deadline,
            "daemon condition timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
fn stop_daemon(child: &mut DaemonChild) {
    assert_eq!(unsafe { libc::kill(child.0.id() as i32, libc::SIGTERM) }, 0);
    daemon_wait(|| child.0.try_wait().unwrap().is_some(), 3);
    assert_eq!(child.0.try_wait().unwrap().unwrap().code(), Some(0));
}
fn start_daemon(h: &TempDir) -> DaemonChild {
    DaemonChild(
        cmd(h)
            .arg("daemon")
            .env("CHROMIUM", "/nonexistent")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    )
}
#[test]
fn daemon_fires_due_schedule() {
    let h = TempDir::new();
    schedule_model(&h);
    ok(run(&h, &["schedule", "add", "every 5m", "x"]));
    rusqlite::Connection::open(h.0.join("ledger.db"))
        .unwrap()
        .execute("UPDATE schedules SET next_ms=0", [])
        .unwrap();
    let mut child = start_daemon(&h);
    daemon_wait(|| ok(run(&h, &["inbox"])).contains("\tx\n"), 5);
    stop_daemon(&mut child);
    let ledger = Ledger::open(h.0.join("ledger.db")).unwrap();
    assert_eq!(ledger.list().unwrap().len(), 1);
    assert_eq!(ledger.list().unwrap()[0].state, State::Suggested);
    // Drain still fires schedules without launching a browser for suggestions.
    rusqlite::Connection::open(h.0.join("ledger.db"))
        .unwrap()
        .execute("UPDATE schedules SET next_ms=0", [])
        .unwrap();
    assert_eq!(
        ok(cmd(&h)
            .arg("run")
            .env("CHROMIUM", "/nonexistent")
            .output()
            .unwrap()),
        "no queued tasks\n"
    );
    assert_eq!(ledger.list().unwrap().len(), 1);
}
#[test]
fn daemon_holds_run_lock() {
    let h = TempDir::new();
    let mut child = start_daemon(&h);
    daemon_wait(|| iksa::daemon::lock(&h.0).is_err(), 5);
    for command in ["run", "daemon"] {
        let out = run(&h, &[command]);
        assert_eq!(out.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&out.stderr).contains("another iksa run is active"));
        assert!(child.0.try_wait().unwrap().is_none());
    }
    stop_daemon(&mut child);
}
#[test]
fn daemon_launch_failure_keeps_polling() {
    let h = TempDir::new();
    let first = add(&h);
    let ledger = Ledger::open(h.0.join("ledger.db")).unwrap();
    let mut child = start_daemon(&h);
    daemon_wait(
        || matches!(ledger.get(first).unwrap().state, State::Failed(_)),
        5,
    );
    assert!(
        matches!(ledger.get(first).unwrap().state, State::Failed(ref message) if message.starts_with("launching Chromium"))
    );
    let second = add(&h);
    daemon_wait(
        || matches!(ledger.get(second).unwrap().state, State::Failed(_)),
        5,
    );
    assert!(child.0.try_wait().unwrap().is_none());
    stop_daemon(&mut child);
}
#[test]
fn list_explains_blocked_wait() {
    let h = TempDir::new();
    let ledger = Ledger::open(h.0.join("ledger.db")).unwrap();
    let id = ledger.add_goal("private", "private").unwrap();
    ledger.transition(id, State::Executing).unwrap();
    ledger
        .record_step(
            id,
            &iksa::audit::StepRecord::new(
                0,
                &iksa::action::Action::Extract {
                    selector: "html".into(),
                },
                "planner".into(),
                None,
                "blocked: private.example is on never_send".into(),
            ),
        )
        .unwrap();
    ledger
        .transition(id, State::AwaitingHandoff(iksa::ledger::Handoff::Approval))
        .unwrap();
    let output = ok(run(&h, &["list"]));
    assert!(
        output
            .trim_end()
            .ends_with("\tblocked: private.example is on never_send"),
        "{output}"
    );
}
#[test]
fn drain_removes_never_firing_schedule() {
    let h = TempDir::new();
    schedule_model(&h);
    let ledger = Ledger::open(h.0.join("ledger.db")).unwrap();
    let id = ledger
        .add_schedule("0 0 30 2 *", "must not run", true, 0)
        .unwrap();
    let out = cmd(&h)
        .arg("run")
        .env("CHROMIUM", "/nonexistent")
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains(&format!("schedule {id} will never fire again; removed"))
    );
    assert_eq!(ok(out), "no queued tasks\n");
    assert!(ledger.schedules().unwrap().is_empty());
    assert!(ledger.list().unwrap().is_empty());
}

#[test]
fn daemon_stops_between_launch_failures() {
    let h = TempDir::new();
    let ledger = Ledger::open(h.0.join("ledger.db")).unwrap();
    let conn = rusqlite::Connection::open(h.0.join("ledger.db")).unwrap();
    conn.execute_batch("BEGIN").unwrap();
    for _ in 0..4000 {
        conn.execute(
            r#"INSERT INTO tasks(title,steps,state) VALUES('queued failure','[]','"Queued"')"#,
            [],
        )
        .unwrap();
    }
    conn.execute_batch("COMMIT").unwrap();
    let mut child = DaemonChild(
        cmd(&h)
            .arg("daemon")
            .env("CHROMIUM", "/nonexistent")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    daemon_wait(
        || matches!(ledger.get(1).unwrap().state, State::Failed(_)),
        5,
    );
    stop_daemon(&mut child);
    assert!(
        ledger
            .list()
            .unwrap()
            .iter()
            .any(|t| t.state == State::Queued),
        "daemon kept starting tasks after SIGTERM"
    );
}
