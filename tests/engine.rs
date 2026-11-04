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

#[tokio::test]
#[ignore]
async fn missing_selector_names_it() {
    let f = EngineFixture::new();
    let id =
        f.l.add("missing", &[f.nav("/form"), extract("#nope")])
            .unwrap();
    let state =
        f.e.run_with_timeout(&f.l, &f.c, &Config::default(), id, Duration::from_secs(2))
            .await
            .unwrap();
    assert!(
        matches!(state, State::Failed(ref s) if s.contains("selector \"#nope\" not found within 2s")),
        "{state:?}"
    );
}

async fn observation_page(b: &iksa::cdp::Browser, path: &str) -> String {
    use serde_json::json;
    let t = b
        .call(
            "Target.createTarget",
            json!({"url":"about:blank", "hidden":true}),
            None,
        )
        .await
        .unwrap();
    let s = b
        .call(
            "Target.attachToTarget",
            json!({"targetId":t["targetId"],"flatten":true}),
            None,
        )
        .await
        .unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    b.call(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":1280,"height":800,"deviceScaleFactor":1,"mobile":false}),
        Some(&s),
    )
    .await
    .unwrap();
    b.call("Page.enable", json!({}), Some(&s)).await.unwrap();
    let mut events = b.events();
    b.call("Page.navigate", json!({"url":path}), Some(&s))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let e = events.recv().await.unwrap();
            if e.method == "Page.loadEventFired" && e.session_id.as_deref() == Some(&s) {
                break;
            }
        }
    })
    .await
    .unwrap();
    s
}
async fn eval_page(b: &iksa::cdp::Browser, s: &str, expression: &str) -> serde_json::Value {
    let r = b
        .call(
            "Runtime.evaluate",
            serde_json::json!({"expression":expression,"returnByValue":true}),
            Some(s),
        )
        .await
        .unwrap();
    assert!(r.get("exceptionDetails").is_none(), "{r}");
    r["result"]["value"].clone()
}
async fn assert_observed_selectors(
    b: &iksa::cdp::Browser,
    s: &str,
    o: &iksa::observe::Observation,
) {
    for e in &o.elements {
        let sel = serde_json::to_string(&e.selector).unwrap();
        assert_eq!(
            eval_page(b, s, &format!("document.querySelectorAll({sel}).length")).await,
            1,
            "{}",
            e.selector
        );
    }
}
#[tokio::test]
#[ignore]
async fn observe_lists_interactive_elements() {
    let chrome = common::Chrome::new(&[]);
    let port = common::serve(&[("/form", common::Response { status:200, headers:vec![], hang:false, body:r#"<title>Form</title><h1>Form</h1><label for=q>Query</label><input id=q placeholder=Search><button id=go>Go</button>"#.into() })]);
    let url = format!("http://127.0.0.1:{port}/form");
    let runner_profile = common::temp_dir("observe-runner");
    let e = Engine::launch(&common::chromium(), &runner_profile.0, true).unwrap();
    let s = observation_page(chrome.b(), &url).await;
    let b = chrome.b();
    let o = e.observe(b, &s).await.unwrap();
    assert_eq!(o.url, url);
    assert_eq!(o.title, "Form");
    assert!(o.text.contains("Form"));
    assert!(
        o.elements
            .iter()
            .any(|e| e.line.contains("textbox \"Query\"") && e.line.contains(" empty"))
    );
    assert!(o.elements.iter().any(|e| e.line.contains("button \"Go\"")));
    assert_observed_selectors(b, &s, &o).await;
    eval_page(b, &s, "document.querySelector('#q').focus()").await;
    b.call(
        "Input.insertText",
        serde_json::json!({"text":"hunter2"}),
        Some(&s),
    )
    .await
    .unwrap();
    eval_page(b, &s, "document.body.insertAdjacentHTML('beforeend', '<h2>hunter2</h2><button>hunter2</button>'); document.title = 'hunter2'").await;
    let o = e.observe(b, &s).await.unwrap();
    assert!(!o.title.contains("hunter2"));
    assert!(!o.text.contains("hunter2"));
    let q = o.elements.iter().find(|e| e.selector == "#q").unwrap();
    assert!(q.line.contains("filled"), "{}", q.line);
    assert!(o.elements.iter().all(|e| !e.line.contains("hunter2")));
    eval_page(b, &s, r#"document.body.innerHTML = `<label for=p>Password</label><input id=p type=password value=private-password><textarea aria-label=Notes>private-notes</textarea><div contenteditable aria-label=Editor>private-edit</div><select aria-label=Size><option value=private-option>Small</option><option>Large</option></select><a href='/next?private=query'>Next</a><div role=tab tabindex=0>Tab</div><button aria-label='${'界'.repeat(90)}'>Long</button><p>${'text '.repeat(1000)}</p>`"#).await;
    let o = e.observe(b, &s).await.unwrap();
    assert_observed_selectors(b, &s, &o).await;
    for secret in [
        "private-password",
        "private-notes",
        "private-edit",
        "private-option",
    ] {
        assert!(!o.text.contains(secret), "text leaked {secret}: {}", o.text);
        assert!(
            o.elements.iter().all(|e| !e.line.contains(secret)),
            "line leaked {secret}"
        );
    }
    assert!(
        o.elements
            .iter()
            .any(|e| e.line.contains("type=password filled"))
    );
    assert!(
        o.elements
            .iter()
            .any(|e| e.line.contains("combobox \"Size\"")
                && e.line.contains("Small")
                && e.line.contains("Large"))
    );
    assert!(
        o.elements
            .iter()
            .any(|e| e.line.contains("link \"Next\" → /next"))
    );
    assert!(o.elements.iter().any(|e| e.line.contains("tab \"Tab\"")));
    let long = o.elements.iter().find(|e| e.line.contains('界')).unwrap();
    assert_eq!(long.line.matches('界').count(), 80);
    assert!(o.text.chars().count() <= 4_000);
    eval_page(b, &s, "document.body.innerHTML = '<span hidden id=name>Hidden label</span><button aria-labelledby=name></button>'").await;
    let o = e.observe(b, &s).await.unwrap();
    assert_eq!(o.elements[0].line, "[1] button \"Hidden label\"");
    eval_page(
        b,
        &s,
        "document.body.innerHTML = '<button>Go</button>'.repeat(220)",
    )
    .await;
    assert_eq!(e.observe(b, &s).await.unwrap().elements.len(), 200);
}
#[tokio::test]
#[ignore]
async fn observe_skips_hidden() {
    let chrome = common::Chrome::new(&[]);
    let runner_profile = common::temp_dir("observe-runner");
    let e = Engine::launch(&common::chromium(), &runner_profile.0, true).unwrap();
    let s = observation_page(chrome.b(), "data:text/html,<button>Visible</button><button style='display:none'>Hidden</button><button style='visibility:hidden'>Invisible</button><div style='display:none'><button>Ancestor hidden</button></div><button style='width:0;height:0;padding:0;border:0'>Zero</button>").await;
    let o = e.observe(chrome.b(), &s).await.unwrap();
    assert_eq!(o.elements.len(), 1, "{o:?}");
    assert_eq!(o.elements[0].line, "[1] button \"Visible\"");
}
#[tokio::test]
#[ignore]
async fn observe_selectors_unique_without_ids() {
    let chrome = common::Chrome::new(&[]);
    let runner_profile = common::temp_dir("observe-runner");
    let e = Engine::launch(&common::chromium(), &runner_profile.0, true).unwrap();
    let s = observation_page(chrome.b(), "data:text/html,<main><section><button>Buy</button><button>Buy</button><button>Buy</button></section></main>").await;
    let o = e.observe(chrome.b(), &s).await.unwrap();
    assert_eq!(o.elements.len(), 3);
    let selectors: std::collections::HashSet<_> = o.elements.iter().map(|e| &e.selector).collect();
    assert_eq!(selectors.len(), 3);
    assert_observed_selectors(chrome.b(), &s, &o).await;
    for (i, element) in o.elements.iter().enumerate() {
        assert_eq!(element.r#ref, i as u32 + 1);
        assert_eq!(element.line, format!("[{}] button \"Buy\"", i + 1));
    }
    eval_page(chrome.b(), &s, r#"document.body.innerHTML = '<div id="a:b"><button id=dup>Buy</button><button id=dup>Buy</button><button id="1:x">Buy</button></div>'"#).await;
    let o = e.observe(chrome.b(), &s).await.unwrap();
    assert_eq!(o.elements.len(), 3);
    assert_observed_selectors(chrome.b(), &s, &o).await;
    assert!(o.elements[0].selector.starts_with("#a\\:b"));
    assert!(o.elements[2].selector.starts_with('#'));
    eval_page(chrome.b(), &s, "document.body.id = 'root:body'; document.body.innerHTML = '<section><button>Buy</button></section>'").await;
    let o = e.observe(chrome.b(), &s).await.unwrap();
    assert!(
        o.elements[0].selector.starts_with("#root\\:body > "),
        "{}",
        o.elements[0].selector
    );
    assert_observed_selectors(chrome.b(), &s, &o).await;
}
