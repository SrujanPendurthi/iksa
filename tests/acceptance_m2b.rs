//! Independent acceptance checks for M2b, written from the plan
//! (docs/superpowers/plans/2026-11-04-m2b-proactive.md), not from the implementation.
//! Implementers must not edit this file to make it pass; a wrong expectation is reported as BLOCKED.
mod common;
use common::TempDir;
use iksa::{
    action::Action,
    challenge::ChallengeKind,
    config::Config,
    gate::Risk,
    ledger::{Handoff, Ledger, State, waiting_on},
    schedule::{self, Spec},
};
use serde_json::json;
use std::{
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

const HOUR: u64 = 3_600_000;

fn cmd(h: &TempDir) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_iksa"));
    c.env("IKSA_HOME", &h.0)
        .env("IKSA_MOCK_KEYCHAIN", "1")
        .env("IKSA_NOTIFY", "off")
        .env("CHROMIUM", "/nonexistent");
    c
}
fn run(h: &TempDir, args: &[&str]) -> Output {
    cmd(h).args(args).output().unwrap()
}
fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}
fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}
fn ok(o: Output) -> String {
    assert!(o.status.success(), "{}", stderr(&o));
    stdout(&o)
}
fn write_config(h: &TempDir, edit: impl FnOnce(&mut serde_json::Value)) {
    let mut v = serde_json::to_value(Config::default()).unwrap();
    edit(&mut v);
    std::fs::write(h.0.join("config.json"), v.to_string()).unwrap();
}
fn with_model(h: &TempDir) {
    write_config(h, |v| {
        v["model"] = json!({"provider":"anthropic","name":"claude-opus-5-5"});
    });
}
fn ledger(h: &TempDir) -> Ledger {
    Ledger::open(h.0.join("ledger.db")).unwrap()
}
fn force_due(h: &TempDir) {
    rusqlite::Connection::open(h.0.join("ledger.db"))
        .unwrap()
        .execute("UPDATE schedules SET next_ms=0", [])
        .unwrap();
}
fn wait_until(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}
struct Daemon(Child);
impl Daemon {
    fn start(h: &TempDir) -> Self {
        let child = cmd(h)
            .arg("daemon")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // The run lock is taken before the first tick.
        std::thread::sleep(Duration::from_millis(500));
        Daemon(child)
    }
    fn term(&mut self) -> Option<i32> {
        unsafe { libc::kill(self.0.id() as i32, libc::SIGTERM) };
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status.code();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn pause_for_approval(l: &Ledger, action: Action) -> i64 {
    let id = l.add("t", &[action]).unwrap();
    l.transition(id, State::Executing).unwrap();
    l.transition(id, State::AwaitingHandoff(Handoff::Approval))
        .unwrap();
    id
}

// ---- Task 0: inbox ----

#[test]
fn a0_suggestions_never_run_until_accepted() {
    let h = TempDir::new();
    let l = ledger(&h);
    let a = l.suggest("check prices").unwrap().unwrap();
    assert_eq!(l.get(a).unwrap().state, State::Suggested);
    assert!(
        l.next_queued().unwrap().is_none(),
        "a suggestion was queued"
    );
    assert_eq!(
        l.suggest("check prices").unwrap(),
        None,
        "open goal deduped"
    );
    l.dismiss(a).unwrap();
    assert_eq!(l.get(a).unwrap().state, State::Dismissed);
    assert!(l.get(a).unwrap().finished_ms.is_some());
    assert!(l.accept(a).is_err(), "dismissed task accepted");
    assert!(
        l.suggest("check prices").unwrap().is_some(),
        "dismissed goal blocks re-suggest"
    );
    let q = l.add_goal("queued goal", "queued goal").unwrap();
    assert!(l.dismiss(q).is_err(), "queued task dismissed");
    assert_eq!(l.suggest("queued goal").unwrap(), None);
}
#[test]
fn a0_inbox_cli() {
    let h = TempDir::new();
    ok(run(&h, &["list"]));
    let l = ledger(&h);
    let a = l.suggest("first idea").unwrap().unwrap();
    let b = l.suggest("second idea").unwrap().unwrap();
    let inbox = ok(run(&h, &["inbox"]));
    assert!(
        inbox.contains("first idea") && inbox.contains("second idea"),
        "{inbox}"
    );
    let o = run(&h, &["accept", &a.to_string()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("configure \"model\""), "{}", stderr(&o));
    with_model(&h);
    ok(run(&h, &["accept", &a.to_string()]));
    assert_eq!(l.get(a).unwrap().state, State::Queued);
    ok(run(&h, &["dismiss", &b.to_string()]));
    assert!(ok(run(&h, &["inbox"])).contains("inbox empty"));
    assert_eq!(run(&h, &["accept", "abc"]).status.code(), Some(2));
}

// ---- Task 1: schedule specs ----

#[test]
fn a1_spec_parsing() {
    assert_eq!(
        schedule::parse("every 30m").unwrap(),
        Spec::Every(Duration::from_secs(1800))
    );
    assert!(schedule::parse("every 2h").is_ok() && schedule::parse("every 1d").is_ok());
    for bad in [
        "every 0m",
        "every 5s",
        "every m",
        "sometimes",
        "* * * *",
        "* * * * * *",
        "60 * * * *",
        "* * * 13 *",
    ] {
        assert!(schedule::parse(bad).is_err(), "accepted {bad:?}");
    }
    let e = schedule::parse("0 0 30 2 *").unwrap_err();
    assert!(e.contains("never fires"), "{e}");
    assert!(
        schedule::parse("0 0 29 2 *").is_ok(),
        "Feb 29 fires in leap years"
    );
}
fn civil(day: u8, weekday: u8, hour: u8, minute: u8) -> schedule::Civil {
    schedule::Civil {
        year: 2026,
        month: 11,
        day,
        hour,
        minute,
        weekday,
    }
}
fn cron(s: &str) -> schedule::Cron {
    match schedule::parse(s).unwrap() {
        Spec::Cron(c) => c,
        other => panic!("{s} parsed as {other:?}"),
    }
}
#[test]
fn a1_cron_fields_and_dom_or_dow() {
    let weekdays = cron("30 9 * * 1-5");
    assert!(weekdays.matches(&civil(16, 1, 9, 30)));
    assert!(
        !weekdays.matches(&civil(14, 6, 9, 30)),
        "Saturday matched 1-5"
    );
    assert!(!weekdays.matches(&civil(16, 1, 9, 31)));
    let quarter = cron("*/15 * * * *");
    assert!(quarter.matches(&civil(1, 0, 3, 45)) && !quarter.matches(&civil(1, 0, 3, 46)));
    assert!(
        cron("0 0 * * 7").matches(&civil(15, 0, 0, 0)),
        "7 is Sunday"
    );
    assert!(cron("0 0 1,15 * *").matches(&civil(15, 0, 0, 0)));
    let either = cron("0 0 13 * 5");
    assert!(either.matches(&civil(14, 5, 0, 0)), "Friday via dow");
    assert!(either.matches(&civil(13, 3, 0, 0)), "13th via dom");
    assert!(!either.matches(&civil(12, 4, 0, 0)));
}
#[test]
fn a1_next_fire_is_strictly_next_matching_minute() {
    let now = iksa::audit::now_ms();
    for s in ["30 9 * * 1-5", "0 * * * *", "*/7 * * * *"] {
        let spec = schedule::parse(s).unwrap();
        let c = cron(s);
        let t = schedule::next_fire_ms(&spec, now).unwrap();
        assert!(t > now && t % 60_000 == 0, "{s}: {t}");
        assert!(
            c.matches(&schedule::local(t)),
            "{s}: next fire does not match"
        );
        let mut m = (now / 60_000 + 1) * 60_000;
        while m < t {
            assert!(
                !c.matches(&schedule::local(m)),
                "{s}: skipped an earlier match at {m}"
            );
            m += 60_000;
        }
    }
    let every = schedule::parse("every 2h").unwrap();
    assert_eq!(
        schedule::next_fire_ms(&every, 1_000),
        Some(1_000 + 2 * HOUR)
    );
}

// ---- Task 2: schedules ----

#[test]
fn a2_missed_runs_collapse_and_open_task_blocks_fire() {
    let h = TempDir::new();
    let l = ledger(&h);
    let s = l.add_schedule("every 1h", "hourly", true, 0).unwrap();
    let fired = l.fire_due(72 * HOUR, |_, now| now + HOUR).unwrap();
    assert_eq!(
        fired.len(),
        1,
        "three days asleep produced {} tasks",
        fired.len()
    );
    assert_eq!(
        l.get(fired[0]).unwrap().state,
        State::Queued,
        "auto schedule queues"
    );
    assert_eq!(l.get(fired[0]).unwrap().schedule_id, Some(s));
    assert!(
        l.fire_due(72 * HOUR, |_, now| now + HOUR)
            .unwrap()
            .is_empty()
    );
    force_due(&h);
    assert!(
        l.fire_due(73 * HOUR, |_, now| now + HOUR)
            .unwrap()
            .is_empty(),
        "fired while the previous task is still open"
    );
    assert_eq!(
        l.schedules().unwrap()[0].next_ms,
        74 * HOUR,
        "next_ms not advanced on skip"
    );
    l.transition(fired[0], State::Executing).unwrap();
    l.transition(fired[0], State::Failed("x".into())).unwrap();
    force_due(&h);
    assert_eq!(l.fire_due(74 * HOUR, |_, now| now + HOUR).unwrap().len(), 1);
}
#[test]
fn a2_suggest_mode_and_trial_counts() {
    let h = TempDir::new();
    let l = ledger(&h);
    l.add_schedule("every 1h", "digest", false, 0).unwrap();
    let mut ids = vec![];
    for i in 0..3 {
        force_due(&h);
        let fired = l.fire_due(i * HOUR + 1, |_, now| now + HOUR).unwrap();
        assert_eq!(fired.len(), 1);
        assert_eq!(
            l.get(fired[0]).unwrap().state,
            State::Suggested,
            "suggest mode queued"
        );
        if i == 0 {
            l.accept(fired[0]).unwrap();
        } else {
            l.dismiss(fired[0]).unwrap();
        }
        if i == 0 {
            // An accepted task must close before the schedule fires again.
            l.transition(fired[0], State::Executing).unwrap();
            l.transition(fired[0], State::Committed).unwrap();
        }
        ids.push(fired[0]);
    }
    let s = &l.schedules().unwrap()[0];
    assert_eq!((s.accepted, s.dismissed), (1, 2));
}
#[test]
fn a2_at_most_32_schedules() {
    let h = TempDir::new();
    let l = ledger(&h);
    for i in 0..32 {
        l.add_schedule("every 1h", &format!("g{i}"), false, HOUR)
            .unwrap();
    }
    let e = l.add_schedule("every 1h", "g32", false, HOUR).unwrap_err();
    assert!(e.to_string().contains("at most 32"), "{e}");
}
#[test]
fn a2_schedule_cli() {
    let h = TempDir::new();
    let o = run(&h, &["schedule", "add", "every 2h", "check", "hn"]);
    assert_eq!(o.status.code(), Some(1), "added without a model");
    assert!(stderr(&o).contains("configure \"model\""));
    with_model(&h);
    for (spec, needle) in [
        ("every 5s", "every 5s"),
        ("every 4m", ""),
        ("0 0 30 2 *", "never fires"),
    ] {
        let o = run(&h, &["schedule", "add", spec, "x"]);
        assert_eq!(o.status.code(), Some(2), "{spec}: {}", stderr(&o));
        assert!(stderr(&o).contains(needle), "{spec}: {}", stderr(&o));
    }
    assert!(
        ledger(&h).schedules().unwrap().is_empty(),
        "a rejected spec was stored"
    );
    assert!(ok(run(&h, &["schedule", "add", "every 2h", "check", "hn"])).contains("next "));
    let list = ok(run(&h, &["schedule", "list"]));
    assert!(
        list.contains("every 2h") && list.contains("suggest") && list.contains("0/0 accepted"),
        "{list}"
    );
    ok(run(&h, &["schedule", "set", "1", "auto"]));
    assert!(ok(run(&h, &["schedule", "list"])).contains("\tauto\t"));
    assert!(
        ok(run(
            &h,
            &["schedule", "add", "--auto", "0 9 * * 1", "weekly"]
        ))
        .contains("next ")
    );
    ok(run(&h, &["schedule", "rm", "1"]));
    assert_eq!(ok(run(&h, &["schedule", "list"])).lines().count(), 1);
    let o = run(&h, &["schedule", "rm", "9"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains("schedule 9 not found"),
        "{}",
        stderr(&o)
    );
}

// ---- Task 3: daemon ----

#[test]
fn a3_waiting_on_and_digest() {
    let h = TempDir::new();
    let l = ledger(&h);
    let a = pause_for_approval(
        &l,
        Action::Click {
            selector: "#b".into(),
            risk: Risk::Write,
        },
    );
    let mut t = l.get(a).unwrap();
    assert_eq!(
        waiting_on(&t).as_deref(),
        Some("approve step 1: click (write)")
    );
    t.state = State::AwaitingHandoff(Handoff::Challenge(ChallengeKind::Turnstile));
    assert_eq!(waiting_on(&t).as_deref(), Some("solve Turnstile challenge"));
    t.state = State::Committed;
    assert_eq!(waiting_on(&t), None);
    assert_eq!(
        iksa::daemon::digest(&l.list().unwrap()).as_deref(),
        Some("waiting on you: 1 approval")
    );
    pause_for_approval(
        &l,
        Action::Click {
            selector: "#c".into(),
            risk: Risk::Write,
        },
    );
    for g in ["s1", "s2", "s3"] {
        l.suggest(g).unwrap();
    }
    let c = l.add("c", &[]).unwrap();
    l.transition(c, State::Executing).unwrap();
    l.transition(
        c,
        State::AwaitingHandoff(Handoff::Challenge(ChallengeKind::HCaptcha)),
    )
    .unwrap();
    assert_eq!(
        iksa::daemon::digest(&l.list().unwrap()).as_deref(),
        Some("waiting on you: 2 approvals, 1 challenge, 3 suggestions")
    );
    let empty = TempDir::new();
    assert_eq!(iksa::daemon::digest(&ledger(&empty).list().unwrap()), None);
}
#[test]
fn a3_daemon_fires_schedule_and_stops_on_sigterm() {
    let h = TempDir::new();
    with_model(&h);
    ok(run(
        &h,
        &["schedule", "add", "every 5m", "morning", "digest"],
    ));
    force_due(&h);
    let mut d = Daemon::start(&h);
    assert!(
        wait_until(Duration::from_secs(5), || ok(run(&h, &["inbox"]))
            .contains("morning digest")),
        "daemon did not fire the due schedule into the inbox"
    );
    assert_eq!(
        d.term(),
        Some(0),
        "daemon did not exit 0 within 3 s of SIGTERM"
    );
}
#[test]
fn a3_one_runner_per_data_dir() {
    let h = TempDir::new();
    ok(run(&h, &["list"]));
    let mut d = Daemon::start(&h);
    for args in [&["run"][..], &["daemon"][..]] {
        let o = run(&h, args);
        assert_eq!(o.status.code(), Some(1), "{args:?}: {}", stderr(&o));
        assert!(
            stderr(&o).contains("another iksa run is active"),
            "{args:?}: {}",
            stderr(&o)
        );
    }
    assert_eq!(d.term(), Some(0), "first daemon was disturbed");
}
#[test]
fn a3_run_with_nothing_due_never_launches_chromium() {
    let h = TempDir::new();
    let o = run(&h, &["run"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("no queued tasks"));
}
#[test]
fn a3_idle_daemon_has_no_browser_and_no_sockets() {
    let h = TempDir::new();
    ok(run(&h, &["list"]));
    let mut d = Daemon::start(&h);
    std::thread::sleep(Duration::from_secs(3));
    let pid = d.0.id();
    assert!(
        common::descendants(pid).is_empty(),
        "idle daemon has child processes"
    );
    let o = Command::new("lsof")
        .args(["-nP", "-a", "-p", &pid.to_string(), "-i"])
        .output()
        .unwrap();
    assert!(
        o.stdout.is_empty(),
        "idle daemon holds sockets: {}",
        stdout(&o)
    );
    assert_eq!(d.term(), Some(0));
}

// ---- Task 4: max_agents ----

#[test]
fn a4_max_agents_range() {
    let h = TempDir::new();
    for bad in [0, 9] {
        write_config(&h, |v| v["max_agents"] = json!(bad));
        let o = run(&h, &["list"]);
        assert_eq!(o.status.code(), Some(1), "max_agents {bad} accepted");
        assert!(stderr(&o).contains("max_agents"), "{}", stderr(&o));
    }
    write_config(&h, |v| {
        v.as_object_mut().unwrap().remove("max_agents");
    });
    ok(run(&h, &["list"]));
    assert_eq!(Config::load(&h.0).unwrap().max_agents, 3);
}

// ---- Task 5: follow-up suggestions ----

#[test]
fn a5_suggestions_bounded() {
    let long = "x".repeat(301);
    let input = json!({"answer":"a","suggestions":["  watch prices  ", "", long, "watch prices", "b", "c", "d"]});
    assert_eq!(
        iksa::planner::suggestions(&input),
        vec!["watch prices", "b", "c"]
    );
    assert!(iksa::planner::suggestions(&json!({"answer":"old mock"})).is_empty());
    let tools = iksa::planner::tools();
    let done = tools
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "done")
        .unwrap();
    let mut required: Vec<_> = done["input_schema"]["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    required.sort_unstable();
    assert_eq!(required, ["answer", "suggestions"]);
}

// Acceptance checks for tasks not built yet. When starting a task, delete its `#[cfg(any())]`
// line (and nothing else); the task is done only when these pass unchanged.
#[cfg(any())]
#[path = "acceptance/m2b_task6.rs"]
mod task6;
#[cfg(any())]
#[path = "acceptance/m2b_task7.rs"]
mod task7;
#[path = "acceptance/m2b_task8.rs"]
mod task8;
