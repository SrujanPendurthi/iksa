use crate::gate::Risk;
use serde::{Deserialize, Serialize};

fn write() -> Risk {
    Risk::Write
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Navigate {
        url: String,
    },
    Search {
        query: String,
    },
    Click {
        selector: String,
        #[serde(default = "write")]
        risk: Risk,
    },
    Type {
        selector: String,
        text: String,
        #[serde(default)]
        secret: bool,
        #[serde(default = "write")]
        risk: Risk,
    },
    Extract {
        selector: String,
    },
}

impl Action {
    pub fn risk(&self) -> Risk {
        match self {
            Action::Click { risk, .. } | Action::Type { risk, .. } => *risk,
            Action::Navigate { .. } | Action::Search { .. } | Action::Extract { .. } => Risk::Read,
        }
    }

    /// Copy safe to store or show: secret typed text becomes "••••".
    pub fn redacted(&self) -> Action {
        let mut action = self.clone();
        if let Action::Type {
            text, secret: true, ..
        } = &mut action
        {
            *text = "••••".into();
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn action_unknown_field_rejected() {
        assert!(
            serde_json::from_str::<Action>(
                r##"{"op":"click","selector":"#buy","rsik":"financial"}"##
            )
            .is_err()
        );
    }

    #[test]
    fn action_json_shape() {
        let click: Action =
            serde_json::from_str(r##"{"op":"click","selector":"#buy","risk":"financial"}"##)
                .unwrap();
        assert_eq!(
            click,
            Action::Click {
                selector: "#buy".into(),
                risk: Risk::Financial
            }
        );
        assert_eq!(click.risk(), Risk::Financial);
        let click: Action = serde_json::from_str(r#"{"op":"click","selector":"a"}"#).unwrap();
        assert_eq!(click.risk(), Risk::Write);
        let typed: Action =
            serde_json::from_str(r##"{"op":"type","selector":"#q","text":"hi"}"##).unwrap();
        assert_eq!(
            typed,
            Action::Type {
                selector: "#q".into(),
                text: "hi".into(),
                secret: false,
                risk: Risk::Write
            }
        );
        let nav = Action::Navigate {
            url: "https://example.com".into(),
        };
        assert_eq!(
            serde_json::to_value(&nav).unwrap(),
            json!({"op":"navigate","url":"https://example.com"})
        );
        assert_eq!(nav.risk(), Risk::Read);
        assert_eq!(Action::Search { query: "x".into() }.risk(), Risk::Read);
        assert_eq!(
            Action::Extract {
                selector: "h1".into()
            }
            .risk(),
            Risk::Read
        );
        assert!(serde_json::from_str::<Action>(r#"{"op":"fly"}"#).is_err());
    }

    #[test]
    fn redacted_hides_secret() {
        let secret = Action::Type {
            selector: "#pw".into(),
            text: "hunter2".into(),
            secret: true,
            risk: Risk::Write,
        };
        let json = serde_json::to_string(&secret.redacted()).unwrap();
        assert!(!json.contains("hunter2"), "{json}");
        assert!(json.contains("••••"), "{json}");
        let plain = Action::Type {
            selector: "#q".into(),
            text: "rust".into(),
            secret: false,
            risk: Risk::Write,
        };
        assert_eq!(plain.redacted(), plain);
    }
}
