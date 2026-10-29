use crate::{
    action::Action,
    cas::Cas,
    gate::Risk,
    ledger::{Ledger, State, Task},
    snapshot::{self, invalid_data},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs, io,
    time::{SystemTime, UNIX_EPOCH},
};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepRecord {
    pub step: usize,
    pub action: Action,
    pub decision: String,
    pub after: Option<String>,
    pub outcome: String,
    pub at_ms: u64,
}
impl StepRecord {
    pub fn new(
        step: usize,
        action: &Action,
        decision: String,
        after: Option<String>,
        outcome: String,
    ) -> Self {
        Self {
            step,
            action: action.redacted(),
            decision,
            after: if action.risk() == Risk::Financial {
                None
            } else {
                after
            },
            outcome,
            at_ms: now_ms(),
        }
    }
}
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn tree_hash(hex: &str) -> io::Result<blake3::Hash> {
    blake3::Hash::from_hex(hex).map_err(invalid_data)
}
pub fn render(task: &Task, records: &[StepRecord], cas: &Cas) -> io::Result<String> {
    let mut text = format!(
        "# Task {}: {}\nState: {:?}\n",
        task.id, task.title, task.state
    );
    let mut previous = HashSet::new();
    for record in records {
        let action = serde_json::to_value(&record.action).map_err(invalid_data)?;
        let op = action["op"]
            .as_str()
            .ok_or_else(|| invalid_data("missing action op"))?;
        text.push_str(&format!(
            "\n## Step {} — {}\n{}\nDecision: {}\nOutcome: {}\n",
            record.step,
            op,
            action,
            record.decision,
            record.outcome.chars().take(2000).collect::<String>()
        ));
        if let Some(after) = &record.after {
            let tree = tree_hash(after)?;
            let chunks = match snapshot::chunk_hashes(cas, &tree) {
                Ok(chunks) => chunks,
                Err(error) => {
                    text.push_str(&format!("DOM: unavailable ({error})\n"));
                    previous.clear();
                    continue;
                }
            };
            let changed = chunks
                .iter()
                .filter(|hash| !previous.contains(*hash))
                .count();
            text.push_str(&format!("DOM: {changed}/{} chunks changed\n", chunks.len()));
            previous = chunks.into_iter().collect();
        } else {
            text.push_str("DOM: not stored\n");
            previous.clear();
        }
    }
    Ok(text)
}
/// Run before execution begins; callers must not claim tasks concurrently with GC.
pub fn gc(
    ledger: &Ledger,
    cas: &Cas,
    retention_days: u32,
    now_ms: u64,
) -> anyhow::Result<(usize, usize)> {
    let tasks = ledger.list()?;
    anyhow::ensure!(
        !tasks.iter().any(|task| task.state == State::Executing),
        "cannot run GC while a task is Executing"
    );
    let cutoff = now_ms.saturating_sub(u64::from(retention_days).saturating_mul(86_400_000));
    let mut removed_tasks = 0;
    for task in tasks {
        if matches!(task.state, State::Committed | State::Failed(_))
            && task.finished_ms.is_some_and(|finished| finished < cutoff)
        {
            ledger.delete_task(task.id)?;
            removed_tasks += 1;
        }
    }
    let mut reachable = HashSet::new();
    for task in ledger.list()? {
        for record in ledger.records(task.id)? {
            if let Some(after) = record.after {
                let tree = tree_hash(&after)?;
                if let Ok(chunks) = snapshot::chunk_hashes(cas, &tree) {
                    reachable.insert(tree);
                    reachable.extend(chunks);
                }
            }
        }
    }
    let mut removed_blobs = 0;
    for hash in cas.all()? {
        if !reachable.contains(&hash) {
            cas.delete(&hash)?;
            removed_blobs += 1;
        }
    }
    let now = UNIX_EPOCH + std::time::Duration::from_millis(now_ms);
    for entry in fs::read_dir(cas.root())? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with("tmp-")
            && entry.file_type()?.is_file()
            && now
                .duration_since(entry.metadata()?.modified()?)
                .is_ok_and(|age| age > std::time::Duration::from_secs(3600))
        {
            fs::remove_file(entry.path())?;
        }
    }
    Ok((removed_tasks, removed_blobs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        gate::Risk,
        ledger::State,
        snapshot,
        testutil::{TempDir, temp_dir},
    };
    use std::{
        fs,
        time::{Duration, UNIX_EPOCH},
    };
    fn setup(label: &str) -> (TempDir, Ledger, Cas) {
        let dir = temp_dir(label);
        let ledger = Ledger::open(dir.0.join("ledger.db")).unwrap();
        let cas = Cas::open(dir.0.join("cas")).unwrap();
        (dir, ledger, cas)
    }
    fn action() -> Action {
        Action::Extract {
            selector: "h1".into(),
        }
    }
    fn record(step: usize, after: Option<String>) -> StepRecord {
        StepRecord::new(step, &action(), "auto".into(), after, "ok".into())
    }
    #[test]
    fn report_lists_every_step() {
        let (_dir, ledger, cas) = setup("audit-report");
        let id = ledger.add("example", &[action(), action()]).unwrap();
        let mut second = record(1, None);
        second.outcome = "😀".repeat(2001);
        let text = render(&ledger.get(id).unwrap(), &[record(0, None), second], &cas).unwrap();
        assert!(text.starts_with(&format!("# Task {id}: example\n")));
        assert!(text.contains("Queued"));
        assert!(text.contains("## Step 0 — extract"));
        assert!(text.contains("## Step 1 — extract"));
        assert!(text.contains("\"selector\":\"h1\""));
        assert!(text.contains("Decision: auto"));
        assert!(text.contains("Outcome: ok"));
        assert_eq!(text.matches('😀').count(), 2000);
        assert_eq!(text.matches("DOM: not stored").count(), 2);
        let bad = record(0, Some("bad hash".into()));
        assert_eq!(
            render(&ledger.get(id).unwrap(), &[bad], &cas)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
    fn assert_secret_erased(terminal: State) {
        let (dir, ledger, cas) = setup("audit-secret");
        let path = dir.0.join("ledger.db");
        let typed = Action::Type {
            selector: "#password".into(),
            text: "hunter2".into(),
            secret: true,
            risk: Risk::Write,
        };
        let id = ledger.add("secret", &[typed.clone()]).unwrap();
        assert_eq!(ledger.get(id).unwrap().steps, vec![typed.clone()]);
        ledger
            .record_step(
                id,
                &StepRecord::new(0, &typed, "approved".into(), None, "typed".into()),
            )
            .unwrap();
        let records = ledger.records(id).unwrap();
        assert!(!serde_json::to_string(&records).unwrap().contains("hunter2"));
        assert!(
            !render(&ledger.get(id).unwrap(), &records, &cas)
                .unwrap()
                .contains("hunter2")
        );
        ledger.transition(id, State::Executing).unwrap();
        ledger.transition(id, terminal).unwrap();
        let task = ledger.get(id).unwrap();
        drop(ledger);
        for file in [&path, &dir.0.join("ledger.db-wal")] {
            match fs::read(file) {
                Ok(bytes) => assert!(
                    !bytes.windows(b"hunter2".len()).any(|w| w == b"hunter2"),
                    "secret in {}",
                    file.display()
                ),
                Err(error) if file != &path && error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => panic!("{}: {error}", file.display()),
            }
        }
        assert_eq!(task.steps, vec![typed.redacted()]);
        assert!(serde_json::to_string(&task.steps).unwrap().contains("••••"));
        let ledger = Ledger::open(&path).unwrap();
        assert_eq!(ledger.get(id).unwrap().steps, task.steps);
        assert!(
            !serde_json::to_string(&ledger.records(id).unwrap())
                .unwrap()
                .contains("hunter2")
        );
        assert!(!render(&task, &records, &cas).unwrap().contains("hunter2"));
    }
    #[test]
    fn secret_never_rendered() {
        assert_secret_erased(State::Committed);
    }
    #[test]
    fn secret_erased_on_failed() {
        assert_secret_erased(State::Failed("x".into()));
    }
    #[test]
    fn rerun_step_keeps_both_records() {
        let (_dir, ledger, cas) = setup("audit-rerun");
        let id = ledger.add("rerun", &[action()]).unwrap();
        for outcome in ["challenge", "ok"] {
            let mut rec = record(0, None);
            rec.outcome = outcome.into();
            ledger.record_step(id, &rec).unwrap();
        }
        let records = ledger.records(id).unwrap();
        assert_eq!(
            records
                .iter()
                .map(|r| r.outcome.as_str())
                .collect::<Vec<_>>(),
            vec!["challenge", "ok"]
        );
        assert_eq!(
            render(&ledger.get(id).unwrap(), &records, &cas)
                .unwrap()
                .matches("## Step 0")
                .count(),
            2
        );
    }
    #[test]
    fn render_survives_missing_tree() {
        let (_dir, ledger, cas) = setup("audit-missing-render");
        let id = ledger.add("missing", &[]).unwrap();
        let tree = snapshot::put(&cas, "<p>missing</p>").unwrap();
        fs::remove_file(cas.path(&tree)).unwrap();
        let text = render(
            &ledger.get(id).unwrap(),
            &[record(0, Some(tree.to_hex().to_string())), record(1, None)],
            &cas,
        )
        .unwrap();
        assert!(text.contains("DOM: unavailable"));
        assert!(text.contains("## Step 1"));
    }
    #[test]
    fn gc_survives_missing_tree() {
        let (dir, ledger, cas) = setup("audit-missing-gc");
        let path = dir.0.join("ledger.db");
        let mut trees = Vec::new();
        for html in ["<p>first</p>", "<p>second</p>"] {
            let tree = snapshot::put(&cas, html).unwrap();
            trees.push(tree);
            let id = ledger.add("old", &[]).unwrap();
            ledger
                .record_step(id, &record(0, Some(tree.to_hex().to_string())))
                .unwrap();
            ledger.transition(id, State::Executing).unwrap();
            ledger.transition(id, State::Committed).unwrap();
        }
        fs::remove_file(cas.path(&trees[0])).unwrap();
        // A retained record references the missing tree, exercising the mark phase too.
        let retained = ledger.add("retained", &[]).unwrap();
        ledger
            .record_step(retained, &record(0, Some(trees[0].to_hex().to_string())))
            .unwrap();
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute(
                "UPDATE tasks SET finished_ms=0 WHERE finished_ms IS NOT NULL",
                [],
            )
            .unwrap();
        assert_eq!(gc(&ledger, &cas, 1, 3 * 86_400_000).unwrap().0, 2);
        assert!(cas.all().unwrap().is_empty());
        assert_eq!(ledger.list().unwrap().len(), 1);
    }
    #[test]
    fn financial_step_stores_no_snapshot() {
        let (_dir, ledger, _cas) = setup("audit-financial");
        let id = ledger.add("financial", &[]).unwrap();
        let rec = StepRecord::new(
            0,
            &Action::Click {
                selector: "#buy".into(),
                risk: Risk::Financial,
            },
            "approved".into(),
            Some("tree".into()),
            "done".into(),
        );
        assert_eq!(rec.after, None);
        ledger.record_step(id, &rec).unwrap();
        assert_eq!(ledger.records(id).unwrap()[0].after, None);
    }
    #[test]
    fn records_roundtrip() {
        let (dir, ledger, _cas) = setup("audit-roundtrip");
        let path = dir.0.join("ledger.db");
        let id = ledger.add("records", &[]).unwrap();
        let other = ledger.add("keep", &[]).unwrap();
        let first = record(0, None);
        let second = record(5, None);
        assert!(first.at_ms.abs_diff(now_ms()) < 5000);
        ledger.record_step(id, &second).unwrap();
        ledger.record_step(id, &first).unwrap();
        ledger.record_step(other, &first).unwrap();
        drop(ledger);
        let ledger = Ledger::open(&path).unwrap();
        assert_eq!(ledger.records(id).unwrap(), vec![second, first.clone()]);
        ledger.delete_task(id).unwrap();
        assert!(ledger.get(id).is_err());
        assert!(ledger.records(id).unwrap().is_empty());
        assert_eq!(ledger.records(other).unwrap(), vec![first]);
        drop(ledger);
        let db = rusqlite::Connection::open(path).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM steps WHERE task_id=?1", [id], |r| {
                r.get::<_, usize>(0)
            })
            .unwrap(),
            0
        );
    }
    #[test]
    fn dom_change_summary() {
        let (_dir, ledger, cas) = setup("audit-dom");
        let id = ledger.add("dom", &[]).unwrap();
        let mut html: String = (0..2000)
            .map(|i| format!("<div class=\"row\" id=\"r{i}\">item {i}</div>"))
            .collect();
        let first = snapshot::put(&cas, &html).unwrap();
        let at = html[..html.len() / 2].rfind('>').unwrap() + 1;
        html.insert_str(at, "<b>x</b>");
        let second = snapshot::put(&cas, &html).unwrap();
        let before = snapshot::chunk_hashes(&cas, &first).unwrap();
        let after = snapshot::chunk_hashes(&cas, &second).unwrap();
        let changed = after.iter().filter(|h| !before.contains(h)).count();
        assert!(changed > 0 && changed < after.len());
        let text = render(
            &ledger.get(id).unwrap(),
            &[
                record(0, Some(first.to_hex().to_string())),
                record(1, Some(second.to_hex().to_string())),
            ],
            &cas,
        )
        .unwrap();
        assert!(text.contains(&format!(
            "DOM: {}/{} chunks changed",
            before.len(),
            before.len()
        )));
        assert!(text.contains(&format!("DOM: {changed}/{} chunks changed", after.len())));
    }
    #[test]
    fn gc_removes_old_tasks_and_orphan_blobs() {
        let (dir, ledger, cas) = setup("audit-gc");
        let root = dir.0.join("cas");
        let path = dir.0.join("ledger.db");
        let old = ledger.add("old", &[]).unwrap();
        let recent = ledger.add("recent", &[]).unwrap();
        let running = ledger.add("running", &[]).unwrap();
        let shared = snapshot::put(&cas, "<p>shared</p>").unwrap();
        let unique = snapshot::put(&cas, "<p>old unique</p>").unwrap();
        let protected = snapshot::put(&cas, "<p>running</p>").unwrap();
        let orphan = cas.put(b"orphan").unwrap();
        for (id, step, hash) in [
            (old, 0, shared),
            (old, 1, unique),
            (recent, 0, shared),
            (running, 0, protected),
        ] {
            ledger
                .record_step(id, &record(step, Some(hash.to_hex().to_string())))
                .unwrap();
        }
        for id in [old, recent] {
            ledger.transition(id, State::Executing).unwrap();
            ledger.transition(id, State::Committed).unwrap();
        }
        ledger.transition(running, State::Executing).unwrap();
        let now = now_ms();
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute(
                "UPDATE tasks SET finished_ms=?2 WHERE id=?1",
                rusqlite::params![old, now - 3 * 86400000],
            )
            .unwrap();
        for name in ["tmp-stale", "tmp-fresh", "unrelated"] {
            fs::write(root.join(name), b"x").unwrap();
        }
        let old_time = UNIX_EPOCH + Duration::from_millis(now - 7200000);
        for name in ["tmp-stale", "unrelated"] {
            fs::File::options()
                .write(true)
                .open(root.join(name))
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(old_time))
                .unwrap();
        }
        fs::write(root.join("tmp-just-stale"), b"x").unwrap();
        fs::File::options()
            .write(true)
            .open(root.join("tmp-just-stale"))
            .unwrap()
            .set_times(
                fs::FileTimes::new()
                    .set_modified(UNIX_EPOCH + Duration::from_millis(now - 3_600_500)),
            )
            .unwrap();
        let initial = cas.all().unwrap().len();
        assert!(gc(&ledger, &cas, 1, now).is_err());
        assert_eq!(cas.all().unwrap().len(), initial);
        assert!(ledger.get(old).is_ok());
        assert!(root.join("tmp-stale").exists());
        ledger.transition(running, State::Queued).unwrap();
        let (tasks, blobs) = gc(&ledger, &cas, 1, now).unwrap();
        assert_eq!(tasks, 1);
        assert_eq!(blobs, 3);
        assert!(ledger.get(old).is_err());
        assert!(ledger.records(old).unwrap().is_empty());
        assert_eq!(ledger.get(running).unwrap().state, State::Queued);
        assert!(ledger.get(recent).is_ok());
        assert!(cas.get(&unique).is_err());
        assert!(cas.get(&orphan).is_err());
        assert_eq!(snapshot::get(&cas, &shared).unwrap(), "<p>shared</p>");
        assert_eq!(snapshot::get(&cas, &protected).unwrap(), "<p>running</p>");
        assert!(!root.join("tmp-stale").exists());
        assert!(!root.join("tmp-just-stale").exists());
        assert!(root.join("tmp-fresh").exists());
        assert!(root.join("unrelated").exists());
        println!(
            "GC removed {tasks} tasks, {blobs} blobs, 2 stale tmp (including just over one hour); fresh tmp and unrelated retained"
        );
    }
}
