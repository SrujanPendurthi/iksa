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
struct F {
    e: Engine,
    l: Ledger,
    c: Cas,
    p: std::path::PathBuf,
    port: u16,
}
impl F {
    fn new() -> Self {
        let p = common::temp_profile();
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
            // Every read changes the DOM so each measured snapshot writes new
            // CAS blobs through the production fsync path instead of deduplicating.
            (
                "/overhead",
                r(
                    "<h1>Form</h1><script>const h=document.querySelector('h1');const native=Object.getOwnPropertyDescriptor(HTMLElement.prototype,'innerText').get;let n=0;Object.defineProperty(h,'innerText',{get(){document.body.dataset.step=String(++n);return native.call(h)}})</script>",
                ),
            ),
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
        let e = Engine::launch(&common::chromium(), &p.join("profile"), true).unwrap();
        let l = Ledger::open(p.join("ledger.db")).unwrap();
        let c = Cas::open(p.join("cas")).unwrap();
        Self { e, l, c, p, port }
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
impl Drop for F {
    fn drop(&mut self) {
        let _ = &self.p;
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
    let f = F::new();
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
    let f = F::new();
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
    let f = F::new();
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
    let f = F::new();
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
    let f = F::new();
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
    let f = F::new();
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
    let f = F::new();
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
    let f = F::new();
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
async fn step_overhead_under_25ms() {
    let f = F::new();
    let mut steps = vec![f.nav("/overhead")];
    steps.extend((0..20).map(|_| extract("h1")));
    let (id, s) = f.run(&steps, &Config::default()).await;
    let records = f.l.records(id).unwrap();
    assert_eq!(
        records
            .iter()
            .map(|r| r.after.clone().unwrap())
            .collect::<std::collections::HashSet<_>>()
            .len(),
        21,
        "all measured snapshots must be distinct so CAS writes and fsync occur"
    );
    assert!(f.c.all().unwrap().len() >= 42);
    assert_eq!(s, State::Committed);
    let mut times = f.e.step_overheads();
    assert_eq!(times.len(), 21);
    times.remove(0);
    times.sort();
    let median = (times[9] + times[10]) / 2;
    println!(
        "engine overhead samples {times:?}; median {median:?}; upper median {:?}",
        times[10]
    );
    assert!(times[10] < Duration::from_millis(25));
}
