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
}
