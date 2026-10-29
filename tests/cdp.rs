use iksa::cdp;
mod common;
use serde_json::json;
use std::time::Duration;
async fn session(b: &cdp::Browser) -> String {
    let c = b
        .call("Target.createBrowserContext", json!({}), None)
        .await
        .unwrap();
    let t = b
        .call(
            "Target.createTarget",
            json!({"url":"about:blank","hidden":true,"browserContextId":c["browserContextId"]}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        b.call(
            "Target.getTargetInfo",
            json!({"targetId":t["targetId"]}),
            None
        )
        .await
        .unwrap()["targetInfo"]["type"],
        "other"
    );
    b.call(
        "Target.attachToTarget",
        json!({"targetId":t["targetId"],"flatten":true}),
        None,
    )
    .await
    .unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .into()
}
#[tokio::test]
#[ignore]
async fn version_over_pipe() {
    let f = common::Chrome::new(&[]);
    assert!(
        f.b()
            .call("Browser.getVersion", json!({}), None)
            .await
            .unwrap()["product"]
            .as_str()
            .unwrap()
            .starts_with("Chrome/156")
    );
}
#[tokio::test]
#[ignore]
async fn no_tcp_listener() {
    let f = common::Chrome::new(&[]);
    f.b()
        .call("Browser.getVersion", json!({}), None)
        .await
        .unwrap();
    let o = std::process::Command::new("lsof")
        .args([
            "-nP",
            "-a",
            "-p",
            &f.b().pid().to_string(),
            "-iTCP",
            "-sTCP:LISTEN",
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1), "unexpected lsof status: {:?}", o);
    assert!(o.stdout.is_empty());
    assert!(o.stderr.is_empty());
}
#[tokio::test]
#[ignore]
async fn hidden_target_navigate_eval_type() {
    let f = common::Chrome::new(&[]);
    let b = f.b();
    let s = session(b).await;
    b.call("Page.enable", json!({}), Some(&s)).await.unwrap();
    let port=common::serve(&[("/form",common::Response{status:200,headers:vec![],body:r#"<input id=q><button id=go onclick="out.textContent=q.value">Go</button><output id=out></output>"#.into(),hang:false})]);
    let mut events = b.events();
    b.call(
        "Page.navigate",
        json!({"url":format!("http://127.0.0.1:{port}/form")}),
        Some(&s),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let e = events.recv().await.unwrap();
            if e.session_id.as_deref() == Some(&s) && e.method == "Page.loadEventFired" {
                assert!(e.params.is_object());
                break;
            }
        }
    })
    .await
    .unwrap();
    b.call(
        "Runtime.evaluate",
        json!({"expression":"document.querySelector('#q').focus()"}),
        Some(&s),
    )
    .await
    .unwrap();
    b.call("Input.insertText", json!({"text":"pipe typed"}), Some(&s))
        .await
        .unwrap();
    let v=b.call("Runtime.evaluate",json!({"expression":"document.querySelector('#go').click(); document.querySelector('#out').textContent","returnByValue":true}),Some(&s)).await.unwrap();
    assert_eq!(v["result"]["value"], "pipe typed");
}
#[tokio::test]
#[ignore]
async fn large_eval_roundtrip() {
    let f = common::Chrome::new(&[]);
    let s = session(f.b()).await;
    let v = f
        .b()
        .call(
            "Runtime.evaluate",
            json!({"expression":"'x'.repeat(2_000_000)","returnByValue":true}),
            Some(&s),
        )
        .await
        .unwrap();
    assert_eq!(v["result"]["value"].as_str().unwrap().len(), 2_000_000);
}
#[tokio::test]
#[ignore]
async fn cdp_error_is_err() {
    let f = common::Chrome::new(&[]);
    let e = f
        .b()
        .call("Nope.nope", json!({}), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("-32601"), "{e}");
    assert!(e.contains("Nope.nope"), "{e}");
}
#[tokio::test]
#[ignore]
async fn browser_exit_fails_pending() {
    let f = common::Chrome::new(&[]);
    let b = f.b();
    let s = session(b).await;
    let pending = b.call(
        "Runtime.evaluate",
        json!({"expression":"new Promise(() => {})","awaitPromise":true}),
        Some(&s),
    );
    let kill = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(unsafe { libc::kill(b.pid() as i32, libc::SIGKILL) }, 0);
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(pending, kill)
    })
    .await
    .unwrap();
    assert!(result.unwrap_err().to_string().contains("browser closed"));
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        b.call("Browser.getVersion", json!({}), None),
    )
    .await
    .unwrap();
    assert!(result.unwrap_err().to_string().contains("browser closed"));
}
#[tokio::test]
#[ignore]
async fn drop_kills_browser() {
    let mut f = common::Chrome::new(&[]);
    f.b()
        .call("Browser.getVersion", json!({}), None)
        .await
        .unwrap();
    let pid = f.b().pid();
    drop(f.b.take());
    tokio::time::timeout(Duration::from_secs(2), async {
        while unsafe { libc::kill(pid as i32, 0) } == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
#[ignore]
async fn drop_closes_gracefully() {
    let mut f = common::Chrome::new(&[]);
    let _ = session(f.b()).await;
    drop(f.b.take());
    let prefs = std::fs::read(f.profile.0.join("Default/Preferences")).unwrap();
    let prefs: serde_json::Value = serde_json::from_slice(&prefs).unwrap();
    assert_eq!(prefs["profile"]["exit_type"], "Normal");
}

#[tokio::test]
#[ignore]
async fn drop_leaves_no_helpers() {
    let mut f = common::Chrome::new(&[]);
    let _ = session(f.b()).await;
    let helpers = common::descendants(f.b().pid());
    assert!(!helpers.is_empty(), "browser must have helpers before drop");
    drop(f.b.take());
    tokio::time::timeout(Duration::from_secs(5), async {
        while helpers
            .iter()
            .any(|pid| unsafe { libc::kill(*pid as i32, 0) } == 0)
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("browser helpers survived drop");
}
