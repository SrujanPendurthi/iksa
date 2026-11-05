// M2b Task 7: pause/resume. Enabled from tests/acceptance_m2b.rs.
// The stuck detector and pausing a running goal need the mock model and Chromium;
// they are covered by the plan's own planner tests.
use super::*;

#[test]
fn a7_pause_queued_task_and_resume() {
    let h = TempDir::new();
    ok(run(&h, &["list"]));
    let l = ledger(&h);
    let id = l.add("t", &[]).unwrap();
    ok(run(&h, &["pause", &id.to_string()]));
    assert_eq!(
        l.get(id).unwrap().state,
        State::AwaitingHandoff(Handoff::Paused)
    );
    assert_eq!(waiting_on(&l.get(id).unwrap()).as_deref(), Some("paused"));
    assert!(ok(run(&h, &["list"])).contains("paused"));
    ok(run(&h, &["resume", &id.to_string()]));
    assert_eq!(l.get(id).unwrap().state, State::Queued);
    l.transition(id, State::Executing).unwrap();
    l.transition(id, State::Committed).unwrap();
    let o = run(&h, &["pause", &id.to_string()]);
    assert_eq!(o.status.code(), Some(1), "paused a committed task");
    assert!(stderr(&o).contains("illegal transition"), "{}", stderr(&o));
    assert_eq!(run(&h, &["pause", "abc"]).status.code(), Some(2));
}
#[test]
fn a7_pause_request_on_executing_task_is_recorded() {
    let h = TempDir::new();
    let l = ledger(&h);
    let id = l.add("t", &[]).unwrap();
    l.transition(id, State::Executing).unwrap();
    l.request_pause(id).unwrap();
    assert_eq!(
        l.get(id).unwrap().state,
        State::Executing,
        "a running task must stop at its next step, not mid-step"
    );
}
