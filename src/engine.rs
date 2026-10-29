use crate::{
    action::Action,
    audit::StepRecord,
    cas::Cas,
    cdp::{self, Browser},
    challenge::{self, ChallengeKind},
    config::Config,
    gate::{self, Decision, Risk},
    ledger::{Handoff, Ledger, State},
    snapshot,
};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
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
fn scrub(s: &str, secrets: &[&str]) -> String {
    secrets
        .iter()
        .filter(|s| !s.is_empty())
        .fold(s.to_owned(), |s, secret| s.replace(secret, "••••"))
}
/// Sequential execution on the caller's future; the ledger is deliberately not Sync.
pub struct Engine {
    browser: RefCell<Option<Browser>>,
}
impl Engine {
    pub fn launch(chromium: &Path, profile: &Path, mock_keychain: bool) -> anyhow::Result<Self> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(profile)?;
        std::fs::set_permissions(profile, std::fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            browser: RefCell::new(Some(cdp::launch(
                chromium,
                &launch_args(profile, mock_keychain),
            )?)),
        })
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
        let mut response = None;
        let mut loaded = false;
        while response.is_none() || !loaded {
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
                let headers = serde_json::from_value::<std::collections::HashMap<String, String>>(
                    r["headers"].clone(),
                )
                .unwrap_or_default()
                .into_iter()
                .collect();
                response = Some((r["status"].as_f64().map(|n| n as u16), headers));
            }
            if e.method == "Page.loadEventFired" {
                loaded = true;
            }
        }
        Ok(response.unwrap())
    }
    async fn locate(&self, b: &Browser, s: &str, selector: &str) -> anyhow::Result<Value> {
        let selector = serde_json::to_string(selector)?;
        loop {
            let v=self.eval(b,s,format!("(()=>{{const e=document.querySelector({selector});if(!e)return null;e.scrollIntoView({{block:'center'}});const r=e.getBoundingClientRect();return {{x:r.x+r.width/2,y:r.y+r.height/2}}}})()")).await?;
            if !v.is_null() {
                return Ok(v);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    async fn action(
        &self,
        b: &Browser,
        s: &str,
        a: &Action,
        cfg: &Config,
    ) -> anyhow::Result<(String, Option<u16>, Vec<(String, String)>)> {
        match a {
            Action::Navigate { .. } | Action::Search { .. } => {
                let (st, h) = self.navigate(b, s, &url_of(a, cfg).unwrap()).await?;
                Ok((
                    if matches!(a, Action::Search { .. }) {
                        "searched"
                    } else {
                        "navigated"
                    }
                    .into(),
                    st,
                    h,
                ))
            }
            Action::Click { selector, .. } | Action::Type { selector, .. } => {
                let xy = self.locate(b, s, selector).await?;
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
                    b.call("Input.insertText", json!({"text":text}), Some(s))
                        .await?;
                }
                Ok((
                    if matches!(a, Action::Type { .. }) {
                        "typed"
                    } else {
                        "clicked"
                    }
                    .into(),
                    None,
                    vec![],
                ))
            }
            Action::Extract { selector } => {
                self.locate(b, s, selector).await?;
                let selector = serde_json::to_string(selector)?;
                let text = self
                    .eval(
                        b,
                        s,
                        format!("document.querySelector({selector}).innerText"),
                    )
                    .await?;
                Ok((text.as_str().unwrap_or("").into(), None, vec![]))
            }
        }
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
        let secrets: Vec<&str> = task
            .steps
            .iter()
            .filter_map(|a| match a {
                Action::Type {
                    text, secret: true, ..
                } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let mut secret_on_page = false;
        l.transition(id, State::Executing)?;
        let mut context = None;
        let result: anyhow::Result<State> = async {
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
                                self.action(b, &s, a, cfg).await?;
                                secret_on_page |= *secret;
                            }
                        }
                    }
                }
                Ok::<_, anyhow::Error>(s)
            })
            .await??;
            let s = result;
            let task = l.get(id)?;
            for (n, a) in task.steps.iter().enumerate().skip(task.cursor) {
                let decision = if gate::decide(cfg.auto_max, a.risk()) == Decision::NeedsApproval {
                    if task.approved_step != Some(n) {
                        let state = State::AwaitingHandoff(Handoff::Approval);
                        l.transition(id, state.clone())?;
                        return Ok(state);
                    }
                    "approved"
                } else {
                    "auto"
                };
                let execution = tokio::time::timeout(timeout, async {
                    let (outcome, status, headers) = self.action(b, &s, a, cfg).await?;
                    match a {
                        Action::Type { secret: true, .. } => secret_on_page = true,
                        // Fragment navigation leaves the secret-bearing Document intact.
                        Action::Navigate { .. } | Action::Search { .. } if status.is_some() => {
                            secret_on_page = false
                        }
                        _ => {}
                    }
                    let html = self
                        .eval(b, &s, "document.documentElement.outerHTML".into())
                        .await?
                        .as_str()
                        .unwrap_or("")
                        .to_owned();
                    let after = if secret_on_page || a.risk() == Risk::Financial {
                        None
                    } else {
                        Some(snapshot::put(c, &html)?.to_hex().to_string())
                    };
                    l.record_step(
                        id,
                        &StepRecord::new(n, a, decision.into(), after, scrub(&outcome, &secrets)),
                    )?;
                    let challenge = challenge::detect(status, &headers, &html);
                    Ok::<_, anyhow::Error>(match challenge {
                        Some(k) if self.remaining_challenge(b, &s, k).await? => Some(k),
                        _ => None,
                    })
                })
                .await;
                match execution {
                    Ok(Ok(Some(k))) => {
                        let state = State::AwaitingHandoff(Handoff::Challenge(k));
                        l.transition(id, state.clone())?;
                        return Ok(state);
                    }
                    Ok(Ok(None)) => {
                        l.set_cursor(id, n + 1)?;
                    }
                    Err(e) => return Err(e.into()),
                    Ok(Err(e)) => return Err(e),
                }
            }
            l.transition(id, State::Committed)?;
            Ok(State::Committed)
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
                    } else {
                        format!("error: {e}")
                    },
                    &secrets,
                );
                if task.state == State::Executing {
                    if let Some(a) = task.steps.get(task.cursor) {
                        l.record_step(
                            id,
                            &StepRecord::new(
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
                            ),
                        )?;
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
}
