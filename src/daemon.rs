use crate::{
    audit,
    cas::Cas,
    config::Config,
    engine::Engine,
    extension,
    ledger::{Handoff, Ledger, State, Task},
    schedule, setup,
};
use anyhow::Context;
use std::{
    collections::HashSet,
    fs::File,
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
    time::{Duration, Instant},
};
use tokio::{
    signal::unix::{SignalKind, signal},
    task::{JoinError, JoinSet, LocalSet},
};

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
    let directory = extension::install(home, &Config::load(home)?, &std::env::current_exe()?)?;
    Engine::launch_with_extension(
        &path,
        &home.join("profile"),
        std::env::var("IKSA_MOCK_KEYCHAIN").as_deref() == Ok("1"),
        Some(&directory),
    )
    .map(|engine| engine.with_policy_home(home))
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
    // LocalSet owns all non-Send workers. Drop it before releasing the lock so
    // every worker and the final shared browser are gone on shutdown.
    let local = LocalSet::new();
    let result = local
        .run_until(async {
            tokio::select! {
                biased;
                _ = terminate.recv() => Ok(()),
                _ = interrupt.recv() => Ok(()),
                result = dispatch(home, &opts) => result,
            }
        })
        .await;
    drop(local);
    result
}
type Completion = (i64, anyhow::Result<Option<State>>);
fn complete(
    result: Result<Completion, JoinError>,
    running: &mut HashSet<i64>,
    background: &mut HashSet<i64>,
) -> anyhow::Result<()> {
    let (id, state) = result?;
    running.remove(&id);
    background.remove(&id);
    if let Some(state) = state? {
        report(id, &state);
    }
    Ok(())
}
fn is_background(task: &Task) -> bool {
    task.schedule_id.is_some()
        && task.accepted_ms.is_none()
        && task.human_started_ms.is_none()
        && task.approved_step.is_none()
}
async fn notifications(
    ledger: &Ledger,
    previous: &mut Vec<(i64, State, usize)>,
) -> anyhow::Result<()> {
    let tasks = ledger.list()?;
    let current = waiting(&tasks);
    if current != *previous {
        if let Some(message) = digest(&tasks) {
            notify(message).await;
        }
        *previous = current;
    }
    Ok(())
}
async fn dispatch(home: &Path, opts: &Opts) -> anyhow::Result<()> {
    let mut cfg = Config::load(home)?;
    let ledger = Ledger::open(home.join("ledger.db"))?;
    let cas = Cas::open(home.join("cas"))?;
    ledger.recover()?;
    audit::gc(&ledger, &cas, cfg.retention_days, audit::now_ms())?;
    let mut previous = waiting(&ledger.list()?);
    let mut engine: Option<Rc<Engine>> = None;
    let mut workers: JoinSet<Completion> = JoinSet::new();
    let mut running = HashSet::new();
    let mut background = HashSet::new();
    let mut last_busy = Instant::now();
    let mut last_gc = Instant::now();
    let mut next_tick = tokio::time::Instant::now();
    let mut ran_any = false;
    loop {
        while let Some(result) = workers.try_join_next() {
            complete(result, &mut running, &mut background)?;
            last_busy = Instant::now();
        }
        if running.is_empty() && engine.as_ref().is_some_and(|e| !e.is_available()) {
            engine = None;
        }
        let ticked = tokio::time::Instant::now() >= next_tick;
        if ticked {
            cfg = Config::load(home)?;
            let (_, removed) = ledger.fire_due_with_expiry(audit::now_ms(), |text, now| {
                schedule::parse(text)
                    .ok()
                    .and_then(|spec| schedule::next_fire_ms(&spec, now))
            })?;
            for id in removed {
                eprintln!("schedule {id} will never fire again; removed");
            }
            next_tick = tokio::time::Instant::now() + opts.poll;
        }
        let cap = cfg.max_agents as usize;
        // Bounded reads, plus a separate human window so a background backlog
        // cannot conceal work eligible for the reserved slot.
        let window = cap + 1 + running.len();
        let mut queued = ledger.queued(window)?;
        queued.extend(ledger.queued_humans(window)?);
        queued.sort_by_key(|t| t.id);
        queued.dedup_by_key(|t| t.id);
        for task in queued {
            if running.contains(&task.id) {
                continue;
            }
            let is_background = is_background(&task);
            let allowed = running.len() < cap
                || (!is_background && !background.is_empty() && running.len() < cap + 1);
            if !allowed {
                continue;
            }
            // Synchronous launch failures must not starve signal handling.
            tokio::task::yield_now().await;
            if engine.as_ref().is_some_and(|e| !e.is_available()) {
                break;
            }
            if engine.is_none() {
                match launch(home, opts) {
                    Ok(browser) => engine = Some(Rc::new(browser)),
                    Err(error) if opts.drain => return Err(error),
                    Err(error) => {
                        ledger.transition(task.id, State::Executing)?;
                        let state = State::Failed(format!("launching Chromium: {error:#}"));
                        ledger.transition(task.id, state.clone())?;
                        report(task.id, &state);
                        ran_any = true;
                        continue;
                    }
                }
            }
            let active = engine.as_ref().unwrap().clone();
            let path = home.to_owned();
            let config = cfg.clone();
            let id = task.id;
            running.insert(id);
            if is_background {
                background.insert(id);
            }
            workers.spawn_local(async move {
                let result = async {
                    let ledger = Ledger::open(path.join("ledger.db"))?;
                    let cas = Cas::open(path.join("cas"))?;
                    // Another worker may have broken the engine since admission.
                    // Leave this goal queued for the replacement browser.
                    if !active.is_available() {
                        return Ok(None);
                    }
                    active.run(&ledger, &cas, &config, id).await.map(Some)
                }
                .await;
                (id, result)
            });
            ran_any = true;
        }
        if ticked {
            notifications(&ledger, &mut previous).await?;
        }
        if running.is_empty() {
            let pending = ledger.next_queued()?.is_some();
            if opts.drain && !pending {
                notifications(&ledger, &mut previous).await?;
                if !ran_any {
                    println!("no queued tasks");
                }
                return Ok(());
            }
            if engine.as_ref().is_some_and(|e| !e.is_available()) {
                engine = None;
            }
            if last_busy.elapsed() >= opts.idle_close {
                engine = None;
            }
            if !pending && last_gc.elapsed() >= Duration::from_secs(3600) {
                audit::gc(&ledger, &cas, cfg.retention_days, audit::now_ms())?;
                last_gc = Instant::now();
            }
            // A synchronous failure batch or skipped broken-engine admissions
            // can leave more queued work without a completion to wake us.
            if opts.drain && pending {
                continue;
            }
        }
        tokio::select! {
            result = workers.join_next(), if !workers.is_empty() => {
                complete(result.expect("nonempty worker set"), &mut running, &mut background)?;
                last_busy = Instant::now();
            }
            _ = tokio::time::sleep_until(next_tick) => {},
        }
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
