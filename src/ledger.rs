use crate::{action::Action, challenge::ChallengeKind};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

const SELECT: &str = "SELECT id,title,steps,cursor,state,approved_step,finished_ms,goal FROM tasks";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Handoff {
    Approval,
    Challenge(ChallengeKind),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Suggested,
    Queued,
    Executing,
    AwaitingHandoff(Handoff),
    Committed,
    Failed(String),
    Dismissed,
}
pub fn is_open(state: &State) -> bool {
    matches!(
        state,
        State::Suggested | State::Queued | State::Executing | State::AwaitingHandoff(_)
    )
}
#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub id: i64,
    pub title: String,
    pub goal: Option<String>,
    pub steps: Vec<Action>,
    pub cursor: usize,
    pub state: State,
    pub approved_step: Option<usize>,
    pub finished_ms: Option<u64>,
}
pub struct Ledger {
    conn: rusqlite::Connection,
}
impl Ledger {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let conn = rusqlite::Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA secure_delete=ON;
            CREATE TABLE IF NOT EXISTS tasks (
                id INTEGER PRIMARY KEY, title TEXT NOT NULL, goal TEXT, steps TEXT NOT NULL,
                cursor INTEGER NOT NULL DEFAULT 0, state TEXT NOT NULL,
                approved_step INTEGER, finished_ms INTEGER
            );
            CREATE TABLE IF NOT EXISTS steps (seq INTEGER PRIMARY KEY AUTOINCREMENT, task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE, step INTEGER NOT NULL, json TEXT NOT NULL);",
        )?;
        let tx =
            rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)?;
        let has_goal: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('tasks') WHERE name='goal')",
            [],
            |row| row.get(0),
        )?;
        if !has_goal {
            tx.execute("ALTER TABLE tasks ADD COLUMN goal TEXT", [])?;
        }
        tx.commit()?;
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
        Ok(Self { conn })
    }
    pub fn record_step(&self, id: i64, record: &crate::audit::StepRecord) -> anyhow::Result<()> {
        let mut record = record.clone();
        record.thought = record
            .thought
            .map(|s| s.chars().take(crate::audit::MAX_THOUGHT_CHARS).collect());
        self.conn.execute(
            "INSERT INTO steps (task_id, step, json) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, record.step, serde_json::to_string(&record)?],
        )?;
        Ok(())
    }
    pub fn records(&self, id: i64) -> anyhow::Result<Vec<crate::audit::StepRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT json FROM steps WHERE task_id=?1 ORDER BY seq")?;
        let rows = stmt.query_map([id], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }
    pub fn delete_task(&self, id: i64) -> anyhow::Result<()> {
        if self.conn.execute("DELETE FROM tasks WHERE id=?1", [id])? == 0 {
            anyhow::bail!("task {id} not found");
        }
        Ok(())
    }
    pub fn add(&self, title: &str, steps: &[Action]) -> anyhow::Result<i64> {
        self.conn.execute(
            "INSERT INTO tasks (title, steps, state) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                title,
                serde_json::to_string(steps)?,
                serde_json::to_string(&State::Queued)?
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }
    pub fn add_goal(&self, title: &str, goal: &str) -> anyhow::Result<i64> {
        self.conn.execute(
            "INSERT INTO tasks (title, goal, steps, state) VALUES (?1, ?2, '[]', ?3)",
            rusqlite::params![title, goal, serde_json::to_string(&State::Queued)?],
        )?;
        Ok(self.conn.last_insert_rowid())
    }
    pub fn suggest(&self, goal: &str) -> anyhow::Result<Option<i64>> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        {
            let mut stmt = tx.prepare("SELECT state FROM tasks WHERE goal=?1")?;
            let rows = stmt.query_map([goal], |row| row.get::<_, String>(0))?;
            for row in rows {
                if is_open(&serde_json::from_str(&row?)?) {
                    return Ok(None);
                }
            }
        }
        tx.execute(
            "INSERT INTO tasks (title, goal, steps, state) VALUES (?1, ?1, '[]', ?2)",
            rusqlite::params![goal, serde_json::to_string(&State::Suggested)?],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(Some(id))
    }
    pub fn accept(&self, id: i64) -> anyhow::Result<()> {
        self.transition_from(id, State::Suggested, State::Queued, None)
    }
    pub fn dismiss(&self, id: i64) -> anyhow::Result<()> {
        self.transition_from(
            id,
            State::Suggested,
            State::Dismissed,
            Some(self.get(id)?.steps),
        )
    }
    pub fn push_step(&self, id: i64, action: &Action) -> anyhow::Result<usize> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let mut task = self.get(id)?;
        let index = task.steps.len();
        task.steps.push(action.clone());
        tx.execute(
            "UPDATE tasks SET steps=?2 WHERE id=?1",
            rusqlite::params![id, serde_json::to_string(&task.steps)?],
        )?;
        tx.commit()?;
        Ok(index)
    }
    pub fn get(&self, id: i64) -> anyhow::Result<Task> {
        let mut stmt = self.conn.prepare(&format!("{SELECT} WHERE id=?1"))?;
        let mut rows = stmt.query([id])?;
        let row = rows
            .next()?
            .ok_or_else(|| anyhow::anyhow!("task {id} not found"))?;
        Self::task(row)
    }
    fn task(row: &rusqlite::Row<'_>) -> anyhow::Result<Task> {
        Ok(Task {
            id: row.get(0)?,
            title: row.get(1)?,
            goal: row.get(7)?,
            steps: serde_json::from_str(&row.get::<_, String>(2)?)?,
            cursor: row.get(3)?,
            state: serde_json::from_str(&row.get::<_, String>(4)?)?,
            approved_step: row.get(5)?,
            finished_ms: row.get(6)?,
        })
    }
    pub fn list(&self) -> anyhow::Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(&format!("{SELECT} ORDER BY id"))?;
        let mut rows = stmt.query([])?;
        let mut tasks = Vec::new();
        while let Some(row) = rows.next()? {
            tasks.push(Self::task(row)?);
        }
        Ok(tasks)
    }
    pub fn set_cursor(&self, id: i64, cursor: usize) -> anyhow::Result<()> {
        if self.conn.execute(
            "UPDATE tasks SET cursor=?2 WHERE id=?1",
            rusqlite::params![id, cursor],
        )? == 0
        {
            anyhow::bail!("task {id} not found");
        }
        Ok(())
    }
    pub fn next_queued(&self) -> anyhow::Result<Option<Task>> {
        let mut stmt = self
            .conn
            .prepare(&format!("{SELECT} WHERE state=?1 ORDER BY id LIMIT 1"))?;
        let mut rows = stmt.query([serde_json::to_string(&State::Queued)?])?;
        rows.next()?.map(Self::task).transpose()
    }
    pub fn transition(&self, id: i64, to: State) -> anyhow::Result<()> {
        let task = self.get(id)?;
        let from = task.state;
        if !matches!(
            (&from, &to),
            (State::Suggested, State::Queued | State::Dismissed)
                | (State::Queued, State::Executing)
                | (
                    State::Executing,
                    State::AwaitingHandoff(_) | State::Committed | State::Failed(_) | State::Queued
                )
                | (State::AwaitingHandoff(_), State::Queued | State::Failed(_))
        ) {
            anyhow::bail!("illegal transition from {from:?} to {to:?}");
        }
        let steps = matches!(to, State::Committed | State::Failed(_) | State::Dismissed)
            .then_some(task.steps);
        self.transition_from(id, from, to, steps)
    }
    fn transition_from(
        &self,
        id: i64,
        from: State,
        to: State,
        steps: Option<Vec<Action>>,
    ) -> anyhow::Result<()> {
        let terminal = matches!(to, State::Committed | State::Failed(_) | State::Dismissed);
        let redacted = steps
            .map(|steps| {
                serde_json::to_string(&steps.iter().map(Action::redacted).collect::<Vec<_>>())
            })
            .transpose()?;
        if self.conn.execute(
            "UPDATE tasks SET state=?2, finished_ms=CASE WHEN ?3 THEN CAST(unixepoch('subsec')*1000 AS INTEGER) END, steps=COALESCE(?5, steps) WHERE id=?1 AND state=?4",
            rusqlite::params![id, serde_json::to_string(&to)?, terminal, serde_json::to_string(&from)?, redacted],
        )? == 0 {
            anyhow::bail!("illegal transition from {:?} to {to:?}", self.get(id)?.state);
        }
        if terminal {
            // The state update has committed. A busy reader may defer secret erasure
            // in the WAL until the next checkpoint; it must not fail the transition.
            self.conn
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                    row.get::<_, i64>(0)
                })?;
        }
        Ok(())
    }
    pub fn approve(&self, id: i64) -> anyhow::Result<()> {
        let task = self.get(id)?;
        if task.state != State::AwaitingHandoff(Handoff::Approval) {
            anyhow::bail!("illegal transition from {:?} to Queued", task.state);
        }
        if self.conn.execute(
            "UPDATE tasks SET state=?2, approved_step=cursor, finished_ms=NULL WHERE id=?1 AND state=?3",
            rusqlite::params![id, serde_json::to_string(&State::Queued)?, serde_json::to_string(&State::AwaitingHandoff(Handoff::Approval))?],
        )? == 0 {
            anyhow::bail!("illegal transition from {:?} to Queued", self.get(id)?.state);
        }
        Ok(())
    }
    pub fn resume(&self, id: i64) -> anyhow::Result<()> {
        let task = self.get(id)?;
        if !matches!(task.state, State::AwaitingHandoff(Handoff::Challenge(_))) {
            anyhow::bail!("illegal transition from {:?} to Queued", task.state);
        }
        self.transition_from(id, task.state, State::Queued, None)
    }
    pub fn recover(&self) -> anyhow::Result<usize> {
        Ok(self.conn.execute(
            "UPDATE tasks SET state=?1 WHERE state=?2",
            rusqlite::params![
                serde_json::to_string(&State::Queued)?,
                serde_json::to_string(&State::Executing)?
            ],
        )?)
    }
}

impl Drop for Ledger {
    fn drop(&mut self) {
        let _ = self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
    #[test]
    fn terminal_transition_ok_while_reader_open() {
        let dir = temp_dir("ledger-reader");
        let path = dir.0.join("ledger.db");
        let ledger = Ledger::open(&path).unwrap();
        let id = ledger
            .add(
                "secret",
                &[Action::Type {
                    selector: "#password".into(),
                    text: "hunter2".into(),
                    secret: true,
                    risk: crate::gate::Risk::Write,
                }],
            )
            .unwrap();
        let reader = rusqlite::Connection::open(&path).unwrap();
        reader.execute_batch("BEGIN").unwrap();
        assert_eq!(
            reader
                .query_row("SELECT count(*) FROM tasks", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        ledger.transition(id, State::Executing).unwrap();
        let started = std::time::Instant::now();
        let result = ledger.transition(id, State::Committed);
        println!(
            "terminal transition while reader held: {:?}; {result:?}",
            started.elapsed()
        );
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(ledger.get(id).unwrap().state, State::Committed);
        // A held reader may defer WAL erasure until a clean checkpoint.
        reader.execute_batch("ROLLBACK").unwrap();
        drop(reader);
        drop(ledger);
        drop(Ledger::open(&path).unwrap());
        for file in [&path, &dir.0.join("ledger.db-wal")] {
            match std::fs::read(file) {
                Ok(bytes) => {
                    assert!(
                        !bytes
                            .windows(b"hunter2".len())
                            .any(|window| window == b"hunter2"),
                        "secret in {}",
                        file.display()
                    );
                    println!(
                        "{}: {} bytes, no secret",
                        file.file_name().unwrap().to_string_lossy(),
                        bytes.len()
                    );
                }
                Err(error) if file != &path && error.kind() == std::io::ErrorKind::NotFound => {
                    println!("ledger.db-wal absent")
                }
                Err(error) => panic!("{}: {error}", file.display()),
            }
        }
    }
    #[test]
    fn delete_task_cascades() {
        let dir = temp_dir("ledger-cascade");
        let ledger = Ledger::open(dir.0.join("db")).unwrap();
        let id = ledger.add("cascade", &[]).unwrap();
        let rec = crate::audit::StepRecord::new(
            0,
            &Action::Extract {
                selector: "h1".into(),
            },
            "auto".into(),
            None,
            "ok".into(),
        );
        ledger.record_step(id, &rec).unwrap();
        ledger.delete_task(id).unwrap();
        assert_eq!(
            ledger
                .conn
                .query_row("SELECT count(*) FROM steps", [], |r| r.get::<_, usize>(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn concurrent_writer_waits() {
        let dir = temp_dir("ledger-concurrent");
        let path = dir.0.join("ledger.db");
        let first = Ledger::open(&path).unwrap();
        let second = Ledger::open(&path).unwrap();
        first.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            first.conn.execute_batch("COMMIT").unwrap();
        });
        let result = second.add("x", &[]);
        writer.join().unwrap();
        assert!(result.is_ok(), "{result:?}");
    }
    #[test]
    fn ledger_errors_keep_messages() {
        let dir = temp_dir("ledger-error-messages");
        let path = dir.0.join("ledger.db");
        let first = Ledger::open(&path).unwrap();
        let second = Ledger::open(&path).unwrap();
        let missing: anyhow::Result<Task> = first.get(-1);
        assert_eq!(missing.unwrap_err().to_string(), "task -1 not found");
        let id = first.add("x", &[]).unwrap();
        first.transition(id, State::Executing).unwrap();
        let stale: anyhow::Result<()> = second.transition(id, State::Executing);
        assert_eq!(
            stale.unwrap_err().to_string(),
            "illegal transition from Executing to Executing"
        );
    }
    #[test]
    fn stale_handle_transition_rejected() {
        let dir = temp_dir("ledger-stale");
        let path = dir.0.join("ledger.db");
        let first = Ledger::open(&path).unwrap();
        let second = Ledger::open(&path).unwrap();
        let id = first.add("x", &[]).unwrap();
        first.transition(id, State::Executing).unwrap();
        assert_eq!(
            second
                .transition(id, State::Executing)
                .unwrap_err()
                .to_string(),
            "illegal transition from Executing to Executing"
        );
        assert_eq!(second.get(id).unwrap().state, State::Executing);
    }
    #[test]
    fn finished_ms_is_epoch_ms() {
        let dir = temp_dir("ledger-epoch");
        let ledger = Ledger::open(dir.0.join("ledger.db")).unwrap();
        let id = ledger.add("x", &[]).unwrap();
        ledger.transition(id, State::Executing).unwrap();
        ledger.transition(id, State::Committed).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert!(ledger.get(id).unwrap().finished_ms.unwrap().abs_diff(now) <= 5_000);
    }
    fn states() -> Vec<State> {
        vec![
            State::Queued,
            State::Executing,
            State::AwaitingHandoff(Handoff::Approval),
            State::AwaitingHandoff(Handoff::Challenge(ChallengeKind::Turnstile)),
            State::Committed,
            State::Failed("failure".into()),
            State::Suggested,
            State::Dismissed,
        ]
    }
    #[test]
    fn add_then_get() {
        let dir = temp_dir("ledger-add");
        let path = dir.0.join("ledger.db");
        let ledger = Ledger::open(&path).unwrap();
        assert_eq!(
            ledger
                .conn
                .query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "wal"
        );
        assert_eq!(
            ledger
                .conn
                .query_row("PRAGMA synchronous", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        let steps = vec![Action::Navigate {
            url: "https://example.com".into(),
        }];
        let id = ledger.add("first", &steps).unwrap();
        let second = ledger.add("second", &[]).unwrap();
        assert!(second > id);
        assert_eq!(
            ledger.get(id).unwrap(),
            Task {
                id,
                title: "first".into(),
                goal: None,
                steps,
                cursor: 0,
                state: State::Queued,
                approved_step: None,
                finished_ms: None
            }
        );
        assert_eq!(
            ledger
                .list()
                .unwrap()
                .iter()
                .map(|t| t.id)
                .collect::<Vec<_>>(),
            vec![id, second]
        );
        assert_eq!(ledger.next_queued().unwrap().unwrap().id, id);
        ledger.transition(id, State::Executing).unwrap();
        assert_eq!(ledger.next_queued().unwrap().unwrap().id, second);
        ledger.transition(second, State::Executing).unwrap();
        assert!(ledger.next_queued().unwrap().is_none());
        for error in [
            ledger.get(-1).unwrap_err(),
            ledger.set_cursor(-1, 2).unwrap_err(),
            ledger.transition(-1, State::Executing).unwrap_err(),
            ledger.approve(-1).unwrap_err(),
            ledger.resume(-1).unwrap_err(),
        ] {
            assert_eq!(error.to_string(), "task -1 not found");
        }
        drop(ledger);
        assert_eq!(Ledger::open(path).unwrap().get(id).unwrap().title, "first");
    }
    #[test]
    fn happy_path_transitions() {
        let dir = temp_dir("ledger-transitions");
        let ledger = Ledger::open(dir.0.join("ledger.db")).unwrap();
        // Queued, Executing, approval wait, challenge wait, Committed, Failed, Suggested, Dismissed.
        let expected = [
            [false, true, false, false, false, false, false, false],
            [true, false, true, true, true, true, false, false],
            [true, false, false, false, false, true, false, false],
            [true, false, false, false, false, true, false, false],
            [false; 8],
            [false; 8],
            [true, false, false, false, false, false, false, true],
            [false; 8],
        ];
        for (from_index, from) in states().into_iter().enumerate() {
            for (to_index, to) in states().into_iter().enumerate() {
                let id = if matches!(from, State::Suggested | State::Dismissed) {
                    let id = ledger
                        .suggest(&format!("matrix {from_index} {to_index}"))
                        .unwrap()
                        .unwrap();
                    if from == State::Dismissed {
                        ledger.dismiss(id).unwrap();
                    }
                    id
                } else {
                    ledger.add("matrix", &[]).unwrap()
                };
                if !matches!(from, State::Queued | State::Suggested | State::Dismissed) {
                    ledger.transition(id, State::Executing).unwrap();
                    if from != State::Executing {
                        ledger.transition(id, from.clone()).unwrap();
                    }
                }
                let legal = expected[from_index][to_index];
                let result = ledger.transition(id, to.clone());
                if legal {
                    result.unwrap();
                    assert_eq!(ledger.get(id).unwrap().state, to);
                } else {
                    assert_eq!(
                        result.unwrap_err().to_string(),
                        format!("illegal transition from {from:?} to {to:?}")
                    );
                    assert_eq!(ledger.get(id).unwrap().state, from);
                }
            }
        }
    }
    #[test]
    fn committed_is_terminal() {
        let dir = temp_dir("ledger-terminal");
        let ledger = Ledger::open(dir.0.join("ledger.db")).unwrap();
        for terminal in [State::Committed, State::Failed("failure".into())] {
            let id = ledger.add("terminal", &[]).unwrap();
            ledger.transition(id, State::Executing).unwrap();
            ledger.transition(id, terminal).unwrap();
            let before = ledger.get(id).unwrap();
            assert!(before.finished_ms.unwrap() > 0);
            for to in states() {
                assert_eq!(
                    ledger.transition(id, to.clone()).unwrap_err().to_string(),
                    format!("illegal transition from {:?} to {to:?}", before.state)
                );
            }
            assert!(ledger.approve(id).is_err());
            assert!(ledger.resume(id).is_err());
            assert_eq!(ledger.get(id).unwrap(), before);
        }
    }
    #[test]
    fn approve_only_from_approval_wait() {
        let dir = temp_dir("ledger-approve");
        let path = dir.0.join("ledger.db");
        let ledger = Ledger::open(&path).unwrap();
        let id = ledger.add("approval", &[]).unwrap();
        assert!(ledger.approve(id).is_err());
        assert!(ledger.resume(id).is_err());
        ledger.transition(id, State::Executing).unwrap();
        ledger.set_cursor(id, 3).unwrap();
        assert!(ledger.approve(id).is_err());
        assert!(ledger.resume(id).is_err());
        ledger
            .transition(id, State::AwaitingHandoff(Handoff::Approval))
            .unwrap();
        assert!(ledger.resume(id).is_err());
        ledger.approve(id).unwrap();
        let task = ledger.get(id).unwrap();
        assert_eq!(task.state, State::Queued);
        assert_eq!(task.approved_step, Some(3));
        assert_eq!(task.finished_ms, None);
        ledger.transition(id, State::Executing).unwrap();
        ledger
            .transition(
                id,
                State::AwaitingHandoff(Handoff::Challenge(ChallengeKind::ReCaptcha)),
            )
            .unwrap();
        let before = ledger.get(id).unwrap();
        assert!(ledger.approve(id).is_err());
        assert_eq!(ledger.get(id).unwrap(), before);
        ledger.resume(id).unwrap();
        assert_eq!(ledger.get(id).unwrap().state, State::Queued);
        drop(ledger);
        assert_eq!(
            Ledger::open(path).unwrap().get(id).unwrap().approved_step,
            Some(3)
        );
    }
    #[test]
    fn recover_requeues_executing() {
        let dir = temp_dir("ledger-recover");
        let path = dir.0.join("ledger.db");
        let ledger = Ledger::open(&path).unwrap();
        let id = ledger.add("interrupted", &[]).unwrap();
        let waiting = ledger.add("waiting", &[]).unwrap();
        ledger.transition(id, State::Executing).unwrap();
        ledger.set_cursor(id, 7).unwrap();
        ledger.transition(waiting, State::Executing).unwrap();
        ledger
            .transition(
                waiting,
                State::AwaitingHandoff(Handoff::Challenge(ChallengeKind::HCaptcha)),
            )
            .unwrap();
        drop(ledger);
        let ledger = Ledger::open(path).unwrap();
        assert_eq!(ledger.get(id).unwrap().state, State::Executing);
        assert_eq!(ledger.recover().unwrap(), 1);
        assert_eq!(ledger.get(id).unwrap().cursor, 7);
        assert_eq!(ledger.get(id).unwrap().state, State::Queued);
        assert_eq!(ledger.get(id).unwrap().finished_ms, None);
        assert!(matches!(
            ledger.get(waiting).unwrap().state,
            State::AwaitingHandoff(_)
        ));
        assert_eq!(ledger.recover().unwrap(), 0);
    }
    #[test]
    fn finished_ms_set_on_commit() {
        let dir = temp_dir("ledger-finish");
        let path = dir.0.join("ledger.db");
        let ledger = Ledger::open(&path).unwrap();
        let id = ledger.add("finished", &[]).unwrap();
        assert_eq!(ledger.get(id).unwrap().finished_ms, None);
        ledger.transition(id, State::Executing).unwrap();
        assert_eq!(ledger.get(id).unwrap().finished_ms, None);
        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        ledger.transition(id, State::Committed).unwrap();
        let finished = ledger.get(id).unwrap().finished_ms.unwrap();
        let after = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert!((before..=after).contains(&finished));
        drop(ledger);
        assert_eq!(
            Ledger::open(path).unwrap().get(id).unwrap().finished_ms,
            Some(finished)
        );
    }
    #[test]
    fn goal_task_roundtrip() {
        let dir = temp_dir("ledger-goal");
        let path = dir.0.join("ledger.db");
        let ledger = Ledger::open(&path).unwrap();
        let goal = "Find the opening hours";
        let id = ledger.add_goal("hours", goal).unwrap();
        let task = ledger.get(id).unwrap();
        assert_eq!(task.title, "hours");
        assert_eq!(task.goal.as_deref(), Some(goal));
        assert!(task.steps.is_empty());
        assert_eq!(task.state, State::Queued);
        assert_eq!(task.cursor, 0);
        assert_eq!(ledger.next_queued().unwrap().unwrap(), task);
        assert_eq!(ledger.list().unwrap(), vec![task.clone()]);
        let scripted = ledger.add("scripted", &[]).unwrap();
        assert!(ledger.get(scripted).unwrap().goal.is_none());
        drop(ledger);
        let ledger = Ledger::open(path).unwrap();
        assert_eq!(ledger.get(id).unwrap(), task);
    }
    #[test]
    fn push_step_appends() {
        let dir = temp_dir("ledger-append");
        let path = dir.0.join("ledger.db");
        let ledger = Ledger::open(&path).unwrap();
        let id = ledger.add_goal("append", "Read a heading").unwrap();
        let nav = Action::Navigate {
            url: "https://example.com".into(),
        };
        let extract = Action::Extract {
            selector: "h1".into(),
        };
        assert_eq!(ledger.push_step(id, &nav).unwrap(), 0);
        assert_eq!(ledger.push_step(id, &extract).unwrap(), 1);
        assert_eq!(
            ledger.get(id).unwrap().steps,
            vec![nav.clone(), extract.clone()]
        );
        assert_eq!(ledger.get(id).unwrap().cursor, 0);
        assert_eq!(
            ledger.get(id).unwrap().goal.as_deref(),
            Some("Read a heading")
        );
        assert_eq!(
            ledger.push_step(-1, &nav).unwrap_err().to_string(),
            "task -1 not found"
        );
        let scripted = ledger.add("scripted", &[nav]).unwrap();
        assert_eq!(ledger.push_step(scripted, &extract).unwrap(), 1);
        drop(ledger);
        let first = Ledger::open(&path).unwrap();
        let second = Ledger::open(&path).unwrap();
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let peer_gate = gate.clone();
        let writer = std::thread::spawn(move || {
            peer_gate.wait();
            (0..10)
                .map(|n| {
                    let action = Action::Extract {
                        selector: format!("#a{n}"),
                    };
                    (first.push_step(id, &action).unwrap(), action)
                })
                .collect::<Vec<_>>()
        });
        gate.wait();
        let mut appended = (0..10)
            .map(|n| {
                let action = Action::Extract {
                    selector: format!("#b{n}"),
                };
                (second.push_step(id, &action).unwrap(), action)
            })
            .collect::<Vec<_>>();
        appended.extend(writer.join().unwrap());
        let steps = second.get(id).unwrap().steps;
        assert_eq!(steps.len(), 22);
        let indices: std::collections::HashSet<_> = appended.iter().map(|(i, _)| *i).collect();
        assert_eq!(indices.len(), 20);
        for (index, action) in appended {
            assert_eq!(steps[index], action);
        }
    }
    #[test]
    fn old_db_gains_goal_column() {
        let dir = temp_dir("ledger-old-schema");
        let path = dir.0.join("ledger.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(r#"
            CREATE TABLE tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL, steps TEXT NOT NULL,
                cursor INTEGER NOT NULL DEFAULT 0, state TEXT NOT NULL, approved_step INTEGER, finished_ms INTEGER);
            CREATE TABLE steps (seq INTEGER PRIMARY KEY AUTOINCREMENT, task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE, step INTEGER NOT NULL, json TEXT NOT NULL);
            INSERT INTO tasks(id,title,steps,cursor,state) VALUES(1,'old','[{"op":"extract","selector":"h1"}]',1,'"Committed"');
            INSERT INTO steps(task_id,step,json) VALUES(1,0,'{"step":0,"action":{"op":"extract","selector":"h1"},"decision":"auto","after":null,"outcome":"old heading","at_ms":1}');
        "#).unwrap();
        drop(conn);
        let ledger = Ledger::open(&path).unwrap();
        assert_eq!(ledger.get(1).unwrap().goal, None);
        assert_eq!(
            ledger.get(1).unwrap().steps,
            vec![Action::Extract {
                selector: "h1".into()
            }]
        );
        assert_eq!(ledger.get(1).unwrap().cursor, 1);
        assert_eq!(ledger.get(1).unwrap().state, State::Committed);
        let records = ledger.records(1).unwrap();
        assert_eq!(records[0].outcome, "old heading");
        assert_eq!(records[0].thought, None);
        assert_eq!(records[0].model_request, None);
        let id = ledger.add_goal("new", "Read more").unwrap();
        assert_eq!(ledger.get(id).unwrap().goal.as_deref(), Some("Read more"));
        drop(ledger);
        let reopened = Ledger::open(path).unwrap();
        assert_eq!(reopened.get(id).unwrap().goal.as_deref(), Some("Read more"));
        assert_eq!(reopened.records(1).unwrap(), records);
    }
    #[test]
    fn suggest_accept_dismiss() {
        let dir = temp_dir("ledger-inbox");
        let path = dir.0.join("ledger.db");
        let ledger = Ledger::open(&path).unwrap();
        let id = ledger.suggest("read the news").unwrap().unwrap();
        let task = ledger.get(id).unwrap();
        assert_eq!(task.state, State::Suggested);
        assert_eq!(task.title, "read the news");
        assert_eq!(task.goal.as_deref(), Some("read the news"));
        assert!(task.steps.is_empty());
        assert_eq!(task.cursor, 0);
        assert_eq!(task.finished_ms, None);
        assert!(ledger.next_queued().unwrap().is_none());
        ledger.accept(id).unwrap();
        assert_eq!(ledger.get(id).unwrap().state, State::Queued);
        assert_eq!(ledger.next_queued().unwrap().unwrap().id, id);
        assert_eq!(
            ledger.accept(id).unwrap_err().to_string(),
            "illegal transition from Queued to Queued"
        );
        let dismissed = ledger.suggest("check the weather").unwrap().unwrap();
        ledger.dismiss(dismissed).unwrap();
        let before = ledger.get(dismissed).unwrap();
        assert_eq!(before.state, State::Dismissed);
        assert!(before.finished_ms.unwrap().abs_diff(crate::audit::now_ms()) < 5_000);
        assert_eq!(
            ledger.accept(dismissed).unwrap_err().to_string(),
            "illegal transition from Dismissed to Queued"
        );
        assert_eq!(
            ledger.dismiss(id).unwrap_err().to_string(),
            "illegal transition from Queued to Dismissed"
        );
        for to in states() {
            assert_eq!(
                ledger
                    .transition(dismissed, to.clone())
                    .unwrap_err()
                    .to_string(),
                format!("illegal transition from Dismissed to {to:?}")
            );
        }
        assert_eq!(ledger.get(dismissed).unwrap(), before);
        ledger.transition(id, State::Executing).unwrap();
        assert_eq!(
            ledger.accept(id).unwrap_err().to_string(),
            "illegal transition from Executing to Queued"
        );
        assert_eq!(ledger.get(id).unwrap().state, State::Executing);
        for error in [
            ledger.accept(-1).unwrap_err(),
            ledger.dismiss(-1).unwrap_err(),
        ] {
            assert_eq!(error.to_string(), "task -1 not found");
        }
        drop(ledger);
        let ledger = Ledger::open(path).unwrap();
        assert_eq!(ledger.get(dismissed).unwrap(), before);
    }
    #[test]
    fn suggest_dedupes_open_goal() {
        let dir = temp_dir("ledger-suggest-dedupe");
        let ledger = Ledger::open(dir.0.join("ledger.db")).unwrap();
        let id = ledger.suggest("x").unwrap().unwrap();
        assert_eq!(ledger.suggest("x").unwrap(), None);
        ledger.dismiss(id).unwrap();
        let replacement = ledger.suggest("x").unwrap().unwrap();
        assert_ne!(replacement, id);
        ledger.dismiss(replacement).unwrap();
        let queued = ledger.add_goal("x", "x").unwrap();
        assert_eq!(ledger.suggest("x").unwrap(), None);
        ledger.transition(queued, State::Executing).unwrap();
        assert_eq!(ledger.suggest("x").unwrap(), None);
        ledger
            .transition(queued, State::AwaitingHandoff(Handoff::Approval))
            .unwrap();
        assert_eq!(ledger.suggest("x").unwrap(), None);
        ledger
            .transition(queued, State::Failed("not available".into()))
            .unwrap();
        let next = ledger.suggest("x").unwrap().unwrap();
        ledger.accept(next).unwrap();
        ledger.transition(next, State::Executing).unwrap();
        ledger.transition(next, State::Committed).unwrap();
        assert!(ledger.suggest("x").unwrap().is_some());
        for state in states() {
            assert_eq!(
                is_open(&state),
                matches!(
                    state,
                    State::Suggested | State::Queued | State::Executing | State::AwaitingHandoff(_)
                ),
                "{state:?}"
            );
        }
    }
    #[test]
    fn concurrent_suggestions_dedupe() {
        let dir = temp_dir("ledger-suggest-concurrent");
        let path = dir.0.join("ledger.db");
        let first = Ledger::open(&path).unwrap();
        let second = Ledger::open(&path).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let peer = barrier.clone();
        let writer = std::thread::spawn(move || {
            peer.wait();
            first.suggest("same goal").unwrap()
        });
        barrier.wait();
        let results = [second.suggest("same goal").unwrap(), writer.join().unwrap()];
        assert_eq!(results.iter().filter(|r| r.is_some()).count(), 1);
        assert_eq!(second.list().unwrap().len(), 1);
        assert_eq!(second.list().unwrap()[0].state, State::Suggested);
    }
}
