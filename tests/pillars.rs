mod common;
use iksa::cdp;
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
struct PillarBrowser {
    chrome: common::Chrome,
}
impl PillarBrowser {
    async fn new(netlog: Option<&std::path::Path>) -> Self {
        let args: Vec<String> = netlog
            .into_iter()
            .map(|n| format!("--log-net-log={}", n.display()))
            .collect();
        let f = Self {
            chrome: common::Chrome::new(&args),
        };
        f.normalize().await;
        f
    }
    fn b(&self) -> &cdp::Browser {
        self.chrome.b()
    }
    async fn targets(&self) -> Vec<Value> {
        self.b()
            .call("Target.getTargets", json!({}), None)
            .await
            .unwrap()["targetInfos"]
            .as_array()
            .unwrap()
            .clone()
    }
    async fn normalize(&self) {
        let ts = self.targets().await;
        for t in ts.iter().filter(|t| t["type"] == "page") {
            self.b()
                .call(
                    "Target.closeTarget",
                    json!({"targetId":t["targetId"]}),
                    None,
                )
                .await
                .unwrap();
        }
        self.b()
            .call("Target.createTarget", json!({"url":"about:blank"}), None)
            .await
            .unwrap();
    }
    async fn target(&self, hidden: bool) -> String {
        let mut params = json!({"url":"about:blank"});
        if hidden {
            let c = self
                .b()
                .call("Target.createBrowserContext", json!({}), None)
                .await
                .unwrap();
            params["browserContextId"] = c["browserContextId"].clone();
            params["hidden"] = true.into();
        }
        let t = self
            .b()
            .call("Target.createTarget", params, None)
            .await
            .unwrap();
        let session = self
            .b()
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
        if hidden {
            self.b()
                .call(
                    "Emulation.setDeviceMetricsOverride",
                    json!({"width":1280,"height":800,"deviceScaleFactor":1,"mobile":false}),
                    Some(&session),
                )
                .await
                .unwrap();
        }
        session
    }
    async fn navigate(&self, s: &str, url: &str) -> String {
        tokio::time::timeout(Duration::from_secs(45), self.navigate_inner(s, url))
            .await
            .expect("BLOCKED whole navigation deadline")
    }
    async fn navigate_inner(&self, s: &str, url: &str) -> String {
        let b = self.b();
        b.call("Page.enable", json!({}), Some(s)).await.unwrap();
        b.call("Network.enable", json!({}), Some(s)).await.unwrap();
        let mut events = b.events();
        let nav = b
            .call("Page.navigate", json!({"url":url}), Some(s))
            .await
            .unwrap();
        assert!(
            nav["errorText"].as_str().unwrap_or("").is_empty(),
            "BLOCKED navigation {url}: {nav}"
        );
        let loader = nav["loaderId"].clone();
        let mut status = None;
        let mut loaded = false;
        tokio::time::timeout(Duration::from_secs(45), async {
            while status.is_none() || !loaded {
                let ev = match events.recv().await {
                    Ok(e) => e,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(e) => panic!("{e}"),
                };
                if ev.session_id.as_deref() != Some(s) {
                    continue;
                }
                if ev.method == "Network.responseReceived"
                    && ev.params["type"] == "Document"
                    && ev.params["loaderId"] == loader
                {
                    status = ev.params["response"]["status"].as_u64();
                }
                if ev.method == "Page.loadEventFired" {
                    loaded = true;
                }
            }
        })
        .await
        .expect("BLOCKED navigation timeout");
        let dom=b.call("Runtime.evaluate",json!({"expression":"JSON.stringify({title:document.title,text:document.body.innerText})","returnByValue":true}),Some(s)).await.unwrap();
        let text = dom["result"]["value"].as_str().unwrap();
        println!(
            "real navigation {url} status={status:?} DOM={}",
            text.chars().take(250).collect::<String>()
        );
        assert_eq!(status, Some(200), "BLOCKED non-200 {url}");
        assert!(text.len() > 100, "BLOCKED empty article {url}: {text}");
        assert!(!text.contains("ERR_"), "BLOCKED error page {text}");
        text.into()
    }
    fn close(mut self) {
        drop(self.chrome.b.take());
    }
}
fn artifact(dir: &common::TempDir, name: &str) -> PathBuf {
    dir.0.join(format!("{name}.json"))
}
async fn positive_control(dir: &common::TempDir) {
    let path = artifact(dir, "positive-netlog");
    let f = PillarBrowser::new(Some(&path)).await;
    let s = f.target(true).await;
    let text = f.navigate(&s, "https://example.com").await;
    assert!(text.contains("Example Domain"));
    f.close();
    let hosts = common::netlog_hosts(&path);
    println!("C11 hosts {hosts:?}");
    assert!(hosts.contains(&"example.com".into()));
}
#[tokio::test]
#[ignore]
async fn netlog_sees_real_navigation() {
    let dir = common::TempDir::new();
    positive_control(&dir).await;
}
#[tokio::test]
#[ignore]
async fn idle_egress_is_loopback_only() {
    let dir = common::TempDir::new();
    positive_control(&dir).await;
    let path = artifact(&dir, "idle-netlog");
    let f = PillarBrowser::new(Some(&path)).await;
    let ts = f.targets().await;
    assert_eq!(ts.iter().filter(|t| t["type"] == "page").count(), 1);
    tokio::time::sleep(Duration::from_secs(60)).await;
    f.close();
    let hosts = common::netlog_hosts(&path);
    println!("C12 hosts {hosts:?}; raw {}", path.display());
    assert!(hosts.is_empty());
}
#[tokio::test]
#[ignore]
async fn idle_footprint_under_300mb() {
    let dir = common::TempDir::new();
    let mut samples = vec![];
    for i in 0..3 {
        let f = PillarBrowser::new(None).await;
        tokio::time::sleep(Duration::from_secs(10)).await;
        let ts = f.targets().await;
        assert_eq!(ts.iter().filter(|t| t["type"] == "page").count(), 1);
        println!("idle sample {i} target inventory {ts:?}");
        std::fs::write(
            artifact(&dir, &format!("idle-targets-{i}")),
            serde_json::to_vec_pretty(&ts).unwrap(),
        )
        .unwrap();
        samples.push(common::tree_footprint_kib(f.b().pid()));
        f.close();
    }
    samples.sort();
    println!("C13 samples KiB={samples:?}; median {} KiB", samples[1]);
    assert!(samples[1] < 307200);
}
#[tokio::test]
#[ignore]
async fn five_tabs_three_agents_under_1200mb() {
    let dir = common::TempDir::new();
    let f = PillarBrowser::new(None).await;
    let initial = f
        .targets()
        .await
        .into_iter()
        .find(|t| t["type"] == "page")
        .unwrap();
    let first = f
        .b()
        .call(
            "Target.attachToTarget",
            json!({"targetId":initial["targetId"],"flatten":true}),
            None,
        )
        .await
        .unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let articles = [
        "Rust_(programming_language)",
        "Linux",
        "Chromium_(web_browser)",
        "SQLite",
        "Mozilla",
        "Python_(programming_language)",
        "Tokio_(software)",
        "WebKit",
    ];
    for (i, a) in articles.iter().enumerate() {
        let s = if i == 0 {
            first.clone()
        } else {
            f.target(i >= 5).await
        };
        let text = f
            .navigate(&s, &format!("https://en.wikipedia.org/wiki/{a}"))
            .await;
        assert!(text.contains("Wikipedia"), "BLOCKED missing article {a}");
        let dom: Value = serde_json::from_str(&text).unwrap();
        let title = format!("{} - Wikipedia", a.replace('_', " "));
        assert_eq!(dom["title"], title, "BLOCKED wrong article {a}");
        assert!(
            dom["text"].as_str().unwrap().len() > 500,
            "BLOCKED missing article DOM {a}"
        );
    }
    tokio::time::sleep(Duration::from_secs(20)).await;
    let ts = f.targets().await;
    println!("loaded target inventory {ts:?}");
    std::fs::write(
        artifact(&dir, "loaded-targets"),
        serde_json::to_vec_pretty(&ts).unwrap(),
    )
    .unwrap();
    assert_eq!(ts.iter().filter(|t| t["type"] == "page").count(), 5);
    assert_eq!(ts.iter().filter(|t| t["type"] == "other").count(), 3);
    let kib = common::tree_footprint_kib(f.b().pid());
    println!("C14 loaded {kib} KiB");
    assert!(kib < 1228800);
}
