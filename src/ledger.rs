use crate::{action::Action, challenge::ChallengeKind};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

const SELECT: &str = "SELECT id,title,steps,cursor,state,approved_step,finished_ms FROM tasks";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Handoff {
    Approval,
    Challenge(ChallengeKind),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Queued,
    Executing,
    AwaitingHandoff(Handoff),
    Committed,
    Failed(String),
}
#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub id: i64,
    pub title: String,
    pub steps: Vec<Action>,
    pub cursor: usize,
    pub state: State,
    pub approved_step: Option<usize>,
    pub finished_ms: Option<u64>,
}
#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("task {0} not found")]
    NotFound(i64),
    #[error("illegal transition from {from:?} to {to:?}")]
    Illegal { from: State, to: State },
}
pub struct Ledger {
    conn: rusqlite::Connection,
}
impl Ledger {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, LedgerError> {
        let conn = rusqlite::Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
            CREATE TABLE IF NOT EXISTS tasks (
                id INTEGER PRIMARY KEY, title TEXT NOT NULL, steps TEXT NOT NULL,
                cursor INTEGER NOT NULL DEFAULT 0, state TEXT NOT NULL,
                approved_step INTEGER, finished_ms INTEGER
            );
            CREATE TABLE IF NOT EXISTS steps (task_id INTEGER NOT NULL, step INTEGER NOT NULL, json TEXT NOT NULL, PRIMARY KEY(task_id, step));",
        )?;
        Ok(Self { conn })
    }
    pub fn record_step(
        &self,
        id: i64,
        record: &crate::audit::StepRecord,
    ) -> Result<(), LedgerError> {
        self.get(id)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO steps (task_id, step, json) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, record.step, serde_json::to_string(record)?],
        )?;
        Ok(())
    }
    pub fn records(&self, id: i64) -> Result<Vec<crate::audit::StepRecord>, LedgerError> {
        let mut stmt = self
            .conn
            .prepare("SELECT json FROM steps WHERE task_id=?1 ORDER BY step")?;
        let mut rows = stmt.query([id])?;
        let mut records = Vec::new();
        while let Some(row) = rows.next()? {
            records.push(serde_json::from_str(&row.get::<_, String>(0)?)?);
        }
        Ok(records)
    }
    pub fn delete_task(&self, id: i64) -> Result<(), LedgerError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM steps WHERE task_id=?1", [id])?;
        if tx.execute("DELETE FROM tasks WHERE id=?1", [id])? == 0 {
            return Err(LedgerError::NotFound(id));
        }
        tx.commit()?;
        Ok(())
    }
    pub fn add(&self, title: &str, steps: &[Action]) -> Result<i64, LedgerError> {
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
    pub fn get(&self, id: i64) -> Result<Task, LedgerError> {
        let mut stmt = self.conn.prepare(&format!("{SELECT} WHERE id=?1"))?;
        let mut rows = stmt.query([id])?;
        let row = rows.next()?.ok_or(LedgerError::NotFound(id))?;
        Self::task(row)
    }
    fn task(row: &rusqlite::Row<'_>) -> Result<Task, LedgerError> {
        Ok(Task {
            id: row.get(0)?,
            title: row.get(1)?,
            steps: serde_json::from_str(&row.get::<_, String>(2)?)?,
            cursor: row.get(3)?,
            state: serde_json::from_str(&row.get::<_, String>(4)?)?,
            approved_step: row.get(5)?,
            finished_ms: row.get(6)?,
        })
    }
    pub fn list(&self) -> Result<Vec<Task>, LedgerError> {
        let mut stmt = self.conn.prepare(&format!("{SELECT} ORDER BY id"))?;
        let mut rows = stmt.query([])?;
        let mut tasks = Vec::new();
        while let Some(row) = rows.next()? {
            tasks.push(Self::task(row)?);
        }
        Ok(tasks)
    }
    pub fn set_cursor(&self, id: i64, cursor: usize) -> Result<(), LedgerError> {
        if self.conn.execute(
            "UPDATE tasks SET cursor=?2 WHERE id=?1",
            rusqlite::params![id, cursor],
        )? == 0
        {
            return Err(LedgerError::NotFound(id));
        }
        Ok(())
    }
    pub fn next_queued(&self) -> Result<Option<Task>, LedgerError> {
        let mut stmt = self
            .conn
            .prepare(&format!("{SELECT} WHERE state=?1 ORDER BY id LIMIT 1"))?;
        let mut rows = stmt.query([serde_json::to_string(&State::Queued)?])?;
        rows.next()?.map(Self::task).transpose()
    }
    pub fn transition(&self, id: i64, to: State) -> Result<(), LedgerError> {
        let from = self.get(id)?.state;
        if !matches!(
            (&from, &to),
            (State::Queued, State::Executing)
                | (
                    State::Executing,
                    State::AwaitingHandoff(_) | State::Committed | State::Failed(_) | State::Queued
                )
                | (State::AwaitingHandoff(_), State::Queued | State::Failed(_))
        ) {
            return Err(LedgerError::Illegal { from, to });
        }
        self.transition_from(id, from, to)
    }
    fn transition_from(&self, id: i64, from: State, to: State) -> Result<(), LedgerError> {
        if self.conn.execute(
            "UPDATE tasks SET state=?2, finished_ms=CASE WHEN ?3 THEN CAST(unixepoch('subsec')*1000 AS INTEGER) END WHERE id=?1 AND state=?4",
            rusqlite::params![id, serde_json::to_string(&to)?, matches!(to, State::Committed | State::Failed(_)), serde_json::to_string(&from)?],
        )? == 0 {
            return Err(LedgerError::Illegal { from: self.get(id)?.state, to });
        }
        Ok(())
    }
    pub fn approve(&self, id: i64) -> Result<(), LedgerError> {
        let task = self.get(id)?;
        if task.state != State::AwaitingHandoff(Handoff::Approval) {
            return Err(LedgerError::Illegal {
                from: task.state,
                to: State::Queued,
            });
        }
        if self.conn.execute(
            "UPDATE tasks SET state=?2, approved_step=cursor, finished_ms=NULL WHERE id=?1 AND state=?3",
            rusqlite::params![id, serde_json::to_string(&State::Queued)?, serde_json::to_string(&State::AwaitingHandoff(Handoff::Approval))?],
        )? == 0 {
            return Err(LedgerError::Illegal { from: self.get(id)?.state, to: State::Queued });
        }
        Ok(())
    }
    pub fn resume(&self, id: i64) -> Result<(), LedgerError> {
        let task = self.get(id)?;
        if !matches!(task.state, State::AwaitingHandoff(Handoff::Challenge(_))) {
            return Err(LedgerError::Illegal {
                from: task.state,
                to: State::Queued,
            });
        }
        self.transition_from(id, task.state, State::Queued)
    }
    pub fn recover(&self) -> Result<usize, LedgerError> {
        Ok(self.conn.execute(
            "UPDATE tasks SET state=?1 WHERE state=?2",
            rusqlite::params![
                serde_json::to_string(&State::Queued)?,
                serde_json::to_string(&State::Executing)?
            ],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
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
    fn stale_handle_transition_rejected() {
        let dir = temp_dir("ledger-stale");
        let path = dir.0.join("ledger.db");
        let first = Ledger::open(&path).unwrap();
        let second = Ledger::open(&path).unwrap();
        let id = first.add("x", &[]).unwrap();
        first.transition(id, State::Executing).unwrap();
        assert!(matches!(
            second.transition(id, State::Executing),
            Err(LedgerError::Illegal {
                from: State::Executing,
                to: State::Executing
            })
        ));
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
            assert!(matches!(error, LedgerError::NotFound(-1)));
        }
        drop(ledger);
        assert_eq!(Ledger::open(path).unwrap().get(id).unwrap().title, "first");
    }
    #[test]
    fn happy_path_transitions() {
        let dir = temp_dir("ledger-transitions");
        let ledger = Ledger::open(dir.0.join("ledger.db")).unwrap();
        // Rows and columns: Queued, Executing, approval wait, challenge wait, Committed, Failed.
        let expected = [
            [false, true, false, false, false, false],
            [true, false, true, true, true, true],
            [true, false, false, false, false, true],
            [true, false, false, false, false, true],
            [false; 6],
            [false; 6],
        ];
        for (from_index, from) in states().into_iter().enumerate() {
            for (to_index, to) in states().into_iter().enumerate() {
                let id = ledger.add("matrix", &[]).unwrap();
                if from != State::Queued {
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
                    assert!(
                        matches!(result, Err(LedgerError::Illegal { from: f, to: t }) if f == from && t == to)
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
                assert!(matches!(
                    ledger.transition(id, to),
                    Err(LedgerError::Illegal { .. })
                ));
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
}
