use anyhow::Result;
use chromiumoxide::Browser;
use futures::StreamExt;
#[tokio::main]
async fn main() -> Result<()> {
    let (mut browser, mut handler) = Browser::launch(iksa_spikes::config()?).await?;
    let task = tokio::spawn(async move {
        while let Some(x) = handler.next().await {
            if let Err(e) = x {
                eprintln!("{e}");
                break;
            }
        }
    });
    let id = std::fs::read_to_string(".spike-home/ext-id")?;
    let p = browser
        .new_page(format!("chrome-extension://{id}/test.html"))
        .await?;
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    println!(
        "native={:?}",
        p.evaluate("document.body.innerText")
            .await?
            .into_value::<String>()?
    );
    if std::env::var_os("SPIKE_UI_HOLD_INITIAL").is_some() {
        println!("initial_ui_hold=15s");
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    }
    let ui = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let settings = browser.new_page("chrome://settings/searchEngines").await?;
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let text = settings.evaluate(r#"(() => {let out=[];function walk(n){if(n.nodeType===3){let t=n.textContent.trim();if(t)out.push(t);return;}if(['STYLE','SCRIPT'].includes(n.nodeName))return;if(n.shadowRoot)walk(n.shadowRoot);for(let ch of n.childNodes)walk(ch);}walk(document.body);return out.join('\n');})()"#).await?.into_value::<String>()?;
        std::fs::write(".spike-home/logs/s4-rendered-settings.txt", &text)?;
        println!("rendered_settings={text}");
        settings.save_screenshot(chromiumoxide::page::ScreenshotParams::default(), ".spike-home/logs/s4-settings.png").await?;
        Ok::<(), anyhow::Error>(())
    }).await;
    println!("settings_ui={ui:?}");
    if std::env::var_os("SPIKE_UI_HOLD").is_some() {
        println!("ui_hold=30s");
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    }
    browser.close().await?;
    task.await?;
    Ok(())
}
