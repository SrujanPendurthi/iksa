use anyhow::Result;
use chromiumoxide::Browser;
use chromiumoxide::cdp::browser_protocol::browser::GetVersionParams;
use futures::StreamExt;
#[tokio::main]
async fn main() -> Result<()> {
    let config = iksa_spikes::config()?;
    if std::env::args().any(|x| x == "--verify-env") {
        let output = std::process::Command::new("/usr/bin/env")
            .envs(config.process_envs.as_ref().unwrap())
            .output()?;
        for line in String::from_utf8(output.stdout)?.lines() {
            if ["HOME=", "CFFIXED_USER_HOME=", "TMPDIR="]
                .iter()
                .any(|key| line.starts_with(key))
            {
                println!("verified_child_{line}");
            }
        }
    }
    let (mut browser, mut handler) = Browser::launch(config).await?;
    let task = tokio::spawn(async move {
        while let Some(event) = handler.next().await {
            if let Err(e) = event {
                eprintln!("handler: {e}");
                break;
            }
        }
    });
    println!(
        "{:?}",
        browser.execute(GetVersionParams::default()).await?.result
    );
    browser.close().await?;
    task.await?;
    Ok(())
}
