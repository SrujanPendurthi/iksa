use anyhow::Context;
use iksa::{
    action::Action,
    audit,
    cas::Cas,
    config::Config,
    daemon::{self, Opts},
    keychain,
    ledger::{Handoff, Ledger, State, waiting_on},
    schedule, setup,
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
const USAGE: &str = "usage: iksa setup | add <task.json|-> | ask <goal…> | key set | secret set <name> | schedule add [--auto] <spec> <goal…> | schedule list | schedule set <id> auto|suggest | schedule rm <id> | grant <domain> [--schedule <id>] [--days N] | grant list | grant rm <id> | daemon | run | list | inbox | accept <id> | dismiss <id> | approve <id> | resume <id> | report <id> | gc";
fn local_time(ms: u64) -> String {
    let c = schedule::local(ms);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02} local",
        c.year, c.month, c.day, c.hour, c.minute
    )
}
fn grant_args(words: &[&str]) -> Option<(String, Option<i64>, u32)> {
    if matches!(*words.first()?, "list" | "rm") {
        return None;
    }
    let domain = words.first()?.trim_end_matches('.').to_ascii_lowercase();
    let valid_domain = !domain.is_empty()
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        });
    if !valid_domain {
        return None;
    }
    let mut schedule = None;
    let mut days = None;
    let mut flags = words[1..].chunks_exact(2);
    for flag in &mut flags {
        match flag {
            ["--schedule", value] if schedule.is_none() => {
                schedule = Some(value.parse::<i64>().ok().filter(|id| *id > 0)?);
            }
            ["--days", value] if days.is_none() => {
                days = Some(value.parse::<u32>().ok().filter(|n| (1..=30).contains(n))?);
            }
            _ => return None,
        }
    }
    if !flags.remainder().is_empty() {
        return None;
    }
    Some((domain, schedule, days.unwrap_or(7)))
}

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
        ["schedule", "add", ref words @ ..] => {
            let (auto, words) = if words.first() == Some(&"--auto") {
                (true, &words[1..])
            } else {
                (false, &words[..])
            };
            if words.len() < 2 || !words[1..].iter().any(|word| !word.trim().is_empty()) {
                eprintln!("{USAGE}");
                return Ok(2);
            }
            let (cfg, ledger, _) = open()?;
            anyhow::ensure!(cfg.model.is_some(), "configure \"model\" in config.json");
            let spec = match schedule::parse(words[0]) {
                Ok(spec) => spec,
                Err(reason) => {
                    eprintln!("error: {reason}");
                    return Ok(2);
                }
            };
            let Some(next) = schedule::next_fire_ms(&spec, audit::now_ms()) else {
                eprintln!(
                    "error: schedule {:?} never fires or timestamp overflows",
                    words[0]
                );
                return Ok(2);
            };
            let id = ledger.add_schedule(words[0], &words[1..].join(" "), auto, next)?;
            println!("{id}\tnext {}", local_time(next));
        }
        ["schedule", "list"] => {
            for s in open()?.1.schedules()? {
                println!(
                    "{}\t{}\t{}\t{}\t{}/{} accepted\t{}",
                    s.id,
                    s.spec,
                    local_time(s.next_ms),
                    if s.auto { "auto" } else { "suggest" },
                    s.accepted,
                    u64::from(s.accepted) + u64::from(s.dismissed),
                    s.goal
                );
            }
        }
        ["schedule", "set", n, mode] if id(n).is_some() && matches!(mode, "auto" | "suggest") => {
            open()?
                .1
                .set_schedule_auto(id(n).unwrap(), mode == "auto")?;
        }
        ["schedule", "rm", n] if id(n).is_some() => open()?.1.remove_schedule(id(n).unwrap())?,
        ["grant", "list"] => {
            for grant in open()?.1.grants()? {
                println!(
                    "{}\t{}\tschedule {}\texpires {}",
                    grant.id,
                    grant.domain,
                    grant
                        .schedule_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "any".into()),
                    local_time(grant.expires_ms)
                );
            }
        }
        ["grant", "rm", n] if id(n).is_some() => open()?.1.remove_grant(id(n).unwrap())?,
        ["grant", ref words @ ..] => {
            let Some((domain, schedule, days)) = grant_args(words) else {
                eprintln!("{USAGE}");
                return Ok(2);
            };
            let (cfg, ledger, _) = open()?;
            let expires = audit::now_ms()
                .checked_add(u64::from(days) * 86_400_000)
                .context("grant expiry overflows")?;
            let id = ledger.add_grant(&domain, schedule, expires, &cfg.policy_fingerprint())?;
            println!("{id}\texpires {}", local_time(expires));
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
            let (_, ledger, _) = open()?;
            for t in ledger.list()? {
                let mut waiting = waiting_on(&t);
                if t.state == State::AwaitingHandoff(Handoff::Approval) {
                    if let Some(record) = ledger.records(t.id)?.last().filter(|r| {
                        r.decision == "planner"
                            && r.outcome.starts_with("blocked: ")
                            && r.outcome.ends_with(" is on never_send")
                    }) {
                        waiting = Some(record.outcome.clone());
                    }
                }
                println!(
                    "{}\t{}{}\t{:?}{}",
                    t.id,
                    if t.goal.is_some() { "[goal] " } else { "" },
                    t.title,
                    t.state,
                    waiting.map(|w| format!("\t{w}")).unwrap_or_default()
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
        ["daemon"] => daemon::run(&home, Opts::default()).await?,
        ["run"] => {
            daemon::run(
                &home,
                Opts {
                    drain: true,
                    ..Opts::default()
                },
            )
            .await?
        }
        _ => {
            eprintln!("{USAGE}");
            return Ok(2);
        }
    }
    Ok(0)
}
