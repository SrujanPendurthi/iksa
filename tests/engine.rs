mod common;
use iksa::{
    action::Action,
    audit,
    cas::Cas,
    challenge::ChallengeKind,
    config::{Config, SearchProvider},
    engine::Engine,
    gate::Risk,
    ledger::{Handoff, Ledger, State},
};
use std::time::Duration;
struct EngineFixture {
    e: Engine,
    l: Ledger,
    c: Cas,
    _chrome: common::Chrome,
    port: u16,
}
impl EngineFixture {
    fn new() -> Self {
        let mut chrome = common::Chrome::new(&[]);
        let p = &chrome.profile.0;
        let r = |body: &str| common::Response {
            status: 200,
            headers: vec![],
            body: body.into(),
            hang: false,
        };
        let form = "<h1>Form</h1><input id=q oninput=\"document.querySelector('#live').innerText=this.value\"><button id=go onclick=\"document.querySelector('#out').innerText=document.querySelector('#q').value\">Go</button><div id=out></div><div id=live></div>";
        let widget =
            "<h1>Widget</h1><div class=\"cf-turnstile\"></div><input name=cf-turnstile-response>";
        let auto = format!(
            "{widget}<script>setTimeout(()=>document.querySelector('input').value='tok',1000)</script>"
        );
        let recap = "<h1>Widget</h1><script src=\"https://www.google.com/recaptcha/api.js?render=k\" async></script><textarea name=g-recaptcha-response></textarea><script>setTimeout(()=>document.querySelector('textarea').value='tok',1000)</script>";
        let port = common::serve(&[
            ("/form", r(form)),
            ("/search?q=a%20b", r("<h1>a b</h1>")),
            ("/turnstile", r(widget)),
            ("/turnstile-auto", r(&auto)),
            (
                "/recaptcha-v3",
                common::Response {
                    headers: vec![(
                        "Content-Security-Policy".into(),
                        "script-src 'unsafe-inline'".into(),
                    )],
                    ..r(recap)
                },
            ),
            (
                "/cf",
                common::Response {
                    status: 403,
                    headers: vec![("cf-mitigated".into(), "challenge".into())],
                    ..r("<h1>CF</h1>")
                },
            ),
            (
                "/slow",
                common::Response {
                    hang: true,
                    ..r("")
                },
            ),
            (
                "/cookie",
                common::Response {
                    headers: vec![("Set-Cookie".into(), "task=A; Path=/".into())],
                    ..r("<h1>Cookie</h1>")
                },
            ),
            (
                "/cookies",
                r(
                    "<h1></h1><script>document.querySelector('h1').innerText=document.cookie</script>",
                ),
            ),
        ]);
        let e = Engine::new(chrome.b.take().unwrap());
        let l = Ledger::open(p.join("ledger.db")).unwrap();
        let c = Cas::open(p.join("cas")).unwrap();
        Self {
            e,
            l,
            c,
            _chrome: chrome,
            port,
        }
    }
    fn nav(&self, path: &str) -> Action {
        Action::Navigate {
            url: format!("http://127.0.0.1:{}{path}", self.port),
        }
    }
    async fn run(&self, steps: &[Action], cfg: &Config) -> (i64, State) {
        let id = self.l.add("test", steps).unwrap();
        (id, self.e.run(&self.l, &self.c, cfg, id).await.unwrap())
    }
    fn outcome(&self, id: i64) -> String {
        self.l.records(id).unwrap().last().unwrap().outcome.clone()
    }
}
fn extract(s: &str) -> Action {
    Action::Extract { selector: s.into() }
}
fn typed() -> Action {
    Action::Type {
        selector: "#q".into(),
        text: "hi".into(),
        secret: false,
        risk: Risk::Write,
    }
}
#[tokio::test]
#[ignore]
async fn runs_read_task_to_committed() {
    let f = EngineFixture::new();
    let (id, s) = f
        .run(&[f.nav("/form"), extract("h1")], &Config::default())
        .await;
    assert_eq!(s, State::Committed);
    assert_eq!(f.outcome(id), "Form");
    assert!(f.l.records(id).unwrap().iter().all(|r| r.after.is_some()));
}
#[tokio::test]
#[ignore]
async fn search_uses_configured_provider() {
    let f = EngineFixture::new();
    let cfg = Config {
        search: "Local".into(),
        providers: vec![SearchProvider {
            name: "Local".into(),
            url_template: format!("http://127.0.0.1:{}/search?q={{q}}", f.port),
        }],
        ..Config::default()
    };
    let (id, s) = f
        .run(
            &[
                Action::Search {
                    query: "a b".into(),
                },
                extract("h1"),
            ],
            &cfg,
        )
        .await;
    assert_eq!(s, State::Committed);
    assert_eq!(f.outcome(id), "a b");
}
#[tokio::test]
#[ignore]
async fn write_step_waits_for_approval() {
    let f = EngineFixture::new();
    let cfg = Config::default();
    let (id, s) = f
        .run(&[f.nav("/form"), typed(), extract("#live")], &cfg)
        .await;
    assert_eq!(s, State::AwaitingHandoff(Handoff::Approval));
    assert_eq!(f.l.get(id).unwrap().cursor, 1);
    f.l.approve(id).unwrap();
    assert_eq!(f.l.get(id).unwrap().approved_step, Some(1));
    assert_eq!(
        f.e.run(&f.l, &f.c, &cfg, id).await.unwrap(),
        State::Committed
    );
    assert_eq!(f.outcome(id), "hi");
    assert_eq!(f.l.records(id).unwrap()[1].decision, "approved");
}
#[tokio::test]
#[ignore]
async fn click_uses_real_input() {
    let f = EngineFixture::new();
    let (id, s) = f
        .run(
            &[
                f.nav("/form"),
                typed(),
                Action::Click {
                    selector: "#go".into(),
                    risk: Risk::Write,
                },
                extract("#out"),
            ],
            &Config {
                auto_max: Risk::Write,
                ..Config::default()
            },
        )
        .await;
    assert_eq!(s, State::Committed);
    assert_eq!(f.outcome(id), "hi");
}
async fn challenge(path: &str, k: ChallengeKind) {
    let f = EngineFixture::new();
    let (id, s) = f.run(&[f.nav(path)], &Config::default()).await;
    assert_eq!(s, State::AwaitingHandoff(Handoff::Challenge(k)));
    assert_eq!(f.l.get(id).unwrap().cursor, 0);
    assert_eq!(f.l.records(id).unwrap().len(), 1);
}
#[tokio::test]
#[ignore]
async fn challenge_pauses() {
    challenge("/cf", ChallengeKind::CloudflareChallenge).await;
}
#[tokio::test]
#[ignore]
async fn turnstile_blocks_without_token() {
    challenge("/turnstile", ChallengeKind::Turnstile).await;
}
async fn auto(path: &str) {
    let f = EngineFixture::new();
    let (id, s) = f
        .run(&[f.nav(path), extract("h1")], &Config::default())
        .await;
    assert_eq!(s, State::Committed);
    assert_eq!(f.outcome(id), "Widget");
}
#[tokio::test]
#[ignore]
async fn turnstile_auto_pass_continues() {
    auto("/turnstile-auto").await;
}
#[tokio::test]
#[ignore]
async fn recaptcha_v3_auto_pass_continues() {
    auto("/recaptcha-v3").await;
}
#[tokio::test]
#[ignore]
async fn hung_page_fails_with_timeout() {
    let f = EngineFixture::new();
    for steps in [
        vec![f.nav("/slow")],
        vec![f.nav("/form"), extract("#absent")],
    ] {
        let id = f.l.add("timeout", &steps).unwrap();
        let start = std::time::Instant::now();
        let s =
            f.e.run_with_timeout(&f.l, &f.c, &Config::default(), id, Duration::from_secs(2))
                .await
                .unwrap();
        assert!(matches!(s,State::Failed(ref s) if s.contains("timeout")));
        assert!(start.elapsed() < Duration::from_secs(10));
        assert!(
            !audit::render(&f.l.get(id).unwrap(), &f.l.records(id).unwrap(), &f.c)
                .unwrap()
                .is_empty()
        );
    }
}
#[tokio::test]
#[ignore]
async fn contexts_are_isolated() {
    let f = EngineFixture::new();
    let (a, s) = f
        .run(
            &[f.nav("/cookie"), f.nav("/cookies"), extract("h1")],
            &Config::default(),
        )
        .await;
    assert_eq!(s, State::Committed);
    assert_eq!(f.outcome(a), "task=A");
    let (b, s) = f
        .run(&[f.nav("/cookies"), extract("h1")], &Config::default())
        .await;
    assert_eq!(s, State::Committed);
    assert_eq!(f.outcome(b), "");
}
#[tokio::test]
#[ignore]
async fn approval_resume_replays_typed_fields() {
    let f = EngineFixture::new();
    let cfg = Config {
        auto_max: Risk::Write,
        ..Config::default()
    };
    let (id, s) = f
        .run(
            &[
                f.nav("/form"),
                typed(),
                Action::Click {
                    selector: "#go".into(),
                    risk: Risk::Financial,
                },
                extract("#out"),
            ],
            &cfg,
        )
        .await;
    assert_eq!(s, State::AwaitingHandoff(Handoff::Approval));
    assert_eq!(f.l.get(id).unwrap().cursor, 2);
    f.l.approve(id).unwrap();
    assert_eq!(
        f.e.run(&f.l, &f.c, &cfg, id).await.unwrap(),
        State::Committed
    );
    assert_eq!(f.outcome(id), "hi");
}
#[tokio::test]
#[ignore]
async fn resume_with_click_before_cursor_fails_safely() {
    let f = EngineFixture::new();
    let cfg = Config {
        auto_max: Risk::Write,
        ..Config::default()
    };
    let (id, s) = f
        .run(
            &[
                f.nav("/form"),
                Action::Click {
                    selector: "#go".into(),
                    risk: Risk::Write,
                },
                Action::Type {
                    selector: "#q".into(),
                    text: "x".into(),
                    secret: false,
                    risk: Risk::Financial,
                },
            ],
            &cfg,
        )
        .await;
    assert_eq!(s, State::AwaitingHandoff(Handoff::Approval));
    assert_eq!(f.l.get(id).unwrap().cursor, 2);
    f.l.approve(id).unwrap();
    let s = f.e.run(&f.l, &f.c, &cfg, id).await.unwrap();
    assert!(matches!(s,State::Failed(ref m) if m.contains("would repeat a click")));
    let r = f.l.records(id).unwrap();
    assert_eq!(r.last().unwrap().decision, "approved");
}
#[tokio::test]
#[ignore]
async fn secret_never_in_snapshots_or_outcomes() {
    let f = EngineFixture::new();
    let cfg = Config {
        auto_max: Risk::Write,
        ..Config::default()
    };
    let (id, s) = f
        .run(
            &[
                f.nav("/form"),
                Action::Type {
                    selector: "#q".into(),
                    text: "hunter2".into(),
                    secret: true,
                    risk: Risk::Write,
                },
                Action::Click {
                    selector: "#go".into(),
                    risk: Risk::Write,
                },
                extract("#out"),
                f.nav("/form"),
                extract("h1"),
            ],
            &cfg,
        )
        .await;
    assert_eq!(s, State::Committed);
    for hash in f.c.all().unwrap() {
        assert!(!String::from_utf8_lossy(&f.c.get(&hash).unwrap()).contains("hunter2"));
    }
    let r = f.l.records(id).unwrap();
    assert!(!serde_json::to_string(&r).unwrap().contains("hunter2"));
    assert_eq!(r[3].outcome, "••••");
    assert!(r[1..4].iter().all(|r| r.after.is_none()));
    assert!(r[4..6].iter().all(|r| r.after.is_some()));

    // Fragment navigation keeps the same document and its mirrored secret.
    let secret = Action::Type {
        selector: "#q".into(),
        text: "hunter2".into(),
        secret: true,
        risk: Risk::Write,
    };
    let (fragment_id, state) = f
        .run(
            &[
                f.nav("/form"),
                secret.clone(),
                f.nav("/form#x"),
                extract("#live"),
                f.nav("/form"),
                extract("h1"),
            ],
            &cfg,
        )
        .await;
    assert_eq!(state, State::Committed);
    let records = f.l.records(fragment_id).unwrap();
    assert_eq!(records[3].outcome, "••••");
    assert!(
        records[1..4].iter().all(|r| r.after.is_none()),
        "same-document navigation must preserve secret snapshot suppression"
    );
    assert!(records[4..6].iter().all(|r| r.after.is_some()));

    // A fresh non-HTTP document clears suppression despite having no HTTP status.
    let (data_id, state) = f
        .run(
            &[
                f.nav("/form"),
                secret.clone(),
                Action::Navigate {
                    url: "data:text/html,<h1>d</h1>".into(),
                },
                extract("h1"),
            ],
            &cfg,
        )
        .await;
    assert_eq!(state, State::Committed);
    let records = f.l.records(data_id).unwrap();
    assert!(records[1].after.is_none());
    assert!(records[2..].iter().all(|r| r.after.is_some()));
    assert_eq!(records[3].outcome, "d");

    // A resumed secret Type restores the page flag before the approved click.
    let (resume_id, state) = f
        .run(
            &[
                f.nav("/form"),
                secret,
                Action::Click {
                    selector: "#go".into(),
                    risk: Risk::Financial,
                },
                extract("#out"),
            ],
            &cfg,
        )
        .await;
    assert_eq!(state, State::AwaitingHandoff(Handoff::Approval));
    f.l.approve(resume_id).unwrap();
    assert_eq!(
        f.e.run(&f.l, &f.c, &cfg, resume_id).await.unwrap(),
        State::Committed
    );
    let records = f.l.records(resume_id).unwrap();
    assert_eq!(records[3].outcome, "••••");
    assert!(records[1..4].iter().all(|r| r.after.is_none()));
    for id in [fragment_id, resume_id] {
        assert!(
            !serde_json::to_string(&f.l.records(id).unwrap())
                .unwrap()
                .contains("hunter2")
        );
    }
    for hash in f.c.all().unwrap() {
        assert!(!String::from_utf8_lossy(&f.c.get(&hash).unwrap()).contains("hunter2"));
    }
}
#[tokio::test]
#[ignore]
async fn same_document_navigation_commits() {
    let f = EngineFixture::new();
    let (_, s) = f
        .run(
            &[f.nav("/form"), f.nav("/form#x"), extract("h1")],
            &Config::default(),
        )
        .await;
    assert_eq!(s, State::Committed);
}
#[tokio::test]
#[ignore]
async fn bad_selector_error_is_explained() {
    let f = EngineFixture::new();
    let (_, s) = f.run(&[extract("##bad")], &Config::default()).await;
    assert!(
        matches!(s,State::Failed(ref m) if m.contains("SyntaxError")||m.contains("not a valid selector"))
    );
}

#[tokio::test]
#[ignore]
async fn about_blank_navigation_commits() {
    let f = EngineFixture::new();
    let (id, state) = tokio::time::timeout(
        Duration::from_secs(10),
        f.run(
            &[
                f.nav("/form"),
                Action::Navigate {
                    url: "about:blank".into(),
                },
                Action::Navigate {
                    url: "data:text/html,<h1>d</h1>".into(),
                },
                extract("h1"),
            ],
            &Config::default(),
        ),
    )
    .await
    .expect("non-HTTP navigation must commit within 10 seconds");
    assert_eq!(state, State::Committed);
    assert_eq!(f.outcome(id), "d");
}
