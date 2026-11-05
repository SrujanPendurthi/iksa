// M2b Task 6: standing grants. Enabled from tests/acceptance_m2b.rs.
use super::*;
use iksa::gate::grant_for;

fn grant(
    domain: &str,
    schedule_id: Option<i64>,
    expires_ms: u64,
    policy: &str,
) -> iksa::ledger::Grant {
    iksa::ledger::Grant {
        id: 1,
        domain: domain.into(),
        schedule_id,
        expires_ms,
        policy: policy.into(),
    }
}
#[test]
fn a6_grant_scope_is_enforced() {
    let cfg = Config::default();
    let fp = cfg.policy_fingerprint();
    let now = 1_000;
    let any = [grant("shop.example", None, now + HOUR, &fp)];
    assert_eq!(
        grant_for(&any, "shop.example", None, Risk::Write, now, &cfg),
        Some(1)
    );
    assert_eq!(
        grant_for(&any, "a.shop.example", Some(4), Risk::Write, now, &cfg),
        Some(1)
    );
    assert_eq!(
        grant_for(&any, "evilshop.example", None, Risk::Write, now, &cfg),
        None,
        "suffix without dot"
    );
    assert_eq!(
        grant_for(&any, "shop.example", None, Risk::Financial, now, &cfg),
        None,
        "financial used a grant"
    );
    assert_eq!(
        grant_for(&any, "shop.example", None, Risk::Read, now, &cfg),
        None
    );
    assert_eq!(
        grant_for(&any, "shop.example", None, Risk::Write, now + HOUR, &cfg),
        None,
        "expired grant used"
    );
    let bound = [grant("shop.example", Some(7), now + HOUR, &fp)];
    assert_eq!(
        grant_for(&bound, "shop.example", Some(7), Risk::Write, now, &cfg),
        Some(1)
    );
    assert_eq!(
        grant_for(&bound, "shop.example", Some(8), Risk::Write, now, &cfg),
        None
    );
    assert_eq!(
        grant_for(&bound, "shop.example", None, Risk::Write, now, &cfg),
        None
    );
    let blocked = Config {
        never_send: vec!["shop.example".into()],
        ..Config::default()
    };
    assert_eq!(
        grant_for(&any, "shop.example", None, Risk::Write, now, &blocked),
        None,
        "never_send added later"
    );
    let looser = Config {
        auto_max: Risk::Write,
        ..Config::default()
    };
    assert_ne!(looser.policy_fingerprint(), fp);
    assert_eq!(
        grant_for(&any, "shop.example", None, Risk::Write, now, &looser),
        None,
        "auto_max changed"
    );
}
#[test]
fn a6_grant_cli() {
    let h = TempDir::new();
    ok(run(&h, &["list"]));
    assert!(ok(run(&h, &["grant", "shop.example", "--days", "3"])).contains("expires "));
    assert!(ok(run(&h, &["grant", "list"])).contains("shop.example"));
    for args in [
        &["grant", "x.example", "--days", "31"][..],
        &["grant", "x.example", "--days", "0"][..],
        &["grant", "x.example", "--risk", "financial"][..],
    ] {
        assert_eq!(run(&h, args).status.code(), Some(2), "{args:?} accepted");
    }
    assert_eq!(ledger(&h).grants().unwrap().len(), 1);
    ok(run(&h, &["grant", "rm", "1"]));
    assert!(ledger(&h).grants().unwrap().is_empty());
    ok(run(&h, &["grant", "e.example"]));
    let e = ledger(&h)
        .grants()
        .unwrap()
        .into_iter()
        .find(|g| g.domain == "e.example")
        .unwrap();
    let days = (e.expires_ms - iksa::audit::now_ms()) as f64 / (24.0 * HOUR as f64);
    assert!(
        (6.9..=7.0).contains(&days),
        "default grant lasts {days} days, expected 7"
    );
}
