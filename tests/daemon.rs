mod common;
#[allow(dead_code)]
#[path = "eval/support.rs"]
mod support;
use iksa::{
    config::Config,
    daemon::{self, Opts},
    ledger::{Ledger, State},
};
use std::{process::Command, time::Duration};
use support::Relay;

fn isolated(name: &str) -> bool {
    if std::env::var("IKSA_DAEMON_TEST_CHILD").as_deref() == Ok(name) {
        return false;
    }
    let fake = common::temp_dir("daemon-fake-keychain");
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            name,
            "--ignored",
            "--nocapture",
            "--skip",
            "live_",
        ])
        .env("IKSA_DAEMON_TEST_CHILD", name)
        .env("ANTHROPIC_API_KEY", "offline-test")
        .env("IKSA_MOCK_KEYCHAIN", "1")
        .env("IKSA_KEYCHAIN_FAKE", &fake.0)
        .env("IKSA_NOTIFY", "off")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    true
}
fn config(home: &std::path::Path, relay: &Relay) {
    std::fs::write(
        home.join("config.json"),
        serde_json::to_vec(&Config {
            model: Some(relay.config()),
            ..Config::default()
        })
        .unwrap(),
    )
    .unwrap();
}
fn opts(drain: bool) -> Opts {
    Opts {
        drain,
        poll: Duration::from_millis(200),
        idle_close: Duration::from_secs(1),
        chromium: Some(common::chromium()),
    }
}
fn children() -> Vec<u32> {
    let probe = Command::new("ps")
        .args(["-axo", "pid=,ppid="])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let probe_id = probe.id();
    let output = probe.wait_with_output().unwrap();
    assert!(output.status.success());
    let pairs: Vec<(u32, u32)> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            (
                fields.next().unwrap().parse().unwrap(),
                fields.next().unwrap().parse().unwrap(),
            )
        })
        .collect();
    let root = std::process::id();
    assert!(
        pairs
            .iter()
            .any(|&(pid, parent)| pid == probe_id && parent == root)
    );
    let mut tree = vec![root];
    loop {
        // Exclude only this exact measurement process; retain every real child.
        let next: Vec<_> = pairs
            .iter()
            .filter(|(pid, parent)| {
                *pid != probe_id && tree.contains(parent) && !tree.contains(pid)
            })
            .map(|(pid, _)| *pid)
            .collect();
        if next.is_empty() {
            break;
        }
        tree.extend(next);
    }
    tree.remove(0);
    tree
}
async fn until(mut predicate: impl FnMut() -> bool, seconds: u64) {
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    while !predicate() {
        assert!(
            std::time::Instant::now() < deadline,
            "daemon condition timed out"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
async fn stop(task: tokio::task::JoinHandle<anyhow::Result<()>>, signal: i32) {
    assert_eq!(unsafe { libc::kill(std::process::id() as i32, signal) }, 0);
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
#[tokio::test]
#[ignore]
async fn daemon_recovers_executing() {
    if isolated("daemon_recovers_executing") {
        return;
    }
    let home = common::temp_dir("daemon-recover");
    let relay = Relay::mock();
    config(&home.0, &relay);
    let ledger = Ledger::open(home.0.join("ledger.db")).unwrap();
    let id = ledger.add_goal("recover", "Say done").unwrap();
    ledger.transition(id, State::Executing).unwrap();
    tokio::time::timeout(Duration::from_secs(10), daemon::run(&home.0, opts(true)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ledger.get(id).unwrap().state, State::Committed);
}
#[tokio::test]
#[ignore]
async fn idle_daemon_closes_browser() {
    if isolated("idle_daemon_closes_browser") {
        return;
    }
    let home = common::temp_dir("daemon-idle");
    let (relay, started, release) = Relay::held_mock();
    config(&home.0, &relay);
    let ledger = Ledger::open(home.0.join("ledger.db")).unwrap();
    let id = ledger.add_goal("idle", "Say done").unwrap();
    tokio::task::LocalSet::new()
        .run_until(async {
            let path = home.0.clone();
            let task =
                tokio::task::spawn_local(async move { daemon::run(&path, opts(false)).await });
            until(|| started.try_recv().is_ok(), 10).await;
            assert_eq!(ledger.get(id).unwrap().state, State::Executing);
            assert!(!children().is_empty());
            release.send(()).unwrap();
            until(|| ledger.get(id).unwrap().state == State::Committed, 10).await;
            tokio::time::sleep(Duration::from_secs(3)).await;
            assert!(!task.is_finished());
            assert!(children().is_empty(), "idle daemon retained Chromium");
            stop(task, libc::SIGTERM).await;
        })
        .await;
}
#[tokio::test]
#[ignore]
async fn daemon_runs_new_ask_without_restart() {
    if isolated("daemon_runs_new_ask_without_restart") {
        return;
    }
    let home = common::temp_dir("daemon-new-ask");
    let relay = Relay::mock();
    config(&home.0, &relay);
    let ledger = Ledger::open(home.0.join("ledger.db")).unwrap();
    tokio::task::LocalSet::new()
        .run_until(async {
            let path = home.0.clone();
            let task =
                tokio::task::spawn_local(async move { daemon::run(&path, opts(false)).await });
            until(|| daemon::lock(&home.0).is_err(), 5).await;
            assert!(children().is_empty());
            let id = ledger.add_goal("new ask", "Say done").unwrap();
            until(|| ledger.get(id).unwrap().state == State::Committed, 10).await;
            assert!(!task.is_finished());
            stop(task, libc::SIGTERM).await;
        })
        .await;
}
#[tokio::test]
#[ignore]
async fn interrupted_task_is_recovered_next_start() {
    if isolated("interrupted_task_is_recovered_next_start") {
        return;
    }
    let home = common::temp_dir("daemon-interrupt");
    let (relay, started, release) = Relay::held_mock();
    config(&home.0, &relay);
    let ledger = Ledger::open(home.0.join("ledger.db")).unwrap();
    let first = ledger.add_goal("first", "Say done").unwrap();
    let second = ledger.add_goal("second", "Say done").unwrap();
    tokio::task::LocalSet::new()
        .run_until(async {
            let path = home.0.clone();
            let task =
                tokio::task::spawn_local(async move { daemon::run(&path, opts(false)).await });
            until(|| started.try_recv().is_ok(), 10).await;
            stop(task, libc::SIGINT).await;
            assert_eq!(ledger.get(first).unwrap().state, State::Executing);
            assert_eq!(ledger.get(second).unwrap().state, State::Queued);
            assert!(children().is_empty());
            release.send(()).unwrap();
        })
        .await;
    assert_eq!(relay.finish().len(), 1);
    let next = Relay::mock();
    config(&home.0, &next);
    tokio::time::timeout(Duration::from_secs(10), daemon::run(&home.0, opts(true)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ledger.get(first).unwrap().state, State::Committed);
    assert_eq!(ledger.get(second).unwrap().state, State::Committed);
}
