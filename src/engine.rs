use crate::{
    action::Action,
    audit::StepRecord,
    cas::Cas,
    cdp::{self, Browser},
    challenge::{self, ChallengeKind},
    config::Config,
    gate::{self, Decision, Risk},
    keychain,
    ledger::{Handoff, Ledger, State},
    llm, observe, planner, snapshot,
};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::HashMap,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::Path,
    time::{Duration, Instant},
};
pub const STEP_TIMEOUT: Duration = Duration::from_secs(30);
pub const CAPTCHA_WAIT: Duration = Duration::from_secs(10);
pub fn launch_args(profile: &Path, mock_keychain: bool) -> Vec<String> {
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
        ]
        .map(String::from),
    );
    if mock_keychain {
        a.push("--use-mock-keychain".into());
    }
    a
}
fn url_of(a: &Action, cfg: &Config) -> Option<String> {
    match a {
        Action::Navigate { url } => Some(url.clone()),
        Action::Search { query } => Some(cfg.search_url(query)),
        _ => None,
    }
}
fn scrub(s: &str, secrets: &[String]) -> String {
    secrets
        .iter()
        .filter(|s| !s.is_empty())
        .fold(s.to_owned(), |s, secret| s.replace(secret, "••••"))
}
fn scrub_forms(text: &str, secrets: &[String], forms: impl Fn(&str) -> Vec<String>) -> String {
    let mut variants: Vec<_> = secrets
        .iter()
        .filter(|s| !s.is_empty())
        .flat_map(|s| forms(s))
        .collect();
    variants.sort_by_key(|s| std::cmp::Reverse(s.len()));
    variants.dedup();
    scrub(text, &variants)
}
fn scrub_snapshot(html: &str, secrets: &[String]) -> String {
    // outerHTML escapes text and attributes differently. Redact both serialized forms.
    scrub_forms(html, secrets, |secret| {
        let amp = secret.replace('&', "&amp;");
        let text = amp.replace('<', "&lt;").replace('>', "&gt;");
        vec![
            secret.to_owned(),
            text.clone(),
            text.replace('"', "&quot;"),
            amp.replace('"', "&quot;"),
        ]
    })
}
#[derive(Default)]
struct ModelStep {
    request: Option<String>,
    thought: Option<String>,
}
impl ModelStep {
    fn attach(&self, record: &mut StepRecord, secrets: &[String]) {
        record.model_request = self.request.clone();
        record.thought = self.thought.as_ref().map(|s| {
            scrub_model(s, secrets)
                .chars()
                .take(crate::audit::MAX_THOUGHT_CHARS)
                .collect()
        });
    }
}
fn planner_record(
    l: &Ledger,
    id: i64,
    step: usize,
    outcome: String,
    metadata: &ModelStep,
    secrets: &[String],
) -> anyhow::Result<()> {
    // Audit-only page-observation marker: never append or execute this as a browser step.
    let mut record = StepRecord::new(
        step,
        &Action::Extract {
            selector: "html".into(),
        },
        "planner".into(),
        None,
        scrub_model(&outcome, secrets),
    );
    metadata.attach(&mut record, secrets);
    l.record_step(id, &record)?;
    Ok(())
}
fn action_summary(action: &Action) -> String {
    match action {
        Action::Navigate { url } => format!("navigate {url}"),
        Action::Search { .. } => "search".into(),
        Action::Click { .. } => "click element".into(),
        Action::Type { .. } => "type element".into(),
        Action::Extract { .. } => "extract element".into(),
    }
}
fn action_contains_secret(action: &Action, secrets: &[String]) -> bool {
    let fields: Vec<&str> = match action {
        Action::Navigate { url } => vec![url],
        Action::Search { query } => vec![query],
        Action::Click { selector, .. } | Action::Extract { selector } => vec![selector],
        Action::Type { selector, text, .. } => vec![selector, text],
    };
    fields.iter().any(|value| {
        secrets
            .iter()
            .any(|s| !s.is_empty() && value.contains(s.as_str()))
    })
}
fn scrub_model(text: &str, secrets: &[String]) -> String {
    scrub_forms(text, secrets, |secret| {
        let quoted = serde_json::to_string(secret).unwrap();
        vec![secret.to_owned(), quoted[1..quoted.len() - 1].to_owned()]
    })
}
/// Sequential execution on the caller's future; the ledger is deliberately not Sync.
pub struct Engine {
    browser: RefCell<Option<Browser>>,
}
impl Engine {
    pub fn new(browser: Browser) -> Self {
        Self {
            browser: RefCell::new(Some(browser)),
        }
    }
    pub fn is_available(&self) -> bool {
        self.browser.borrow().is_some()
    }
    pub fn launch(chromium: &Path, profile: &Path, mock_keychain: bool) -> anyhow::Result<Self> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(profile)?;
        std::fs::set_permissions(profile, std::fs::Permissions::from_mode(0o700))?;
        Ok(Self::new(cdp::launch(
            chromium,
            &launch_args(profile, mock_keychain),
        )?))
    }
    pub async fn observe(
        &self,
        b: &Browser,
        session: &str,
    ) -> anyhow::Result<crate::observe::Observation> {
        let value = self
            .eval(b, session, crate::observe::OBSERVE_JS.into())
            .await?;
        Ok(serde_json::from_value(value)?)
    }
    async fn eval(&self, b: &Browser, s: &str, expr: String) -> anyhow::Result<Value> {
        let r = b
            .call(
                "Runtime.evaluate",
                json!({"expression":expr,"returnByValue":true}),
                Some(s),
            )
            .await?;
        if r.get("exceptionDetails").is_some() {
            anyhow::bail!(
                "JavaScript evaluation failed: {}",
                r["exceptionDetails"]["exception"]["description"]
                    .as_str()
                    .or_else(|| r["exceptionDetails"]["text"].as_str())
                    .unwrap_or("unknown exception")
            );
        }
        Ok(r["result"]["value"].clone())
    }
    async fn navigate(
        &self,
        b: &Browser,
        s: &str,
        url: &str,
    ) -> anyhow::Result<(Option<u16>, Vec<(String, String)>)> {
        let mut events = b.events();
        let nav = b.call("Page.navigate", json!({"url":url}), Some(s)).await?;
        if let Some(error) = nav["errorText"].as_str().filter(|v| !v.is_empty()) {
            anyhow::bail!("navigation: {error}");
        }
        let Some(loader) = nav["loaderId"].as_str() else {
            return Ok((None, vec![]));
        };
        let http = url.split_once(':').is_some_and(|(scheme, _)| {
            scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
        });
        let mut response = None;
        let mut loaded = false;
        while (http && response.is_none()) || !loaded {
            let e = match events.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    eprintln!("CDP events lagged: skipped {n}");
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    anyhow::bail!("browser event stream closed")
                }
            };
            if e.session_id.as_deref() != Some(s) {
                continue;
            }
            if e.method == "Network.responseReceived"
                && e.params["type"] == "Document"
                && e.params["loaderId"] == loader
            {
                let r = &e.params["response"];
                let headers =
                    serde_json::from_value::<HashMap<String, String>>(r["headers"].clone())
                        .unwrap_or_default()
                        .into_iter()
                        .collect();
                response = Some((r["status"].as_f64().map(|n| n as u16), headers));
            }
            if e.method == "Page.loadEventFired" {
                loaded = true;
            }
        }
        Ok(if http {
            response.unwrap()
        } else {
            (None, vec![])
        })
    }
    async fn locate(
        &self,
        b: &Browser,
        s: &str,
        selector: &str,
        deadline: Instant,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        let original = selector;
        let selector = serde_json::to_string(selector)?;
        tokio::time::timeout_at(deadline.into(), async {
        loop {
            let v=self.eval(b,s,format!("(()=>{{const e=document.querySelector({selector});if(!e)return null;e.scrollIntoView({{block:'center'}});const r=e.getBoundingClientRect();return {{x:r.x+r.width/2,y:r.y+r.height/2}}}})()")).await?;
            if !v.is_null() {
                return Ok(v);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        }).await.map_err(|_| anyhow::anyhow!("timeout: selector {original:?} not found within {timeout:?}"))?
    }
    async fn action(
        &self,
        b: &Browser,
        s: &str,
        a: &Action,
        cfg: &Config,
        deadline: Instant,
        timeout: Duration,
        secrets: &mut Vec<String>,
    ) -> anyhow::Result<(String, Option<u16>, Vec<(String, String)>)> {
        if let Action::Type {
            text,
            secret: false,
            ..
        } = a
        {
            anyhow::ensure!(
                !text.contains("{{secret:"),
                "secret placeholder requires \"secret\": true"
            );
        }
        let (status, headers) = match a {
            Action::Navigate { .. } | Action::Search { .. } => {
                let (st, h) = self.navigate(b, s, &url_of(a, cfg).unwrap()).await?;
                (st, h)
            }
            Action::Click { selector, .. } | Action::Type { selector, .. } => {
                let xy = self.locate(b, s, selector, deadline, timeout).await?;
                for kind in ["mousePressed", "mouseReleased"] {
                    b.call(
                        "Input.dispatchMouseEvent",
                        json!({"type":kind,"x":xy["x"],"y":xy["y"],"button":"left","clickCount":1}),
                        Some(s),
                    )
                    .await?;
                }
                if let Action::Type { text, .. } = a {
                    // A hidden target can acknowledge the mouse event before its
                    // editable element has focus. Establish and verify focus after
                    // the real click before sending text to the renderer.
                    let selector = serde_json::to_string(selector)?;
                    let focused=self.eval(b,s,format!("(()=>{{const e=document.querySelector({selector});if(!e)return false;e.focus();return document.activeElement===e}})()")).await?;
                    if focused != true {
                        anyhow::bail!("typed element could not receive focus");
                    }
                    let resolved = if let Action::Type { secret: true, .. } = a {
                        if let Some(name) = keychain::secret_name(text) {
                            let name = name.to_owned();
                            tokio::task::spawn_blocking(move || keychain::get("iksa-secret", &name))
                                .await??
                        } else {
                            text.clone()
                        }
                    } else {
                        text.clone()
                    };
                    if matches!(a, Action::Type { secret: true, .. })
                        && !secrets.contains(&resolved)
                    {
                        secrets.push(resolved.clone());
                    }
                    b.call("Input.insertText", json!({"text":resolved}), Some(s))
                        .await?;
                }
                (None, vec![])
            }
            Action::Extract { .. } => (None, vec![]),
        };
        let outcome = match a {
            Action::Navigate { .. } => "navigated".into(),
            Action::Search { .. } => "searched".into(),
            Action::Click { .. } => "clicked".into(),
            Action::Type { .. } => "typed".into(),
            Action::Extract { selector } => {
                self.locate(b, s, selector, deadline, timeout).await?;
                let selector = serde_json::to_string(selector)?;
                self.eval(
                    b,
                    s,
                    format!("document.querySelector({selector}).innerText"),
                )
                .await?
                .as_str()
                .unwrap_or("")
                .into()
            }
        };
        Ok((outcome, status, headers))
    }
    async fn remaining_challenge(
        &self,
        b: &Browser,
        s: &str,
        k: ChallengeKind,
    ) -> anyhow::Result<bool> {
        let name = match k {
            ChallengeKind::Turnstile => "cf-turnstile-response",
            ChallengeKind::ReCaptcha => "g-recaptcha-response",
            ChallengeKind::HCaptcha => "h-captcha-response",
            ChallengeKind::CloudflareChallenge => return Ok(true),
        };
        let selector = serde_json::to_string(&format!("[name=\"{name}\"]"))?;
        let deadline = Instant::now() + CAPTCHA_WAIT;
        loop {
            let v = self
                .eval(
                    b,
                    s,
                    format!("document.querySelector({selector})?.value || ''"),
                )
                .await?;
            if v.as_str().is_some_and(|v| !v.is_empty()) {
                return Ok(false);
            }
            if Instant::now() >= deadline {
                return Ok(true);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    pub async fn run(&self, l: &Ledger, c: &Cas, cfg: &Config, id: i64) -> anyhow::Result<State> {
        self.run_with_timeout(l, c, cfg, id, STEP_TIMEOUT).await
    }
    pub async fn run_with_timeout(
        &self,
        l: &Ledger,
        c: &Cas,
        cfg: &Config,
        id: i64,
        timeout: Duration,
    ) -> anyhow::Result<State> {
        let browser = self.browser.borrow();
        let b = browser
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("browser unavailable after cleanup timeout"))?;
        let task = l.get(id)?;
        let mut secrets: Vec<String> = task
            .steps
            .iter()
            .filter_map(|a| match a {
                Action::Type {
                    text, secret: true, ..
                } if keychain::secret_name(text).is_none() => Some(text.clone()),
                _ => None,
            })
            .collect();
        let mut private_fields: Vec<String> = task
            .steps
            .iter()
            .take(task.cursor)
            .filter_map(|a| match a {
                Action::Type {
                    text,
                    secret: false,
                    ..
                } if !text.is_empty() => Some(text.clone()),
                _ => None,
            })
            .collect();
        let mut secret_on_page = false;
        l.transition(id, State::Executing)?;
        let mut context = None;
        let mut model_step = ModelStep::default();
        let result: anyhow::Result<State> = async {
            anyhow::ensure!(task.cursor <= task.steps.len(), "invalid task cursor");
            // A used placeholder may precede the last navigation, so replay alone cannot
            // restore the redaction list. Re-read only secrets this task already typed.
            let used: std::collections::HashSet<_> = task.steps[..task.cursor].iter().filter_map(|a| match a {
                Action::Type {text,secret:true,..} => keychain::secret_name(text).map(str::to_owned),
                _ => None,
            }).collect();
            for name in used {
                let value = tokio::time::timeout(timeout, tokio::task::spawn_blocking(move || keychain::get("iksa-secret", &name))).await???;
                if !secrets.contains(&value) { secrets.push(value); }
            }
            let ctx = tokio::time::timeout(
                timeout,
                b.call("Target.createBrowserContext", json!({}), None),
            )
            .await??;
            let ctx = ctx["browserContextId"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("missing context id"))?
                .to_owned();
            context = Some(ctx.clone());
            let result = tokio::time::timeout(timeout, async {
                let target = b
                    .call(
                        "Target.createTarget",
                        json!({"url":"about:blank","hidden":true,"browserContextId":ctx}),
                        None,
                    )
                    .await?;
                let session = b
                    .call(
                        "Target.attachToTarget",
                        json!({"targetId":target["targetId"],"flatten":true}),
                        None,
                    )
                    .await?;
                let s = session["sessionId"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing session id"))?
                    .to_owned();
                for method in ["Page.enable", "Network.enable"] {
                    b.call(method, json!({}), Some(&s)).await?;
                }
                b.call(
                    "Emulation.setDeviceMetricsOverride",
                    json!({"width":1280,"height":800,"deviceScaleFactor":1,"mobile":false}),
                    Some(&s),
                )
                .await?;
                let task = l.get(id)?;
                if task.cursor > 0 {
                    if let Some(nav) = task.steps[..task.cursor]
                        .iter()
                        .rposition(|a| url_of(a, cfg).is_some())
                    {
                        let replay = &task.steps[nav + 1..task.cursor];
                        if let Some(i) = replay
                            .iter()
                            .position(|a| matches!(a, Action::Click { .. }))
                        {
                            anyhow::bail!(
                                "cannot resume: step {} would repeat a click",
                                nav + 1 + i
                            );
                        }
                        self.navigate(b, &s, &url_of(&task.steps[nav], cfg).unwrap())
                            .await?;
                        for a in replay {
                            if let Action::Type { secret, .. } = a {
                                self.action(b, &s, a, cfg, Instant::now() + timeout, timeout, &mut secrets).await?;
                                secret_on_page |= *secret;
                            }
                        }
                    }
                }
                Ok::<_, anyhow::Error>(s)
            })
            .await??;
            let s = result;
            let mut invalid_streak = l.records(id)?.iter().rev()
                .take_while(|r| r.outcome.starts_with("invalid:")).count();
            loop {
                let task = l.get(id)?;
                if task.steps.get(task.cursor).is_none() {
                    let Some(goal) = task.goal.as_deref() else {
                        l.transition(id, State::Committed)?;
                        return Ok(State::Committed);
                    };
                    let model = cfg.model.as_ref().ok_or_else(|| anyhow::anyhow!("configure \"model\" in config.json"))?;
                    let records = l.records(id)?;
                    // Control records each spend a model turn. An action spends one
                    // turn per appended step, even if approval/challenge replay records it twice.
                    // CAS hashes alone cannot count turns: redaction may make bodies identical.
                    let control_turns = records.iter().filter(|r| r.model_request.is_some() && r.decision == "planner").count();
                    let action_turns = records.iter().filter(|r| r.model_request.is_some() && r.decision != "planner")
                        .map(|r| r.step).collect::<std::collections::HashSet<_>>().len();
                    let turns = control_turns + action_turns;
                    if turns >= model.max_steps as usize {
                        let state = State::Failed("step budget exhausted".into());
                        l.transition(id, state.clone())?;
                        return Ok(state);
                    }
                    // Capture the original host and pruned observation atomically in one evaluation.
                    let value = tokio::time::timeout(timeout, self.eval(b, &s,
                        format!("({{host:location.hostname,fields:Array.from(document.querySelectorAll('input,textarea,[contenteditable]')).filter(e=>e.tagName!=='INPUT'||!['checkbox','radio','button','submit','reset','image'].includes(e.type)).flatMap(e=>[e.value||'',e.defaultValue||'',e.hasAttribute('contenteditable')?e.innerText:'']),observation:{}}})", observe::OBSERVE_JS)))
                        .await??;
                    let host = value["host"].as_str().unwrap_or("").trim_end_matches('.').to_ascii_lowercase();
                    if cfg.never_send.iter().any(|suffix| {
                        let suffix = suffix.trim_start_matches('.').trim_end_matches('.').to_ascii_lowercase();
                        !suffix.is_empty() && (host == suffix || host.ends_with(&format!(".{suffix}")))
                    }) {
                        planner_record(l, id, task.cursor,
                            format!("blocked: {host} is on never_send"), &ModelStep::default(), &secrets)?;
                        let state = State::AwaitingHandoff(Handoff::Approval);
                        l.transition(id, state.clone())?;
                        return Ok(state);
                    }
                    if let Some(fields) = value["fields"].as_array() {
                        for field in fields.iter().filter_map(Value::as_str).filter(|v| !v.is_empty()) {
                            if !private_fields.iter().any(|v| v == field) { private_fields.push(field.to_owned()); }
                        }
                    }
                    let mut redactions = secrets.clone();
                    redactions.extend(private_fields.iter().cloned());
                    let observation: observe::Observation = serde_json::from_value(value["observation"].clone())?;
                    let log: Vec<_> = records.iter().enumerate().map(|(n,r)| {
                        let summary = if r.decision == "planner" { "planner".into() } else { action_summary(&r.action) };
                        format!("{}. {} → {}", n+1, scrub_model(&summary, &redactions),
                            scrub_model(&r.outcome, &redactions).chars().take(200).collect::<String>())
                    }).collect();
                    let user = scrub_model(&planner::user_message(goal, &log, &observe::render(&observation)), &redactions);
                    let body = llm::request_body(model, planner::SYSTEM, &planner::tools(), &user);
                    model_step = ModelStep { request:Some(c.put(&serde_json::to_vec(&body)?)?.to_hex().to_string()), thought:None };
                    let model = model.clone();
                    let reply = tokio::task::spawn_blocking(move || {
                        let key = llm::api_key()?;
                        llm::call(&model, &key, &body)
                    }).await??;
                    model_step.thought = Some(reply.text);
                    let chosen = if reply.tool_count != 1 {
                        Err("expected exactly one tool call".to_owned())
                    } else if let Some((tool, input)) = &reply.tool {
                        planner::to_action(tool, input, &observation).and_then(|action| {
                            if let Some(Action::Type { text, secret:true, .. }) = &action {
                                let name = keychain::secret_name(text).unwrap();
                                if !goal.split(|c:char| !c.is_ascii_alphanumeric() && c != '_' && c != '-').any(|word| word == name) {
                                    return Err(format!("goal does not name secret {name}"));
                                }
                            }
                            if action.as_ref().is_some_and(|a| action_contains_secret(a, &secrets)) {
                                return Err("action contains a resolved secret; use a named placeholder".into());
                            }
                            Ok(action)
                        })
                    } else { Err("expected exactly one tool call".into()) };
                    match chosen {
                        Err(error) => {
                            planner_record(l, id, task.cursor, format!("invalid: {error}"), &model_step, &secrets)?;
                            model_step = ModelStep::default();
                            invalid_streak += 1;
                            if invalid_streak >= 3 {
                                let state = State::Failed("model kept choosing invalid actions".into());
                                l.transition(id, state.clone())?;
                                return Ok(state);
                            }
                            continue;
                        }
                        Ok(None) => {
                            let answer = reply.tool.as_ref().unwrap().1["answer"].as_str().unwrap();
                            planner_record(l, id, task.cursor, format!("done: {answer}"), &model_step, &secrets)?;
                            l.transition(id, State::Committed)?;
                            return Ok(State::Committed);
                        }
                        Ok(Some(action)) => {
                            invalid_streak = 0;
                            l.push_step(id, &action)?;
                        }
                    }
                }
                let task = l.get(id)?;
                let n = task.cursor;
                let a = &task.steps[n];
                if task.goal.is_some() && model_step.request.is_none() {
                    if let Some(record) = l.records(id)?.iter().rev().find(|r|
                        r.step == n && r.action == *a && r.model_request.is_some() ) {
                        model_step = ModelStep {request:record.model_request.clone(),thought:record.thought.clone()};
                    }
                }
                let decision = if gate::decide(cfg.auto_max, a.risk()) == Decision::NeedsApproval {
                    if task.approved_step != Some(n) {
                        if model_step.request.is_some() {
                            let mut record = StepRecord::new(n, a, "needs approval".into(), None, "awaiting approval".into());
                            model_step.attach(&mut record, &secrets);
                            l.record_step(id, &record)?;
                        }
                        let state = State::AwaitingHandoff(Handoff::Approval);
                        l.transition(id, state.clone())?;
                        return Ok(state);
                    }
                    "approved"
                } else { "auto" };
                let deadline = Instant::now() + timeout;
                let execution = async {
                    let before_document = if secret_on_page && url_of(a, cfg).is_some() {
                        Some(b.call("Page.getFrameTree", json!({}), Some(&s)).await?["frameTree"]["frame"]["loaderId"].clone())
                    } else { None };
                    let (outcome, status, headers) = self.action(b, &s, a, cfg, deadline, timeout, &mut secrets).await?;
                    if task.goal.is_some() {
                        if let Action::Type {text,secret:false,..} = a {
                            if !text.is_empty() && !private_fields.contains(text) { private_fields.push(text.clone()); }
                        }
                    }
                    let new_document = if let Some(before) = before_document {
                        before != b.call("Page.getFrameTree", json!({}), Some(&s)).await?["frameTree"]["frame"]["loaderId"]
                    } else { false };
                    match a {
                        Action::Type { secret:true, .. } => secret_on_page = true,
                        Action::Navigate { .. } | Action::Search { .. } if new_document => secret_on_page = false,
                        _ => {}
                    }
                    let html = self.eval(b, &s, "document.documentElement.outerHTML".into()).await?.as_str().unwrap_or("").to_owned();
                    let after = if secret_on_page || a.risk() == Risk::Financial { None }
                        else { Some(snapshot::put(c, &scrub_snapshot(&html, &secrets))?.to_hex().to_string()) };
                    let safe_outcome = if task.goal.is_some() {
                        let mut redactions = secrets.clone();
                        redactions.extend(private_fields.iter().cloned());
                        scrub_model(&outcome, &redactions)
                    } else { scrub(&outcome, &secrets) };
                    let mut record = StepRecord::new(n, a, decision.into(), after, safe_outcome);
                    model_step.attach(&mut record, &secrets);
                    l.record_step(id, &record)?;
                    let challenge = challenge::detect(status, &headers, &html);
                    Ok::<_, anyhow::Error>(match challenge {
                        Some(k) if self.remaining_challenge(b, &s, k).await? => Some(k),
                        _ => None,
                    })
                };
                let execution = tokio::select! {
                    biased;
                    result = execution => result,
                    _ = tokio::time::sleep_until(deadline.into()) => Err(anyhow::anyhow!("timeout at step {n}")),
                };
                if let Some(k) = execution? {
                    let state = State::AwaitingHandoff(Handoff::Challenge(k));
                    l.transition(id, state.clone())?;
                    return Ok(state);
                }
                l.set_cursor(id, n + 1)?;
                model_step = ModelStep::default();
            }
        }
        .await;
        let cleanup_failed = if let Some(ctx) = context {
            !matches!(
                tokio::time::timeout(
                    Duration::from_secs(2),
                    b.call(
                        "Target.disposeBrowserContext",
                        json!({"browserContextId":ctx}),
                        None
                    )
                )
                .await,
                Ok(Ok(_))
            )
        } else {
            true
        };
        drop(browser);
        if cleanup_failed {
            drop(self.browser.borrow_mut().take());
        }
        match result {
            Ok(state) => Ok(state),
            Err(e) => {
                let task = l.get(id)?;
                let message = scrub(
                    &if e.is::<tokio::time::error::Elapsed>() {
                        format!("timeout at step {}", task.cursor)
                    } else if task.goal.is_some() {
                        e.to_string()
                    } else {
                        format!("error: {e}")
                    },
                    &secrets,
                );
                if task.state == State::Executing {
                    if let Some(a) = task.steps.get(task.cursor) {
                        let mut record = StepRecord::new(
                            task.cursor,
                            a,
                            if task.approved_step == Some(task.cursor) {
                                "approved"
                            } else {
                                "auto"
                            }
                            .into(),
                            None,
                            message.clone(),
                        );
                        model_step.attach(&mut record, &secrets);
                        l.record_step(id, &record)?;
                    } else if task.goal.is_some() {
                        planner_record(l, id, task.cursor, message.clone(), &model_step, &secrets)?;
                    }
                    let state = State::Failed(message);
                    l.transition(id, state.clone())?;
                    Ok(state)
                } else {
                    Err(e)
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launch_args_exact() {
        assert_eq!(
            launch_args(Path::new("/private/profile"), false),
            crate::engine::tests::expected()
        );
    }
    fn expected() -> Vec<String> {
        let mut a = vec!["--user-data-dir=/private/profile".into()];
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
            ]
            .map(String::from),
        );
        a
    }
    #[test]
    fn mock_keychain_only_when_asked() {
        let p = Path::new("/private/profile");
        let mut e = expected();
        e.push("--use-mock-keychain".into());
        assert_eq!(launch_args(p, true), e);
        assert!(
            !launch_args(p, false)
                .iter()
                .any(|s| s.contains("mock-keychain"))
        );
    }
    #[test]
    fn snapshot_scrubs_html_escaped_secrets() {
        let secrets = vec!["a<&\"b".to_owned()];
        assert_eq!(
            scrub_snapshot(
                r#"<p>a&lt;&amp;"b</p><input value="a&lt;&amp;&quot;b"><span>a<&"b</span>"#,
                &secrets
            ),
            "<p>••••</p><input value=\"••••\"><span>••••</span>"
        );
    }
}
