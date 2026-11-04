use crate::{
    audit,
    cas::Cas,
    config::Config,
    engine::Engine,
    ledger::{Handoff, Ledger, State, Task},
    schedule, setup,
};
use anyhow::Context;
use std::{
    fs::File,
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
use tokio::signal::unix::{SignalKind, signal};

pub const POLL: Duration = Duration::from_secs(2);
pub const IDLE_CLOSE: Duration = Duration::from_secs(60);
#[derive(Clone, Debug)]
pub struct Opts {
    pub drain: bool,
    pub poll: Duration,
    pub idle_close: Duration,
    pub chromium: Option<PathBuf>,
}
impl Default for Opts {
    fn default() -> Self {
        Self {
            drain: false,
            poll: POLL,
            idle_close: IDLE_CLOSE,
            chromium: None,
        }
    }
}
pub fn lock(home: &Path) -> anyhow::Result<File> {
    let file = File::create(home.join("run.lock"))?;
    anyhow::ensure!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "another iksa run is active"
    );
    Ok(file)
}
pub fn digest(tasks: &[Task]) -> Option<String> {
    let (mut approvals, mut challenges, mut suggestions) = (0, 0, 0);
    for task in tasks {
        match task.state {
            State::AwaitingHandoff(Handoff::Approval) => approvals += 1,
            State::AwaitingHandoff(Handoff::Challenge(_)) => challenges += 1,
            State::Suggested => suggestions += 1,
            _ => {}
        }
    }
    let parts: Vec<_> = [
        (approvals, "approval"),
        (challenges, "challenge"),
        (suggestions, "suggestion"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, noun)| format!("{count} {noun}{}", if count == 1 { "" } else { "s" }))
    .collect();
    (!parts.is_empty()).then(|| format!("waiting on you: {}", parts.join(", ")))
}
fn waiting(tasks: &[Task]) -> Vec<(i64, State, usize)> {
    tasks
        .iter()
        .filter(|t| matches!(t.state, State::Suggested | State::AwaitingHandoff(_)))
        .map(|t| (t.id, t.state.clone(), t.cursor))
        .collect()
}
async fn notify(message: String) {
    if std::env::var("IKSA_NOTIFY").as_deref() == Ok("off") {
        return;
    }
    let _ = tokio::task::spawn_blocking(move || {
        Command::new("osascript")
            .args([
                "-e",
                &format!("display notification \"{message}\" with title \"iksa\""),
            ])
            .output()
    })
    .await;
}
fn launch(home: &Path, opts: &Opts) -> anyhow::Result<Engine> {
    let path = opts
        .chromium
        .clone()
        .or_else(|| setup::chromium_path(home))
        .context("Chromium is not installed; run iksa setup")?;
    Engine::launch(
        &path,
        &home.join("profile"),
        std::env::var("IKSA_MOCK_KEYCHAIN").as_deref() == Ok("1"),
    )
    .with_context(|| format!("launching Chromium {}", path.display()))
}
fn report(id: i64, state: &State) {
    println!("{id}\t{state:?}");
    match state {
        State::AwaitingHandoff(Handoff::Approval) => eprintln!("hint: iksa approve {id}"),
        State::AwaitingHandoff(Handoff::Challenge(_)) => {
            eprintln!("hint: solve the challenge, then iksa resume {id}")
        }
        _ => {}
    }
}
pub async fn run(home: &Path, opts: Opts) -> anyhow::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(home)?;
    std::fs::set_permissions(home, std::fs::Permissions::from_mode(0o700))?;
    let _lock = lock(home)?;
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    // Dropping dispatch cancels the in-flight step and closes its owned browser
    // before the run lock is released. Executing tasks are recovered next start.
    tokio::select! {
        biased;
        _ = terminate.recv() => Ok(()),
        _ = interrupt.recv() => Ok(()),
        result = dispatch(home, &opts) => result,
    }
}
async fn dispatch(home: &Path, opts: &Opts) -> anyhow::Result<()> {
    let cfg = Config::load(home)?;
    let ledger = Ledger::open(home.join("ledger.db"))?;
    let cas = Cas::open(home.join("cas"))?;
    ledger.recover()?;
    audit::gc(&ledger, &cas, cfg.retention_days, audit::now_ms())?;
    let mut previous = waiting(&ledger.list()?);
    let mut engine: Option<Engine> = None;
    let mut last_busy = Instant::now();
    let mut last_gc = Instant::now();
    let mut ran_any = false;
    loop {
        let (_, removed) = ledger.fire_due_with_expiry(audit::now_ms(), |text, now| {
            schedule::parse(text)
                .ok()
                .and_then(|spec| schedule::next_fire_ms(&spec, now))
        })?;
        for id in removed {
            eprintln!("schedule {id} will never fire again; removed");
        }
        let mut ran_tick = false;
        while let Some(task) = ledger.next_queued()? {
            // Launch failures can be entirely synchronous; let shutdown win
            // between tasks even when this batch has no browser I/O to await.
            tokio::task::yield_now().await;
            let cfg = Config::load(home)?;
            if engine.is_none() {
                match launch(home, opts) {
                    Ok(browser) => engine = Some(browser),
                    Err(error) if opts.drain => return Err(error),
                    Err(error) => {
                        ledger.transition(task.id, State::Executing)?;
                        let state = State::Failed(format!("launching Chromium: {error:#}"));
                        ledger.transition(task.id, state.clone())?;
                        report(task.id, &state);
                        ran_any = true;
                        ran_tick = true;
                        continue;
                    }
                }
            }
            let active = engine.as_ref().unwrap();
            let state = active.run(&ledger, &cas, &cfg, task.id).await?;
            report(task.id, &state);
            if !active.is_available() {
                engine = None;
            }
            last_busy = Instant::now();
            ran_any = true;
            ran_tick = true;
        }
        let tasks = ledger.list()?;
        let current = waiting(&tasks);
        if current != previous {
            if let Some(message) = digest(&tasks) {
                notify(message).await;
            }
            previous = current;
        }
        if last_busy.elapsed() >= opts.idle_close {
            engine = None;
        }
        if !ran_tick && last_gc.elapsed() >= Duration::from_secs(3600) {
            let cfg = Config::load(home)?;
            audit::gc(&ledger, &cas, cfg.retention_days, audit::now_ms())?;
            last_gc = Instant::now();
        }
        if opts.drain {
            if !ran_tick {
                if !ran_any {
                    println!("no queued tasks");
                }
                return Ok(());
            }
            continue;
        }
        tokio::time::sleep(opts.poll).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{challenge::ChallengeKind, testutil::temp_dir};
    #[test]
    fn digest_counts() {
        let dir = temp_dir("digest-counts");
        let ledger = Ledger::open(dir.0.join("ledger.db")).unwrap();
        assert_eq!(digest(&ledger.list().unwrap()), None);
        for n in 0..2 {
            let id = ledger.add(&format!("secret approval {n}"), &[]).unwrap();
            ledger.transition(id, State::Executing).unwrap();
            ledger
                .transition(id, State::AwaitingHandoff(Handoff::Approval))
                .unwrap();
            if n == 0 {
                assert_eq!(
                    digest(&ledger.list().unwrap()).as_deref(),
                    Some("waiting on you: 1 approval")
                );
            }
        }
        let id = ledger.add("secret challenge", &[]).unwrap();
        ledger.transition(id, State::Executing).unwrap();
        ledger
            .transition(
                id,
                State::AwaitingHandoff(Handoff::Challenge(ChallengeKind::Turnstile)),
            )
            .unwrap();
        for n in 0..3 {
            ledger.suggest(&format!("secret suggestion {n}")).unwrap();
        }
        assert_eq!(
            digest(&ledger.list().unwrap()).as_deref(),
            Some("waiting on you: 2 approvals, 1 challenge, 3 suggestions")
        );
    }
}
