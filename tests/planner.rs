mod common;
use iksa::{
    action::Action,
    audit,
    cas::Cas,
    config::{Config, ModelConfig},
    engine::Engine,
    gate::Risk,
    ledger::{Handoff, Ledger, State},
};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

fn isolated(name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    if std::env::var("IKSA_PLANNER_TEST_CHILD").as_deref() == Ok(name) {
        return false;
    }
    let fake = common::temp_dir("planner-keychain");
    std::fs::create_dir(fake.0.join("iksa-secret")).unwrap();
    let path = fake.0.join("iksa-secret/pw");
    std::fs::write(&path, "S3cr3t-xyz").unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--ignored", "--nocapture"])
        .env("IKSA_PLANNER_TEST_CHILD", name)
        .env("ANTHROPIC_API_KEY", "test")
        .env("IKSA_MOCK_KEYCHAIN", "1")
        .env("IKSA_KEYCHAIN_FAKE", &fake.0)
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
#[derive(Clone)]
enum Turn {
    Pick(&'static str, &'static str, Value),
    Call(&'static str, Value),
    Parallel,
    Done(&'static str),
}
struct Mock {
    endpoint: String,
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Mock {
    fn new(turns: Vec<Turn>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, done) = (requests.clone(), stop.clone());
        let worker = thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(s) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("mock accept {e}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut headers = String::new();
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    headers.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                }
                let n = headers
                    .lines()
                    .find_map(|l| {
                        l.split_once(':')
                            .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                            .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                let mut body = vec![0; n];
                reader.read_exact(&mut body).unwrap();
                let request: Value = serde_json::from_slice(&body).unwrap();
                let user = request["messages"][0]["content"].as_str().unwrap();
                let mut seen = seen.lock().unwrap();
                let index = seen.len();
                let turn = &turns[index.min(turns.len() - 1)];
                let (tool, input) = match turn {
                    Turn::Pick(tool, label, input) => {
                        let line = user
                            .rsplit("Current page:\n")
                            .next()
                            .unwrap()
                            .lines()
                            .find(|l| l.starts_with('[') && l.contains(&format!("\"{label}\"")))
                            .expect("fixture element missing from observation");
                        let reference = line
                            .strip_prefix('[')
                            .unwrap()
                            .split_once(']')
                            .unwrap()
                            .0
                            .parse::<u32>()
                            .unwrap();
                        let mut input = input.clone();
                        input["ref"] = json!(reference);
                        (*tool, input)
                    }
                    Turn::Call(tool, input) => (*tool, input.clone()),
                    Turn::Done(answer) => ("done", json!({"answer":answer})),
                    Turn::Parallel => ("click", json!({"ref":999,"risk":"write"})),
                };
                let mut content = vec![
                    json!({"type":"text","text":"Choose the next step"}),
                    json!({"type":"tool_use","id":format!("t{index}"),"name":tool,"input":input}),
                ];
                if matches!(turn, Turn::Parallel) {
                    content.push(json!({"type":"tool_use","id":"extra","name":"done","input":{"answer":"must not act"}}));
                }
                let response=json!({"content":content,"stop_reason":"tool_use","usage":{"input_tokens":10,"output_tokens":5}}).to_string();
                seen.push(request);
                drop(seen);
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).unwrap();
            }
        });
        Self {
            endpoint,
            requests,
            stop,
            worker: Some(worker),
        }
    }
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let result = self.worker.take().unwrap().join();
        if !thread::panicking() {
            result.unwrap();
        }
    }
}
struct Fixture {
    e: Engine,
    l: Ledger,
    c: Cas,
    chrome: common::Chrome,
    url: String,
}
impl Fixture {
    fn new() -> Self {
        let mut chrome = common::Chrome::new(&[]);
        let path = &chrome.profile.0;
        let port=common::serve(&[("/form",common::Response{status:200,headers:vec![],hang:false,body:r#"<title>Form</title><h1>Form</h1><label for=q>Query</label><input id=q oninput="document.querySelector('#live').textContent=this.value"><button id=go onclick="document.querySelector('#out').textContent=document.querySelector('#q').value;document.querySelector('#count').textContent=String(Number(document.querySelector('#count').textContent)+1)">Go</button><h2 id=out></h2><div id=live></div><h3 id=count>0</h3>"#.into() }),
            ("/receipt",common::Response {status:200,headers:vec![],hang:false,body:"<h1>S3cr3t-xyz</h1><button id=go>Go</button>".into()}),
            ("/notes",common::Response {status:200,headers:vec![],hang:false,body:"<h1>Notes page</h1><input value=private-notes><h2 aria-label=Notes>private-notes</h2>".into()}),
        ]);
        Self {
            e: Engine::new(chrome.b.take().unwrap()),
            l: Ledger::open(path.join("ledger.db")).unwrap(),
            c: Cas::open(path.join("cas")).unwrap(),
            chrome,
            url: format!("http://127.0.0.1:{port}/form"),
        }
    }
    fn task(&self, goal: &str) -> i64 {
        let id = self.l.add_goal("goal", goal).unwrap();
        self.l
            .push_step(
                id,
                &Action::Navigate {
                    url: self.url.clone(),
                },
            )
            .unwrap();
        // Start on /form using M1's existing replay of an already completed navigation.
        self.l.set_cursor(id, 1).unwrap();
        id
    }
    fn cfg(&self, mock: &Mock) -> Config {
        let mut cfg = Config {
            auto_max: Risk::Write,
            ..Config::default()
        };
        cfg.model = Some(
            serde_json::from_value::<ModelConfig>(
                json!({"provider":"anthropic","endpoint":mock.endpoint,"name":"test-model"}),
            )
            .unwrap(),
        );
        cfg
    }
    async fn run(&self, id: i64, cfg: &Config) -> State {
        tokio::time::timeout(
            Duration::from_secs(25),
            self.e.run(&self.l, &self.c, cfg, id),
        )
        .await
        .expect("planner run exceeded fixture deadline")
        .unwrap()
    }
}
fn click(risk: &str) -> Turn {
    Turn::Pick("click", "Go", json!({"risk":risk}))
}
#[tokio::test]
#[ignore]
async fn goal_reaches_done() {
    if isolated("goal_reaches_done") {
        return;
    }
    let f = Fixture::new();
    let mock = Mock::new(vec![click("write"), Turn::Done("ok")]);
    let cfg = f.cfg(&mock);
    let id = f.task("Click Go, then finish");
    assert_eq!(f.run(id, &cfg).await, State::Committed);
    assert_eq!(mock.count(), 2);
    let records = f.l.records(id).unwrap();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .all(|r| r.thought.is_some() && r.model_request.is_some())
    );
    let report = audit::render(&f.l.get(id).unwrap(), &records, &f.c).unwrap();
    assert!(report.contains("Thought:"));
    assert!(report.contains("Model request:"));
    assert_eq!(records[1].outcome, "done: ok");
    let requests = mock.requests.lock().unwrap();
    for (rec, body) in records.iter().zip(requests.iter()) {
        let hash = blake3::Hash::from_hex(rec.model_request.as_ref().unwrap()).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&f.c.get(&hash).unwrap()).unwrap(),
            *body
        );
    }
}
#[tokio::test]
#[ignore]
async fn invalid_ref_is_fed_back() {
    if isolated("invalid_ref_is_fed_back") {
        return;
    }
    let f = Fixture::new();
    let mock = Mock::new(vec![
        Turn::Call("click", json!({"ref":999,"risk":"write"})),
        Turn::Done("ok"),
    ]);
    let cfg = f.cfg(&mock);
    let id = f.task("Finish");
    assert_eq!(f.run(id, &cfg).await, State::Committed);
    assert_eq!(mock.count(), 2);
    assert_eq!(f.l.get(id).unwrap().steps.len(), 1);
    assert!(
        mock.requests.lock().unwrap()[1]["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("invalid: no element [999]")
    );
}
#[tokio::test]
#[ignore]
async fn three_invalid_fails() {
    if isolated("three_invalid_fails") {
        return;
    }
    let f = Fixture::new();
    let mock = Mock::new(vec![
        Turn::Call("fly", json!({})),
        Turn::Parallel,
        Turn::Call("click", json!({"ref":999,"risk":"write"})),
    ]);
    let cfg = f.cfg(&mock);
    let id = f.task("Finish");
    assert_eq!(
        f.run(id, &cfg).await,
        State::Failed("model kept choosing invalid actions".into())
    );
    assert_eq!(mock.count(), 3);
    assert_eq!(f.l.get(id).unwrap().steps.len(), 1);
    let records = f.l.records(id).unwrap();
    assert_eq!(records.len(), 3);
    assert!(records.iter().all(|r| r.outcome.starts_with("invalid:")));
    assert!(records[0].outcome.contains("unknown tool"));
    assert!(records[1].outcome.contains("exactly one"));
}
#[tokio::test]
#[ignore]
async fn financial_click_needs_approval() {
    if isolated("financial_click_needs_approval") {
        return;
    }
    let f = Fixture::new();
    let mock = Mock::new(vec![click("financial"), Turn::Done("ok")]);
    let cfg = f.cfg(&mock);
    let id = f.task("Click Go");
    assert_eq!(
        f.run(id, &cfg).await,
        State::AwaitingHandoff(Handoff::Approval)
    );
    assert_eq!(mock.count(), 1);
    let task = f.l.get(id).unwrap();
    assert_eq!(task.cursor, 1);
    assert!(
        matches!(&task.steps[1],Action::Click{selector,risk:Risk::Financial}if selector=="#go")
    );
    let requests = f.l.records(id).unwrap();
    assert!(requests.iter().any(|r| r.model_request.is_some()));
    audit::gc(&f.l, &f.c, 30, audit::now_ms()).unwrap();
    for r in &requests {
        if let Some(hash) = &r.model_request {
            assert!(f.c.get(&blake3::Hash::from_hex(hash).unwrap()).is_ok());
        }
    }
    f.l.approve(id).unwrap();
    assert_eq!(f.run(id, &cfg).await, State::Committed);
    assert_eq!(mock.count(), 2);
    assert!(
        f.l.records(id)
            .unwrap()
            .iter()
            .any(|r| r.decision == "approved")
    );
}
#[tokio::test]
#[ignore]
async fn model_cannot_lower_risk() {
    if isolated("model_cannot_lower_risk") {
        return;
    }
    let f = Fixture::new();
    let mock = Mock::new(vec![Turn::Pick(
        "type",
        "Query",
        json!({"text":"x","secret":false,"risk":"read"}),
    )]);
    let mut cfg = f.cfg(&mock);
    cfg.auto_max = Risk::Read;
    let id = f.task("Type x");
    assert_eq!(
        f.run(id, &cfg).await,
        State::AwaitingHandoff(Handoff::Approval)
    );
    assert_eq!(mock.count(), 1);
    assert!(matches!(
        f.l.get(id).unwrap().steps[1],
        Action::Type {
            risk: Risk::Write,
            ..
        }
    ));
}
#[tokio::test]
#[ignore]
async fn never_send_domain_never_reaches_model() {
    if isolated("never_send_domain_never_reaches_model") {
        return;
    }
    let f = Fixture::new();
    let mock = Mock::new(vec![Turn::Done("must not receive")]);
    let mut cfg = f.cfg(&mock);
    cfg.never_send = vec!["127.0.0.1".into()];
    let id = f.task("Read the page");
    assert_eq!(
        f.run(id, &cfg).await,
        State::AwaitingHandoff(Handoff::Approval)
    );
    assert_eq!(mock.count(), 0);
    let records = f.l.records(id).unwrap();
    assert_eq!(
        records.last().unwrap().outcome,
        "blocked: 127.0.0.1 is on never_send"
    );
    assert!(records.iter().all(|r| r.model_request.is_none()));
    assert!(f.c.all().unwrap().is_empty());
}
#[tokio::test]
#[ignore]
async fn secret_placeholder_never_leaves() {
    if isolated("secret_placeholder_never_leaves") {
        return;
    }
    let f = Fixture::new();
    let mock = Mock::new(vec![
        Turn::Pick(
            "type",
            "Query",
            json!({"text":"{{secret:pw}}","secret":true,"risk":"write"}),
        ),
        click("write"),
        Turn::Pick("extract", "••••", json!({})),
        Turn::Done("S3cr3t-xyz"),
    ]);
    let cfg = f.cfg(&mock);
    let id = f.task("Use {{secret:pw}} in Query, click Go, and read the result");
    assert_eq!(f.run(id, &cfg).await, State::Committed);
    assert_eq!(mock.count(), 4);
    let records = f.l.records(id).unwrap();
    assert_eq!(records[2].outcome, "••••");
    assert_eq!(records[3].outcome, "done: ••••");
    let task = f.l.get(id).unwrap();
    assert!(matches!(&task.steps[1],Action::Type{text,secret:true,..}if text=="{{secret:pw}}"));
    let secret = b"S3cr3t-xyz";
    let contains = |bytes: &[u8]| bytes.windows(secret.len()).any(|w| w == secret);
    assert!(!contains(
        &serde_json::to_vec(&*mock.requests.lock().unwrap()).unwrap()
    ));
    assert!(!contains(&serde_json::to_vec(&records).unwrap()));
    assert!(!contains(
        audit::render(&task, &records, &f.c).unwrap().as_bytes()
    ));
    for hash in f.c.all().unwrap() {
        assert!(!contains(&f.c.get(&hash).unwrap()), "secret in CAS");
    }
    for name in ["ledger.db", "ledger.db-wal", "ledger.db-shm"] {
        let path = f.chrome.profile.0.join(name);
        if path.exists() {
            assert!(!contains(&std::fs::read(path).unwrap()), "secret in {name}");
        }
    }
}
#[tokio::test]
#[ignore]
async fn step_budget_ends_task() {
    if isolated("step_budget_ends_task") {
        return;
    }
    let f = Fixture::new();
    let mock = Mock::new(vec![click("write")]);
    let mut cfg = f.cfg(&mock);
    cfg.model.as_mut().unwrap().max_steps = 2;
    let id = f.task("Keep clicking Go");
    assert_eq!(
        f.run(id, &cfg).await,
        State::Failed("step budget exhausted".into())
    );
    assert_eq!(mock.count(), 2);
    assert_eq!(f.l.get(id).unwrap().steps.len(), 3);
}

#[tokio::test]
#[ignore]
async fn executable_navigation_is_rejected() {
    if isolated("executable_navigation_is_rejected") {
        return;
    }
    let f = Fixture::new();
    let mock = Mock::new(vec![
        Turn::Call(
            "navigate",
            json!({"url":"javascript:document.querySelector('#go').click()"}),
        ),
        Turn::Done("ok"),
    ]);
    let cfg = f.cfg(&mock);
    let id = f.task("Read the page without clicking");
    assert_eq!(f.run(id, &cfg).await, State::Committed);
    assert_eq!(mock.count(), 2);
    assert_eq!(f.l.get(id).unwrap().steps.len(), 1);
    assert!(f.l.records(id).unwrap()[0].outcome.starts_with("invalid:"));
    assert!(
        mock.requests.lock().unwrap()[1]["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("heading \"0\"")
    );
}

#[tokio::test]
#[ignore]
async fn resumed_secret_before_last_navigation_stays_private() {
    if isolated("resumed_secret_before_last_navigation_stays_private") {
        return;
    }
    let f = Fixture::new();
    let receipt = f.url.replace("/form", "/receipt");
    let mock = Mock::new(vec![
        Turn::Pick(
            "type",
            "Query",
            json!({"text":"{{secret:pw}}","secret":true,"risk":"write"}),
        ),
        Turn::Call("navigate", json!({"url":receipt})),
        click("financial"),
        Turn::Done("ok"),
    ]);
    let cfg = f.cfg(&mock);
    let id = f.task("Use {{secret:pw}}, open the receipt, and click Go");
    assert_eq!(
        f.run(id, &cfg).await,
        State::AwaitingHandoff(Handoff::Approval)
    );
    assert!(
        !serde_json::to_string(&*mock.requests.lock().unwrap())
            .unwrap()
            .contains("S3cr3t-xyz")
    );
    f.l.approve(id).unwrap();
    assert_eq!(f.run(id, &cfg).await, State::Committed);
    assert_eq!(mock.count(), 4);
    assert!(
        !serde_json::to_string(&*mock.requests.lock().unwrap())
            .unwrap()
            .contains("S3cr3t-xyz"),
        "secret leaked after resume"
    );
    for hash in f.c.all().unwrap() {
        assert!(
            !String::from_utf8_lossy(&f.c.get(&hash).unwrap()).contains("S3cr3t-xyz"),
            "secret in CAS after resume"
        );
    }
}
#[tokio::test]
#[ignore]
async fn extracted_form_values_are_not_sent_in_logs() {
    if isolated("extracted_form_values_are_not_sent_in_logs") {
        return;
    }
    let f = Fixture::new();
    let mock = Mock::new(vec![
        Turn::Pick("extract", "Notes", json!({})),
        Turn::Done("ok"),
    ]);
    let cfg = f.cfg(&mock);
    let id = f.l.add_goal("notes", "Read the notes").unwrap();
    f.l.push_step(
        id,
        &Action::Navigate {
            url: f.url.replace("/form", "/notes"),
        },
    )
    .unwrap();
    f.l.set_cursor(id, 1).unwrap();
    assert_eq!(f.run(id, &cfg).await, State::Committed);
    assert_eq!(mock.count(), 2);
    assert!(
        !serde_json::to_string(&*mock.requests.lock().unwrap())
            .unwrap()
            .contains("private-notes"),
        "form value re-entered the model request through the log"
    );
}
#[tokio::test]
#[ignore]
async fn challenge_resume_keeps_model_metadata() {
    if isolated("challenge_resume_keeps_model_metadata") {
        return;
    }
    let f = Fixture::new();
    let mock = Mock::new(vec![Turn::Done("ok")]);
    let cfg = f.cfg(&mock);
    let id = f.task("Read Form");
    let action = Action::Extract {
        selector: "h1".into(),
    };
    f.l.push_step(id, &action).unwrap();
    let request =
        f.c.put(br#"{"messages":[],"model":"test-model"}"#)
            .unwrap()
            .to_hex()
            .to_string();
    let mut rec = audit::StepRecord::new(1, &action, "auto".into(), None, "Form".into());
    rec.thought = Some("Read Form".into());
    rec.model_request = Some(request.clone());
    f.l.transition(id, State::Executing).unwrap();
    f.l.record_step(id, &rec).unwrap();
    f.l.transition(
        id,
        State::AwaitingHandoff(Handoff::Challenge(
            iksa::challenge::ChallengeKind::Turnstile,
        )),
    )
    .unwrap();
    f.l.resume(id).unwrap();
    assert_eq!(f.run(id, &cfg).await, State::Committed);
    let records = f.l.records(id).unwrap();
    assert_eq!(records[1].thought.as_deref(), Some("Read Form"));
    assert_eq!(records[1].model_request.as_deref(), Some(request.as_str()));
    assert_eq!(mock.count(), 1);
}

#[tokio::test]
#[ignore]
async fn repeated_request_hashes_still_count_toward_budget() {
    if isolated("repeated_request_hashes_still_count_toward_budget") {
        return;
    }
    let f = Fixture::new();
    let mock = Mock::new(vec![Turn::Done("must not be called")]);
    let mut cfg = f.cfg(&mock);
    cfg.model.as_mut().unwrap().max_steps = 2;
    let id = f.task("Finish");
    // Privacy redaction can make distinct turns produce identical request bodies.
    // Two persisted invalid-call records still represent two spent model turns.
    let request =
        f.c.put(br#"{"messages":[],"model":"test-model"}"#)
            .unwrap()
            .to_hex()
            .to_string();
    for _ in 0..2 {
        let mut record = audit::StepRecord::new(
            1,
            &Action::Extract {
                selector: "html".into(),
            },
            "planner".into(),
            None,
            "invalid: incomplete call".into(),
        );
        record.model_request = Some(request.clone());
        f.l.record_step(id, &record).unwrap();
    }
    assert_eq!(
        f.run(id, &cfg).await,
        State::Failed("step budget exhausted".into())
    );
    assert_eq!(mock.count(), 0);
}
