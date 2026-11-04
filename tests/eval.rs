mod common;
#[path = "eval/support.rs"]
mod support;
use iksa::ledger::{Handoff, State};
use serde_json::json;
use support::*;

#[test]
fn manifest_has_twenty_goals_and_fifteen_local_cases() {
    let cases = cases().unwrap();
    assert_eq!(cases.len(), 20);
    assert_eq!(cases.iter().filter(|c| c.local).count(), 15);
    assert_eq!(
        cases
            .iter()
            .map(|c| &c.id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        20
    );
    let routes = fixture_routes().unwrap();
    for case in cases.iter().filter(|c| c.local) {
        assert!(
            routes.iter().any(|(path, _)| path == &case.start),
            "missing route {}",
            case.start
        );
    }
}
#[test]
fn grading_requires_state_answer_and_evidence() {
    let mut case = Case {
        id: "x".into(),
        title: "fixture".into(),
        local: true,
        start: "/".into(),
        goal: "goal".into(),
        expect: "done".into(),
        answer: Some("NOVA-42".into()),
        evidence: None,
        reference_js: None,
    };
    assert!(grade(
        &case,
        Some(&State::Committed),
        "done: Found NOVA-42",
        &[]
    ));
    assert!(!grade(&case, Some(&State::Committed), "done: guessed", &[]));
    assert!(!grade(
        &case,
        Some(&State::Failed("oops".into())),
        "done: NOVA-42",
        &[]
    ));
    case.evidence = Some("Email required".into());
    assert!(!grade(&case, Some(&State::Committed), "done: NOVA-42", &[]));
    assert!(grade(
        &case,
        Some(&State::Committed),
        "done: NOVA-42",
        &[json!({"messages":[{"content":"Email required"}]})]
    ));
    case.expect = "approval".into();
    assert!(grade(
        &case,
        Some(&State::AwaitingHandoff(Handoff::Approval)),
        "",
        &[]
    ));
    assert!(!grade(
        &case,
        Some(&State::Committed),
        "done: approval",
        &[]
    ));
}
#[test]
fn usage_prices_cache_categories_without_double_counting() {
    let m = measure(
        &json!({"model":"claude-opus-5-5","usage":{"input_tokens":1000000,"output_tokens":1000000,"cache_creation_input_tokens":1000000,"cache_read_input_tokens":1000000}}),
    );
    assert_eq!(m.input, Some(3000000));
    assert_eq!(m.output, Some(1000000));
    assert!((m.usd.unwrap() - 29.2).abs() < 0.000001);
    assert!(measure(&json!({"error":{"type":"bad"}})).usd.is_none());
    let incomplete = measure(
        &json!({"model":"claude-opus-5-5","usage":{"iterations":[{"type":"message","model":"claude-opus-5-5","input_tokens":7,"output_tokens":2}]}}),
    );
    assert_eq!(incomplete.input, Some(7));
    assert!(incomplete.usd.is_none());
    let fallback = measure(
        &json!({"model":"claude-opus-5-5","usage":{"input_tokens":5,"output_tokens":2,"iterations":[{"model":"claude-opus-5-5","input_tokens":7,"output_tokens":0},{"model":"claude-sonnet-5-5","input_tokens":5,"output_tokens":2}]}}),
    );
    assert_eq!(fallback.input, Some(12));
    assert_eq!(fallback.output, Some(2));
    assert!(fallback.usd.is_none());
}
#[test]
fn estimate_states_its_token_assumptions() {
    let text = estimate();
    assert!(text.contains("160"));
    assert!(text.contains("960000"));
    assert!(text.contains("4.52"));
    assert!(text.contains("5.12"));
    println!("{text}");
}
#[test]
fn relay_mock_records_wire_body_and_usage() {
    let relay = Relay::mock();
    let cfg = relay.config();
    let reply = iksa::llm::call(
        &cfg,
        "offline-key",
        &json!({"messages":[{"role":"user","content":"fixture"}]}),
    )
    .unwrap();
    assert_eq!(reply.tool.as_ref().unwrap().0, "done");
    let events = relay.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].request["messages"][0]["content"], "fixture");
    assert_eq!(events[0].measurement.input, Some(10));
}

#[test]
fn external_eval_requires_write_approval() {
    use iksa::gate::{self, Decision, Risk};
    for case in cases().unwrap().iter().filter(|c| !c.local) {
        assert_eq!(
            gate::decide(case_risk(case), Risk::Write),
            Decision::NeedsApproval,
            "external case {} can write",
            case.id
        );
    }
}
#[test]
fn relay_drains_late_exchange_before_reporting() {
    use std::{
        io::Write,
        net::TcpStream,
        sync::atomic::Ordering,
        thread,
        time::{Duration, Instant},
    };
    let (relay, started, release) = Relay::held_mock();
    let stop = relay.stop_signal();
    let mut stream =
        TcpStream::connect(relay.config().endpoint.strip_prefix("http://").unwrap()).unwrap();
    stream
        .write_all(b"POST /v1/messages HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{}")
        .unwrap();
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(stream);
    let reporter = thread::spawn(move || relay.finish());
    let deadline = Instant::now() + Duration::from_secs(2);
    while !stop.load(Ordering::Relaxed) {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    release.send(()).unwrap();
    let events = reporter.join().unwrap();
    assert_eq!(events.len(), 1, "late upstream usage was omitted");
    assert_eq!(events[0].measurement.input, Some(10));
}
#[test]
fn relay_preserves_body_failure_retries() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let upstream = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0; 8192];
        let _ = stream.read(&mut buf).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n{")
            .unwrap();
    });
    let relay = Relay::forward_mock(&endpoint);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .proxy(None)
        .build()
        .into();
    let response = agent
        .post(format!("{}/v1/messages", relay.config().endpoint))
        .header("x-api-key", "offline-key")
        .header("anthropic-version", "2026-10-21")
        .send_json(&json!({}))
        .unwrap();
    assert_eq!(
        response.status().as_u16(),
        502,
        "truncated upstream body must remain retryable"
    );
    upstream.join().unwrap();
    let events = relay.finish();
    assert!(events[0].measurement.usd.is_none());
}

#[test]
fn relay_rejects_malformed_request() {
    use std::{
        io::{Read, Write},
        net::TcpStream,
        time::Duration,
    };
    let relay = Relay::mock();
    let mut stream =
        TcpStream::connect(relay.config().endpoint.strip_prefix("http://").unwrap()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 400"), "{response}");
    iksa::llm::call(&relay.config(), "offline-key", &json!({"messages":[]})).unwrap();
    let events = relay.finish();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].status, 400);
    assert_eq!(events[0].request, serde_json::Value::Null);
}
#[test]
fn grading_numeric_answers_need_whole_number() {
    let mut case = cases()
        .unwrap()
        .into_iter()
        .find(|c| c.expect == "done")
        .unwrap();
    case.evidence = None;
    for (expected, answer, correct) in [
        ("19", "done: 19 credits", true),
        ("19", "done: 119 credits", false),
        ("19", "done: price 19.", true),
        ("19", "done: 190 credits", false),
        ("09:40", "done: departs 09:40", true),
        ("09:40", "done: 109:40", false),
        ("NOVA-42", "done: Found NOVA-42", true),
        ("NOVA-42", "done: prefixNOVA-42suffix", true),
        ("NOVA-42", "done: guessed", false),
    ] {
        case.answer = Some(expected.into());
        assert_eq!(
            grade(&case, Some(&State::Committed), answer, &[]),
            correct,
            "{expected}: {answer}"
        );
    }
}
#[test]
fn relay_timeout_below_client_timeout() {
    assert!(support::RELAY_UPSTREAM_TIMEOUT < iksa::llm::CLIENT_TIMEOUT);
}
#[test]
fn external_order_names_every_external_case() {
    let cases = cases().unwrap();
    let mut ids: Vec<_> = cases
        .iter()
        .filter(|c| !c.local)
        .map(|c| c.id.as_str())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids.len(), 5);
    for mut order in [support::CAPTURE_ORDER, support::RUN_ORDER] {
        order.sort_unstable();
        assert_eq!(order.as_slice(), ids);
    }
    assert_eq!(&support::RUN_ORDER[..2], &["hacker-news", "duckduckgo"]);
    assert_eq!(&support::CAPTURE_ORDER[3..], &["duckduckgo", "hacker-news"]);
}

#[tokio::test]
#[ignore]
async fn offline_eval_harness() {
    run_isolated(false).await.unwrap();
}
#[tokio::test]
#[ignore]
async fn live_eval() {
    run_isolated(true).await.unwrap();
}
