use anyhow::Context;
use iksa::{
    action::Action,
    audit,
    cas::Cas,
    config::Config,
    engine::Engine,
    keychain,
    ledger::{Handoff, Ledger, State},
    setup,
};
use serde::Deserialize;
use std::{
    io::Read,
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, PermissionsExt},
    },
    path::PathBuf,
};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskInput {
    title: String,
    steps: Vec<Action>,
}
const USAGE: &str = "usage: iksa setup | add <task.json|-> | ask <goal…> | key set | secret set <name> | run | list | inbox | accept <id> | dismiss <id> | approve <id> | resume <id> | report <id> | gc";
#[tokio::main]
async fn main() {
    let result = execute().await;
    match result {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
    }
}

async fn execute() -> anyhow::Result<i32> {
    let home = if let Some(h) = std::env::var_os("IKSA_HOME") {
        PathBuf::from(h)
    } else if let Some(h) = std::env::var_os("HOME") {
        PathBuf::from(h).join(".iksa")
    } else {
        anyhow::bail!("set HOME or IKSA_HOME");
    };
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&home)?;
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let open = || -> anyhow::Result<(Config, Ledger, Cas)> {
        Ok((
            Config::load(&home)?,
            Ledger::open(home.join("ledger.db"))?,
            Cas::open(home.join("cas"))?,
        ))
    };
    let id = |s: &str| s.parse::<i64>().ok();
    match args[..] {
        ["key", "set"] => keychain::set_interactive("iksa", "anthropic")?,
        ["secret", "set", name] => {
            anyhow::ensure!(
                keychain::valid_name(name),
                "invalid secret name: use letters, digits, _ or -"
            );
            keychain::set_interactive("iksa-secret", name)?;
        }
        ["setup"] => println!("{}", setup::install(&home)?.display()),
        ["ask", ref words @ ..] if words.iter().any(|word| !word.trim().is_empty()) => {
            let (cfg, ledger, _) = open()?;
            anyhow::ensure!(cfg.model.is_some(), "configure \"model\" in config.json");
            let goal = words.join(" ");
            println!("{}", ledger.add_goal(&goal, &goal)?);
        }
        ["add", path] => {
            let (_, ledger, _) = open()?;
            let mut json = String::new();
            if path == "-" {
                if unsafe { libc::isatty(0) } == 1 {
                    eprintln!("reading task JSON from stdin (Ctrl-D to finish)…");
                }
                std::io::stdin().read_to_string(&mut json)?;
                anyhow::ensure!(!json.trim().is_empty(), "no task JSON on stdin");
            } else {
                json = std::fs::read_to_string(path).with_context(|| path.to_owned())?;
            }
            let task: TaskInput = serde_json::from_str(&json).context("invalid task JSON")?;
            let id = ledger.add(&task.title, &task.steps)?;
            if path != "-"
                && task
                    .steps
                    .iter()
                    .any(|s| matches!(s, Action::Type { secret: true, .. }))
            {
                eprintln!(
                    "warning: {path} contains a secret; delete it — iksa keeps its own copy until the task finishes"
                );
            }
            println!("{id}");
        }
        ["list"] => {
            for t in open()?.1.list()? {
                println!(
                    "{}\t{}{}\t{:?}",
                    t.id,
                    if t.goal.is_some() { "[goal] " } else { "" },
                    t.title,
                    t.state
                );
            }
        }
        ["inbox"] => {
            let suggestions: Vec<_> = open()?
                .1
                .list()?
                .into_iter()
                .filter(|task| task.state == State::Suggested)
                .collect();
            if suggestions.is_empty() {
                println!("inbox empty");
            }
            for task in suggestions {
                println!(
                    "{}\t{}",
                    task.id,
                    task.goal.as_deref().unwrap_or(&task.title)
                );
            }
        }
        ["accept", n] if id(n).is_some() => {
            let (cfg, ledger, _) = open()?;
            anyhow::ensure!(cfg.model.is_some(), "configure \"model\" in config.json");
            ledger.accept(id(n).unwrap())?;
        }
        ["dismiss", n] if id(n).is_some() => open()?.1.dismiss(id(n).unwrap())?,
        ["approve", n] if id(n).is_some() => open()?.1.approve(id(n).unwrap())?,
        ["resume", n] if id(n).is_some() => open()?.1.resume(id(n).unwrap())?,
        ["report", n] if id(n).is_some() => {
            let (_, ledger, cas) = open()?;
            let n = id(n).unwrap();
            print!(
                "{}",
                audit::render(&ledger.get(n)?, &ledger.records(n)?, &cas)?
            );
        }
        ["gc"] => {
            let (cfg, ledger, cas) = open()?;
            let (tasks, blobs) = audit::gc(&ledger, &cas, cfg.retention_days, audit::now_ms())?;
            println!("removed {tasks} tasks and {blobs} blobs");
        }
        ["run"] => {
            // One runner per data dir: a second Chromium on the same profile would hand off and exit.
            let lock = std::fs::File::create(home.join("run.lock"))?;
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                anyhow::bail!("another iksa run is active");
            }
            let (cfg, ledger, cas) = open()?;
            ledger.recover()?;
            audit::gc(&ledger, &cas, cfg.retention_days, audit::now_ms())?;
            if ledger.next_queued()?.is_none() {
                println!("no queued tasks");
                return Ok(0);
            }
            let chromium =
                setup::chromium_path(&home).context("Chromium is not installed; run iksa setup")?;
            let engine = Engine::launch(
                &chromium,
                &home.join("profile"),
                std::env::var("IKSA_MOCK_KEYCHAIN").as_deref() == Ok("1"),
            )
            .with_context(|| format!("launching Chromium {}", chromium.display()))?;
            while let Some(t) = ledger.next_queued()? {
                let state = engine.run(&ledger, &cas, &cfg, t.id).await?;
                println!("{}\t{state:?}", t.id);
                match state {
                    State::AwaitingHandoff(Handoff::Approval) => {
                        eprintln!("hint: iksa approve {}", t.id)
                    }
                    State::AwaitingHandoff(Handoff::Challenge(_)) => {
                        eprintln!("hint: solve the challenge, then iksa resume {}", t.id)
                    }
                    _ => {}
                }
            }
        }
        _ => {
            eprintln!("{USAGE}");
            return Ok(2);
        }
    }
    Ok(0)
}
