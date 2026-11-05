mod common;
use iksa::{cdp, engine};
use serde_json::{Value, json};
use std::time::Duration;

async fn worker_loaded(browser: &cdp::Browser) -> bool {
    let prefix = format!("chrome-extension://{}/", iksa::extension::EXTENSION_ID);
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let targets = browser
                .call("Target.getTargets", json!({}), None)
                .await
                .unwrap();
            if targets["targetInfos"].as_array().unwrap().iter().any(|t| {
                t["type"] == "service_worker"
                    && t["url"]
                        .as_str()
                        .is_some_and(|url| url.starts_with(&prefix))
            }) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .is_ok()
}

#[tokio::test]
#[ignore]
async fn extension_loading_flag_is_supported() {
    let home = common::temp_dir("m3-extension-probe");
    let extension = home.0.join("extension");
    std::fs::create_dir(&extension).unwrap();
    let manifest: Value =
        serde_json::from_str(include_str!("../extension/manifest.base.json")).unwrap();
    std::fs::write(
        extension.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    std::fs::write(
        extension.join("background.js"),
        "chrome.runtime.onInstalled.addListener(() => {});\n",
    )
    .unwrap();
    let mut args = engine::launch_args(&home.0.join("profile"), true, None);
    args.push(format!("--load-extension={}", extension.display()));
    let browser = cdp::launch(&common::chromium(), &args).unwrap();
    assert!(
        worker_loaded(&browser).await,
        "--load-extension ignored on pinned build; evaluate the planned CDP fallback"
    );
    println!("M3 extension loading: --load-extension works on pinned Chromium");
}

#[tokio::test]
#[ignore]
async fn extension_loads_and_owns_search() {
    use iksa::{config::Config, extension};
    let home = common::temp_dir("m3-search");
    let cfg = Config {
        search: "Kagi".into(),
        ..Config::default()
    };
    let dir = extension::install(
        &home.0,
        &cfg,
        std::path::Path::new(env!("CARGO_BIN_EXE_iksa")),
    )
    .unwrap();
    let args = engine::launch_args(&home.0.join("profile"), true, Some(&dir));
    let browser = cdp::launch(&common::chromium(), &args).unwrap();
    let loaded = worker_loaded(&browser).await;
    if !loaded {
        println!(
            "generated manifest: {}",
            std::fs::read_to_string(dir.join("manifest.json")).unwrap()
        );
        println!(
            "targets: {}",
            browser
                .call("Target.getTargets", json!({}), None)
                .await
                .unwrap()
        );
    }
    assert!(loaded);
    let target = browser
        .call(
            "Target.createTarget",
            json!({"url":"chrome://settings/searchEngines"}),
            None,
        )
        .await
        .unwrap();
    let session = browser
        .call(
            "Target.attachToTarget",
            json!({"targetId":target["targetId"],"flatten":true}),
            None,
        )
        .await
        .unwrap();
    let session = session["sessionId"].as_str().unwrap();
    let mut last_settings = String::new();
    let text = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let value = browser.call("Runtime.evaluate", json!({"expression":r#"(() => { const parts = []; const walk = root => { for (const e of root.querySelectorAll('*')) { if (e.shadowRoot) walk(e.shadowRoot); if (e.innerText && e.getClientRects().length) parts.push(e.innerText); } }; walk(document); return parts.join('\n'); })()"#,"returnByValue":true}), Some(session)).await.unwrap();
            let text = value["result"]["value"].as_str().unwrap_or("");
            last_settings = format!("{value}");
            let rendered = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if rendered.contains("Kagi (Default)") { return rendered; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }).await.unwrap_or_else(|error| panic!("Kagi (Default) missing: {error}; settings={last_settings}"));
    assert!(
        text.contains("iksa"),
        "extension control banner missing: {text}"
    );
    println!("M3 effective search settings: Kagi (Default); iksa control banner present");
}

#[test]
#[ignore]
fn daemon_installs_bundled_extension() {
    use iksa::{config::Config, extension, ledger::Ledger};
    let home = common::temp_dir("m3-daemon-extension");
    let cfg = Config {
        search: "Kagi".into(),
        ..Config::default()
    };
    std::fs::write(
        home.0.join("config.json"),
        serde_json::to_vec(&cfg).unwrap(),
    )
    .unwrap();
    let ledger = Ledger::open(home.0.join("ledger.db")).unwrap();
    ledger.add("empty scripted task", &[]).unwrap();
    let fake = common::temp_dir("m3-fake-keychain");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_iksa"))
        .arg("run")
        .env("IKSA_HOME", &home.0)
        .env("CHROMIUM", common::chromium())
        .env("IKSA_MOCK_KEYCHAIN", "1")
        .env("IKSA_KEYCHAIN_FAKE", &fake.0)
        .env("IKSA_NOTIFY", "off")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        home.0.join("extension/manifest.json").is_file(),
        "daemon did not install the bundled extension"
    );
    let host: Value = serde_json::from_slice(
        &std::fs::read(
            home.0
                .join("profile/NativeMessagingHosts/dev.iksa.host.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(host["path"], env!("CARGO_BIN_EXE_iksa"));
    let prefs = std::fs::read_to_string(home.0.join("profile/Default/Secure Preferences")).unwrap();
    assert!(
        prefs.contains(extension::EXTENSION_ID),
        "Chromium did not persist the bundled extension registration"
    );
}
