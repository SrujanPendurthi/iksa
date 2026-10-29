use anyhow::Context;
use iksa::{
    action::Action, audit, cas::Cas, config::Config, engine::Engine, ledger::Ledger, setup,
};
use serde::Deserialize;
use std::{
    io::Read,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::PathBuf,
};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskInput {
    title: String,
    steps: Vec<Action>,
}
const USAGE: &str = "usage: iksa setup | add <task.json|-> | run | list | approve <id> | resume <id> | report <id> | gc";
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
    let home = std::env::var_os("IKSA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".iksa")
        });
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&home)?;
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let valid = matches!(args.as_slice(),[c] if matches!(c.as_str(),"setup"|"run"|"list"|"gc"))
        || matches!(args.as_slice(),[c,_] if matches!(c.as_str(),"add"|"approve"|"resume"|"report"));
    if !valid
        || matches!(args.as_slice(),[c,id] if matches!(c.as_str(),"approve"|"resume"|"report") && id.parse::<i64>().is_err())
    {
        eprintln!("{USAGE}");
        return Ok(2);
    }
    if args[0] == "setup" {
        println!("{}", setup::install(&home)?.display());
        return Ok(0);
    }
    let cfg = Config::load(&home)?;
    let ledger = Ledger::open(home.join("ledger.db"))?;
    let cas = Cas::open(home.join("cas"))?;
    match args[0].as_str() {
        "add" => {
            let path = &args[1];
            let mut json = String::new();
            if path == "-" {
                std::io::stdin().read_to_string(&mut json)?;
            } else {
                json = std::fs::read_to_string(path).with_context(|| path.clone())?;
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
        "list" => {
            for t in ledger.list()? {
                println!("{}\t{}\t{:?}", t.id, t.title, t.state);
            }
        }
        "approve" => ledger.approve(args[1].parse()?)?,
        "resume" => ledger.resume(args[1].parse()?)?,
        "report" => {
            let id = args[1].parse()?;
            print!(
                "{}",
                audit::render(&ledger.get(id)?, &ledger.records(id)?, &cas)?
            );
        }
        "gc" => {
            let (tasks, blobs) = audit::gc(&ledger, &cas, cfg.retention_days, audit::now_ms())?;
            println!("removed {tasks} tasks and {blobs} blobs");
        }
        "run" => {
            ledger.recover()?;
            audit::gc(&ledger, &cas, cfg.retention_days, audit::now_ms())?;
            let chromium =
                setup::chromium_path(&home).context("Chromium is not installed; run iksa setup")?;
            let engine = Engine::launch(
                &chromium,
                &home.join("profile"),
                std::env::var("IKSA_MOCK_KEYCHAIN").as_deref() == Ok("1"),
            )
            .with_context(|| format!("launching Chromium {}", chromium.display()))?;
            while let Some(t) = ledger.next_queued()? {
                engine.run(&ledger, &cas, &cfg, t.id).await?;
            }
        }
        _ => unreachable!(),
    }
    Ok::<i32, anyhow::Error>(0)
}
