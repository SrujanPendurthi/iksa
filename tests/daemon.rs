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
    print!("{}", String::from_utf8_lossy(&out.stdout));
    eprint!("{}", String::from_utf8_lossy(&out.stderr));
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
    let mut one_worker = Config::load(&home.0).unwrap();
    one_worker.max_agents = 1;
    std::fs::write(
        home.0.join("config.json"),
        serde_json::to_vec(&one_worker).unwrap(),
    )
    .unwrap();
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

use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Instant,
};
#[derive(Default)]
struct Metrics {
    active: usize,
    peak: usize,
    starts: Vec<(String, Instant)>,
    finishes: Vec<(String, Instant)>,
    turns: HashMap<String, usize>,
}
struct ConcurrentModel {
    endpoint: String,
    metrics: Arc<Mutex<Metrics>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl ConcurrentModel {
    fn new(isolate: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let metrics = Arc::new(Mutex::new(Metrics::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, done) = (metrics.clone(), stop.clone());
        let worker = thread::spawn(move || {
            let mut handlers = Vec::new();
            loop {
                let (mut stream, _) = listener.accept().unwrap();
                if done.load(Ordering::Relaxed) {
                    break;
                }
                let seen = seen.clone();
                handlers.push(thread::spawn(move || {
                    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    let mut reader = BufReader::new(&mut stream);
                    let mut length = None;
                    loop {
                        let mut line = String::new();
                        assert!(reader.read_line(&mut line).unwrap() > 0);
                        if line == "\r\n" { break; }
                        if let Some((key, value)) = line.split_once(':') {
                            if key.eq_ignore_ascii_case("content-length") { length = Some(value.trim().parse::<usize>().unwrap()); }
                        }
                    }
                    let mut bytes = vec![0; length.unwrap()];
                    reader.read_exact(&mut bytes).unwrap();
                    let body: Value = serde_json::from_slice(&bytes).unwrap();
                    let user = body["messages"][0]["content"].as_str().unwrap();
                    let goal = user.strip_prefix("Goal: ").unwrap().split("\n\nSteps so far:").next().unwrap().to_owned();
                    let turn = {
                        let mut m = seen.lock().unwrap();
                        let turn = *m.turns.entry(goal.clone()).or_default();
                        *m.turns.get_mut(&goal).unwrap() += 1;
                        m.active += 1;
                        m.peak = m.peak.max(m.active);
                        m.starts.push((goal.clone(), Instant::now()));
                        turn
                    };
                    let (tool, input) = if isolate && turn < 2 {
                        let role = if turn == 0 { "button" } else { "heading" };
                        let line = user.rsplit("Current page:\n").next().unwrap().lines()
                            .find(|l| l.starts_with('[') && l.contains(&format!("] {role} "))).expect("fixture element missing");
                        let reference: u32 = line.strip_prefix('[').unwrap().split_once(']').unwrap().0.parse().unwrap();
                        if turn == 0 { ("click", json!({"ref":reference,"risk":"write"})) } else { ("extract", json!({"ref":reference})) }
                    } else { ("done", json!({"answer":"ok"})) };
                    thread::sleep(Duration::from_secs(1));
                    let response = json!({"content":[{"type":"tool_use","id":"offline","name":tool,"input":input}],"stop_reason":"tool_use","usage":{"input_tokens":10,"output_tokens":2}}).to_string();
                    {
                        let mut m = seen.lock().unwrap();
                        m.active -= 1;
                        m.finishes.push((goal, Instant::now()));
                    }
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len());
                }));
            }
            for handler in handlers {
                handler.join().unwrap();
            }
        });
        Self {
            endpoint,
            metrics,
            stop,
            worker: Some(worker),
        }
    }
    fn configure(&self, home: &std::path::Path, max_agents: u32) -> Config {
        let cfg = Config {
            model: Some(
                serde_json::from_value(
                    json!({"provider":"anthropic","endpoint":self.endpoint,"name":"offline-model"}),
                )
                .unwrap(),
            ),
            max_agents,
            auto_max: iksa::gate::Risk::Write,
            ..Config::default()
        };
        std::fs::write(home.join("config.json"), serde_json::to_vec(&cfg).unwrap()).unwrap();
        cfg
    }
}
impl Drop for ConcurrentModel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.endpoint.strip_prefix("http://").unwrap());
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
    }
}
#[tokio::test]
#[ignore]
async fn max_agents_bounds_concurrency() {
    if isolated("max_agents_bounds_concurrency") {
        return;
    }
    for max in [3, 1] {
        let home = common::temp_dir("daemon-concurrency");
        let model = ConcurrentModel::new(false);
        model.configure(&home.0, max);
        let ledger = Ledger::open(home.0.join("ledger.db")).unwrap();
        for n in 0..4 {
            ledger
                .add_goal(&format!("goal {n}"), &format!("goal {n}"))
                .unwrap();
        }
        let start = Instant::now();
        tokio::time::timeout(Duration::from_secs(12), daemon::run(&home.0, opts(true)))
            .await
            .unwrap()
            .unwrap();
        let elapsed = start.elapsed();
        let m = model.metrics.lock().unwrap();
        assert_eq!(m.peak, max as usize);
        assert_eq!(m.starts.len(), 4, "a goal was dispatched more than once");
        assert!(
            ledger
                .list()
                .unwrap()
                .iter()
                .all(|t| t.state == State::Committed)
        );
        if max == 3 {
            assert!(
                elapsed < Duration::from_millis(3500),
                "four goals took {elapsed:?}"
            );
        }
        println!(
            "max_agents={max}, peak={}, four-goal wall={elapsed:?}",
            m.peak
        );
    }
}
#[tokio::test]
#[ignore]
async fn human_task_gets_reserved_slot() {
    if isolated("human_task_gets_reserved_slot") {
        return;
    }
    let home = common::temp_dir("daemon-human-slot");
    let model = ConcurrentModel::new(false);
    model.configure(&home.0, 1);
    let ledger = Ledger::open(home.0.join("ledger.db")).unwrap();
    ledger
        .add_schedule("every 5m", "background one", true, 0)
        .unwrap();
    ledger
        .add_schedule("every 5m", "background two", true, 0)
        .unwrap();
    ledger.fire_due(0, |_, _| 300_000).unwrap();
    // Keep schedules from being due again during the deterministic worker test.
    rusqlite::Connection::open(home.0.join("ledger.db"))
        .unwrap()
        .execute(
            "UPDATE schedules SET next_ms=?1",
            [iksa::audit::now_ms() + 300_000],
        )
        .unwrap();
    ledger.add_goal("human ask", "human ask").unwrap();
    tokio::time::timeout(Duration::from_secs(12), daemon::run(&home.0, opts(true)))
        .await
        .unwrap()
        .unwrap();
    let m = model.metrics.lock().unwrap();
    assert_eq!(m.peak, 2);
    assert!(m.starts[..2].iter().any(|(goal, _)| goal == "human ask"));
    let first_finished = m
        .finishes
        .iter()
        .find(|(goal, _)| goal == "background one")
        .unwrap()
        .1;
    let second_started = m
        .starts
        .iter()
        .find(|(goal, _)| goal == "background two")
        .unwrap()
        .1;
    assert!(
        second_started >= first_finished,
        "second scheduled task stole the reserved slot"
    );
    assert!(
        ledger
            .list()
            .unwrap()
            .iter()
            .all(|t| t.state == State::Committed)
    );
}
#[tokio::test]
#[ignore]
async fn concurrent_tasks_isolated() {
    if isolated("concurrent_tasks_isolated") {
        return;
    }
    let home = common::temp_dir("daemon-isolated");
    let model = ConcurrentModel::new(true);
    model.configure(&home.0, 2);
    let page = "<h1>ready</h1><button onclick=\"document.querySelector('h1').innerText=document.cookie+'|'+localStorage.getItem('owner')\">Read</button><script>const owner=new URLSearchParams(location.search).get('owner');document.cookie='owner='+owner;localStorage.setItem('owner',owner);</script>";
    let response = common::Response {
        status: 200,
        headers: vec![],
        body: page.into(),
        hang: false,
    };
    let port = common::serve(&[
        ("/isolate?owner=one", response.clone()),
        ("/isolate?owner=two", response),
    ]);
    let ledger = Ledger::open(home.0.join("ledger.db")).unwrap();
    let mut ids = Vec::new();
    for owner in ["one", "two"] {
        let id = ledger.add_goal(owner, owner).unwrap();
        ledger
            .push_step(
                id,
                &iksa::action::Action::Navigate {
                    url: format!("http://127.0.0.1:{port}/isolate?owner={owner}"),
                },
            )
            .unwrap();
        ids.push((id, owner));
    }
    tokio::time::timeout(Duration::from_secs(15), daemon::run(&home.0, opts(true)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(model.metrics.lock().unwrap().peak, 2);
    for (id, owner) in ids {
        assert_eq!(ledger.get(id).unwrap().state, State::Committed);
        let records = ledger.records(id).unwrap();
        let extracted = records
            .iter()
            .find(|r| {
                matches!(r.action, iksa::action::Action::Extract { .. }) && r.decision != "planner"
            })
            .unwrap();
        assert_eq!(extracted.outcome, format!("owner={owner}|{owner}"));
    }
}
#[tokio::test]
#[ignore]
async fn cleanup_failure_preserves_other_worker_borrows() {
    if isolated("cleanup_failure_preserves_other_worker_borrows") {
        return;
    }
    let home = common::temp_dir("daemon-broken-engine");
    let (relay, started, release) = Relay::held_mock();
    config(&home.0, &relay);
    let cfg = Config::load(&home.0).unwrap();
    let ledger = Ledger::open(home.0.join("ledger.db")).unwrap();
    let cas = iksa::cas::Cas::open(home.0.join("cas")).unwrap();
    let first = ledger.add_goal("held", "held").unwrap();
    let second = ledger.add_goal("broken", "broken").unwrap();
    let third = ledger.add_goal("not started", "not started").unwrap();
    let mut chrome = common::Chrome::new(&[]);
    let browser = chrome.b.take().unwrap();
    let pid = browser.pid();
    let engine = std::rc::Rc::new(iksa::engine::Engine::new(browser));
    tokio::task::LocalSet::new()
        .run_until(async {
            let active = engine.clone();
            let path = home.0.clone();
            let cfg = cfg.clone();
            let worker = tokio::task::spawn_local(async move {
                let ledger = Ledger::open(path.join("ledger.db")).unwrap();
                let cas = iksa::cas::Cas::open(path.join("cas")).unwrap();
                active.run(&ledger, &cas, &cfg, first).await
            });
            until(|| started.try_recv().is_ok(), 10).await;
            assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGKILL) }, 0);
            assert!(matches!(
                engine
                    .run(&ledger, &cas, &Config::load(&home.0).unwrap(), second)
                    .await
                    .unwrap(),
                State::Failed(_)
            ));
            assert!(!engine.is_available());
            assert_eq!(
                engine
                    .run(&ledger, &cas, &Config::load(&home.0).unwrap(), third)
                    .await
                    .unwrap_err()
                    .to_string(),
                "browser unavailable after cleanup timeout"
            );
            assert_eq!(ledger.get(third).unwrap().state, State::Queued);
            release.send(()).unwrap();
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(3), worker)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap(),
                State::Committed
            );
        })
        .await;
    assert_eq!(std::rc::Rc::strong_count(&engine), 1);
}
#[tokio::test]
#[ignore]
async fn daemon_relaunches_after_workers_release_broken_engine() {
    if isolated("daemon_relaunches_after_workers_release_broken_engine") {
        return;
    }
    let home = common::temp_dir("daemon-relaunch");
    let model = ConcurrentModel::new(false);
    model.configure(&home.0, 2);
    let ledger = Ledger::open(home.0.join("ledger.db")).unwrap();
    for n in 0..3 {
        ledger
            .add_goal(&format!("goal {n}"), &format!("goal {n}"))
            .unwrap();
    }
    tokio::task::LocalSet::new()
        .run_until(async {
            let path = home.0.clone();
            let task =
                tokio::task::spawn_local(async move { daemon::run(&path, opts(true)).await });
            until(|| model.metrics.lock().unwrap().starts.len() == 2, 10).await;
            let old_browser = children()[0];
            assert_eq!(unsafe { libc::kill(old_browser as i32, libc::SIGKILL) }, 0);
            until(|| model.metrics.lock().unwrap().starts.len() == 3, 10).await;
            assert_ne!(children()[0], old_browser);
            tokio::time::timeout(Duration::from_secs(10), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        })
        .await;
    let m = model.metrics.lock().unwrap();
    assert_eq!(m.starts.len(), 3);
    let third_start = m.starts.iter().find(|(g, _)| g == "goal 2").unwrap().1;
    for goal in ["goal 0", "goal 1"] {
        assert!(third_start >= m.finishes.iter().find(|(g, _)| g == goal).unwrap().1);
    }
    assert!(
        ledger
            .list()
            .unwrap()
            .iter()
            .all(|t| t.state == State::Committed)
    );
}
