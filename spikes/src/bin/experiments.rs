use anyhow::Result;
use chromiumoxide::cdp::browser_protocol::{browser::GetWindowForTargetParams, page::*, target::*};
use chromiumoxide::{Browser, Page};
use futures::StreamExt;
use std::{process::Command, time::Duration};
async fn eval(p: &Page, js: &str) -> Result<String> {
    Ok(p.evaluate(js).await?.into_value::<String>()?)
}
fn measure(label: &str) {
    let output = Command::new("python3")
        .arg("spikes/measure.py")
        .arg(label)
        .output()
        .unwrap();
    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
}
#[tokio::main]
async fn main() -> Result<()> {
    let mode = std::env::args().nth(1).unwrap_or("s2".into());
    let (mut browser, mut handler) = Browser::launch(iksa_spikes::config()?).await?;
    let task = tokio::spawn(async move {
        while let Some(x) = handler.next().await {
            if let Err(e) = x {
                eprintln!("handler {e}");
                break;
            }
        }
    });
    println!("MODE {mode}");
    let outcome: Result<()> = async {
        if mode == "s3" {
            let _p = browser.new_page("about:blank").await?;
            tokio::time::sleep(Duration::from_secs(60)).await;
            measure("idle60");
            return Ok(());
        }
        if mode == "s5" {
            let _p = browser.new_page("about:blank").await?;
            tokio::time::sleep(Duration::from_secs(10)).await;
            measure("idle");
        }
        let ctx = browser
            .create_browser_context(CreateBrowserContextParams::default())
            .await?;
        let mut params = CreateTargetParams::new("about:blank");
        params.hidden = Some(true);
        params.browser_context_id = Some(ctx.clone());
        if mode == "s6" {
            // Create and navigate visible target with browser-level commands before Page attachment.
            let id = browser
                .execute(CreateTargetParams::new("https://nowsecure.nl"))
                .await?
                .result
                .target_id;
            tokio::time::sleep(Duration::from_secs(30)).await;
            let p = browser.get_page(id).await?;
            observe(&p, "visible").await?;
        }
        if mode == "s2-direct" {
            let _p =
                tokio::time::timeout(Duration::from_secs(15), browser.new_page(params)).await??;
            return Ok(());
        }
        println!("creating hidden target via raw CDP");
        let target = browser.execute(params).await?.result.target_id;
        println!("raw_hidden_target={target:?}");
        println!(
            "raw_window={:?}",
            browser
                .execute(GetWindowForTargetParams {
                    target_id: Some(target.clone())
                })
                .await
                .map(|x| x.result)
        );
        println!(
            "targets={:?}",
            browser
                .execute(GetTargetsParams::default())
                .await
                .map(|x| x.result)
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
        let p = browser.get_page(target).await?;
        if mode == "s2" {
            println!(
                "window_before={:?}",
                browser
                    .execute(GetWindowForTargetParams {
                        target_id: Some(p.target_id().clone())
                    })
                    .await
                    .map(|x| x.result)
            );
            p.goto("http://127.0.0.1:8765/form.html").await?;
            println!("content={}", p.content().await?);
            p.find_element("#q")
                .await?
                .click()
                .await?
                .type_str("spike typed")
                .await?;
            p.find_element("#go").await?.click().await?;
            println!(
                "interaction={}",
                eval(&p, "document.querySelector('#out').textContent").await?
            );
            let mut frames = p.event_listener::<EventScreencastFrame>().await?;
            println!(
                "screencast_start={:?}",
                p.execute(StartScreencastParams::default())
                    .await
                    .map(|x| x.result)
            );
            let frame = tokio::time::timeout(Duration::from_secs(5), frames.next()).await;
            println!("screencast_frame={}", matches!(frame, Ok(Some(_))));
            println!(
                "activate={:?}",
                browser
                    .execute(ActivateTargetParams::new(p.target_id().clone()))
                    .await
                    .map(|x| x.result)
            );
            println!(
                "window_after={:?}",
                browser
                    .execute(GetWindowForTargetParams {
                        target_id: Some(p.target_id().clone())
                    })
                    .await
                    .map(|x| x.result)
            );
            eval(
                &p,
                "document.cookie='spike=same-context; path=/'; document.cookie",
            )
            .await?;
            let mut v = CreateTargetParams::new("http://127.0.0.1:8765/form.html");
            v.browser_context_id = Some(ctx.clone());
            let visible = browser.new_page(v).await?;
            println!(
                "handoff_cookie={}",
                eval(&visible, "document.cookie").await?
            );
            println!(
                "handoff_window={:?}",
                browser
                    .execute(GetWindowForTargetParams {
                        target_id: Some(visible.target_id().clone())
                    })
                    .await
                    .map(|x| x.result)
            );
        }
        if mode == "s5" {
            p.goto("https://en.wikipedia.org/wiki/Rust_(programming_language)")
                .await?;
            tokio::time::sleep(Duration::from_secs(15)).await;
            println!("agent_content_bytes={}", p.content().await?.len());
            measure("agent");
        }
        if mode == "s6" {
            println!(
                "hidden_navigation={:?}",
                p.goto("https://nowsecure.nl").await.map(|_| ())
            );
            tokio::time::sleep(Duration::from_secs(30)).await;
            observe(&p, "hidden").await?;
        }
        browser.dispose_browser_context(ctx).await?;
        Ok(())
    }
    .await;
    println!("RESULT={outcome:?}");
    browser.close().await?;
    task.await?;
    outcome
}
async fn observe(p: &Page, label: &str) -> Result<()> {
    let html = p.content().await?;
    println!(
        "{label}: title={} bytes={} Just_a_moment={} turnstile={} hcaptcha={} recaptcha={}",
        eval(p, "document.title").await?,
        html.len(),
        html.contains("Just a moment"),
        html.contains("challenges.cloudflare.com/turnstile")
            || html.contains("class=\"cf-turnstile\""),
        html.contains("hcaptcha.com/") || html.contains("class=\"h-captcha\""),
        html.contains("google.com/recaptcha") || html.contains("class=\"g-recaptcha\"")
    );
    std::fs::write(format!(".spike-home/logs/s6-{label}.html"), html)?;
    Ok(())
}
