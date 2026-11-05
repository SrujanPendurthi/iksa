use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Read = 0,
    Write = 1,
    Financial = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Auto,
    NeedsApproval,
}

/// Financial steps always need sign-off, whatever `auto_max` says.
pub fn decide(auto_max: Risk, r: Risk) -> Decision {
    if r == Risk::Financial || r > auto_max {
        Decision::NeedsApproval
    } else {
        Decision::Auto
    }
}

/// A standing grant can authorize routine writes, never reads or financial actions.
pub fn grant_for(
    grants: &[crate::ledger::Grant],
    host: &str,
    schedule_id: Option<i64>,
    risk: Risk,
    now_ms: u64,
    cfg: &crate::config::Config,
) -> Option<i64> {
    use crate::config::domain_matches;
    if risk != Risk::Write
        || cfg
            .never_send
            .iter()
            .any(|suffix| domain_matches(host, suffix))
    {
        return None;
    }
    let policy = cfg.policy_fingerprint();
    grants
        .iter()
        .find(|grant| {
            domain_matches(host, &grant.domain)
                && grant.schedule_id.is_none_or(|id| Some(id) == schedule_id)
                && grant.expires_ms > now_ms
                && grant.policy == policy
        })
        .map(|grant| grant.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn read_auto_write_needs_approval_by_default() {
        let auto_max = Config::default().auto_max;
        assert_eq!(auto_max, Risk::Read);
        assert_eq!(decide(auto_max, Risk::Read), Decision::Auto);
        assert_eq!(decide(auto_max, Risk::Write), Decision::NeedsApproval);
        assert_eq!(decide(auto_max, Risk::Financial), Decision::NeedsApproval);
    }

    #[test]
    fn financial_always_needs_approval() {
        assert_eq!(
            decide(Risk::Financial, Risk::Financial),
            Decision::NeedsApproval
        );
        assert_eq!(decide(Risk::Financial, Risk::Write), Decision::Auto);
        assert_eq!(decide(Risk::Financial, Risk::Read), Decision::Auto);
    }
    #[test]
    fn grant_scope_is_enforced() {
        use crate::ledger::Grant;
        let cfg = Config::default();
        let base = Grant {
            id: 7,
            domain: "shop.example".into(),
            schedule_id: None,
            expires_ms: 101,
            policy: cfg.policy_fingerprint(),
        };
        let check = |grant: Grant, host, schedule, risk, now, cfg: &Config| {
            grant_for(&[grant], host, schedule, risk, now, cfg)
        };
        assert_eq!(
            check(base.clone(), "shop.example", None, Risk::Write, 100, &cfg),
            Some(7)
        );
        assert_eq!(
            check(
                base.clone(),
                "a.shop.example",
                Some(9),
                Risk::Write,
                100,
                &cfg
            ),
            Some(7)
        );
        for host in ["evilshop.example", "shop.example.evil", ""] {
            assert_eq!(
                check(base.clone(), host, None, Risk::Write, 100, &cfg),
                None
            );
        }
        for risk in [Risk::Financial, Risk::Read] {
            assert_eq!(
                check(base.clone(), "shop.example", None, risk, 100, &cfg),
                None
            );
        }
        for now in [101, 102] {
            assert_eq!(
                check(base.clone(), "shop.example", None, Risk::Write, now, &cfg),
                None
            );
        }
        let bound = Grant {
            schedule_id: Some(3),
            ..base.clone()
        };
        assert_eq!(
            check(
                bound.clone(),
                "shop.example",
                Some(3),
                Risk::Write,
                100,
                &cfg
            ),
            Some(7)
        );
        for schedule in [None, Some(4)] {
            assert_eq!(
                check(
                    bound.clone(),
                    "shop.example",
                    schedule,
                    Risk::Write,
                    100,
                    &cfg
                ),
                None
            );
        }
        let mut blocked = cfg.clone();
        blocked.never_send.push("shop.example".into());
        // Even a newly issued grant cannot override never_send.
        let blocked_grant = Grant {
            policy: blocked.policy_fingerprint(),
            ..base.clone()
        };
        assert_eq!(
            check(
                blocked_grant,
                "a.shop.example",
                None,
                Risk::Write,
                100,
                &blocked
            ),
            None
        );
        assert_eq!(
            check(
                base.clone(),
                "shop.example",
                None,
                Risk::Write,
                100,
                &blocked
            ),
            None
        );
        let mut changed = cfg.clone();
        changed.auto_max = Risk::Write;
        assert_eq!(
            check(base, "shop.example", None, Risk::Write, 100, &changed),
            None
        );
        assert_eq!(
            grant_for(&[], "shop.example", None, Risk::Write, 100, &cfg),
            None
        );
    }
}
