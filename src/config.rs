use crate::gate::Risk;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::{os::unix::fs::DirBuilderExt, path::Path};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    pub provider: String,
    #[serde(default = "model_endpoint")]
    pub endpoint: String,
    pub name: String,
    #[serde(default = "model_effort")]
    pub effort: String,
    #[serde(default = "model_max_steps")]
    pub max_steps: u32,
    #[serde(default = "model_fallbacks")]
    pub fallbacks: bool,
}

fn model_endpoint() -> String {
    "https://api.anthropic.com".into()
}
fn model_effort() -> String {
    "low".into()
}
fn model_max_steps() -> u32 {
    25
}
fn model_fallbacks() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchProvider {
    pub name: String,
    /// Must contain `{q}`.
    pub url_template: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub model: Option<ModelConfig>,
    #[serde(default)]
    pub never_send: Vec<String>,
    /// Name of the active provider.
    pub search: String,
    pub providers: Vec<SearchProvider>,
    pub auto_max: Risk,
    pub retention_days: u32,
    #[serde(default = "default_max_agents")]
    pub max_agents: u32,
}

fn default_max_agents() -> u32 {
    3
}

impl Default for Config {
    fn default() -> Self {
        let provider = |name: &str, url_template: &str| SearchProvider {
            name: name.into(),
            url_template: url_template.into(),
        };
        Config {
            model: None,
            never_send: Vec::new(),
            search: "DuckDuckGo".into(),
            providers: vec![
                provider("DuckDuckGo", "https://duckduckgo.com/?q={q}"),
                provider("Kagi", "https://kagi.com/search?q={q}"),
                provider("Brave", "https://search.brave.com/search?q={q}"),
                provider("SearXNG", "https://searx.be/search?q={q}"),
            ],
            auto_max: Risk::Read,
            retention_days: 30,
            max_agents: default_max_agents(),
        }
    }
}

impl Config {
    /// Reads `home/config.json`, writing the default first if it is missing.
    pub fn load(home: &Path) -> anyhow::Result<Config> {
        let path = home.join("config.json");
        if !path.try_exists()? {
            let config = Config::default();
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(home)?;
            std::fs::write(&path, serde_json::to_string_pretty(&config)?)?;
            return Ok(config);
        }
        let mut config: Config = serde_json::from_str(&std::fs::read_to_string(&path)?)
            .with_context(|| path.display().to_string())?;
        if let Some(p) = config
            .providers
            .iter()
            .find(|p| !p.url_template.contains("{q}"))
        {
            anyhow::bail!(
                "search provider \"{}\": url_template must contain {{q}}",
                p.name
            );
        }
        if !config.providers.iter().any(|p| p.name == config.search) {
            anyhow::bail!(
                "active search provider \"{}\" is not in providers",
                config.search
            );
        }
        anyhow::ensure!(
            (1..=8).contains(&config.max_agents),
            "max_agents must be between 1 and 8"
        );
        if config.retention_days == 0 {
            anyhow::bail!("retention_days must be greater than zero");
        }
        if let Some(model) = &config.model {
            anyhow::ensure!(model.provider == "anthropic", "only anthropic in M2a");
            anyhow::ensure!(
                matches!(
                    model.effort.as_str(),
                    "low" | "medium" | "high" | "xhigh" | "max"
                ),
                "invalid model effort: {}",
                model.effort
            );
        }
        for host in &mut config.never_send {
            host.make_ascii_lowercase();
        }
        Ok(config)
    }

    /// Active provider's URL for `q`. `load` guarantees the provider exists.
    pub fn search_url(&self, q: &str) -> String {
        let provider = self.providers.iter().find(|p| p.name == self.search);
        let template = provider
            .expect("active search provider")
            .url_template
            .as_str();
        let encoded = q
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect::<String>();
        template.replace("{q}", &encoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    fn write(home: &Path, json: &str) {
        std::fs::write(home.join("config.json"), json).unwrap();
    }

    fn custom(search: &str, template: &str) -> String {
        serde_json::json!({
            "search": search,
            "providers": [{"name": "Mine", "url_template": template}],
            "auto_max": "read",
            "retention_days": 30
        })
        .to_string()
    }

    #[test]
    fn default_config_written_and_reloaded() {
        use std::os::unix::fs::PermissionsExt;
        let temp = temp_dir("config");
        let home = temp.0.join("home");
        let first = Config::load(&home).unwrap();
        assert_eq!(
            std::fs::metadata(&home).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(first, Config::default());
        assert!(home.join("config.json").exists());
        assert_eq!(Config::load(&home).unwrap(), first);
        let names: Vec<_> = first.providers.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["DuckDuckGo", "Kagi", "Brave", "SearXNG"]);
        assert_eq!(first.search, "DuckDuckGo");
        assert_eq!(first.retention_days, 30);
    }

    #[test]
    fn zero_retention_rejected() {
        let home = temp_dir("config");
        let mut json = serde_json::to_value(Config::default()).unwrap();
        json["retention_days"] = 0.into();
        write(&home.0, &json.to_string());
        let error = Config::load(&home.0).unwrap_err().to_string();
        assert!(error.contains("retention_days"), "{error}");
    }

    #[test]
    fn kagi_search_url() {
        let cfg = Config {
            search: "Kagi".into(),
            ..Config::default()
        };
        assert_eq!(
            cfg.search_url("rust & c++"),
            "https://kagi.com/search?q=rust%20%26%20c%2B%2B"
        );
        assert_eq!(
            Config::default().search_url("a-b_c.d~é"),
            "https://duckduckgo.com/?q=a-b_c.d~%C3%A9"
        );
    }

    #[test]
    fn custom_provider_without_q_rejected() {
        let home = temp_dir("config");
        write(&home.0, &custom("Mine", "https://example.com/search"));
        let error = Config::load(&home.0).unwrap_err().to_string();
        assert!(error.contains("Mine") && error.contains("{q}"), "{error}");
        write(&home.0, &custom("Mine", "https://example.com/search?q={q}"));
        assert_eq!(
            Config::load(&home.0).unwrap().search_url("x"),
            "https://example.com/search?q=x"
        );
    }

    #[test]
    fn unknown_active_provider_rejected() {
        let home = temp_dir("config");
        write(&home.0, &custom("Nope", "https://example.com/?q={q}"));
        let error = Config::load(&home.0).unwrap_err().to_string();
        assert!(error.contains("Nope"), "{error}");
    }

    #[test]
    fn unknown_config_field_rejected() {
        let home = temp_dir("config");
        let mut json: serde_json::Value = serde_json::to_value(Config::default()).unwrap();
        json["bogus"] = 1.into();
        write(&home.0, &json.to_string());
        let error = format!("{:#}", Config::load(&home.0).unwrap_err());
        assert!(error.contains("bogus"), "{error}");
    }
    #[test]
    fn model_defaults_none() {
        let home = temp_dir("model-config");
        write(&home.0, &custom("Mine", "https://example.com/?q={q}"));
        let cfg = Config::load(&home.0).unwrap();
        assert!(cfg.model.is_none());
        assert!(cfg.never_send.is_empty());
        let mut json: serde_json::Value =
            serde_json::from_str(&custom("Mine", "https://example.com/?q={q}")).unwrap();
        json["model"] = serde_json::json!({"provider":"anthropic","name":"test-model"});
        json["never_send"] = serde_json::json!(["BANK.EXAMPLE"]);
        write(&home.0, &json.to_string());
        let cfg = Config::load(&home.0).unwrap();
        let model = cfg.model.unwrap();
        assert_eq!(model.endpoint, "https://api.anthropic.com");
        assert_eq!(model.effort, "low");
        assert_eq!(model.max_steps, 25);
        assert!(model.fallbacks);
        assert_eq!(cfg.never_send, ["bank.example"]);
    }
    #[test]
    fn bad_effort_rejected() {
        let home = temp_dir("model-config");
        for (provider, effort, expected) in [
            ("anthropic", "extreme", "effort"),
            ("other", "low", "only anthropic in M2a"),
        ] {
            let mut json = serde_json::to_value(Config::default()).unwrap();
            json["model"] =
                serde_json::json!({"provider":provider,"name":"test-model","effort":effort});
            write(&home.0, &json.to_string());
            let err = Config::load(&home.0).unwrap_err().to_string();
            assert!(err.contains(expected), "{err}");
        }
    }
    #[test]
    fn max_agents_range() {
        let home = temp_dir("max-agents");
        let mut value = serde_json::to_value(Config::default()).unwrap();
        value.as_object_mut().unwrap().remove("max_agents");
        write(&home.0, &value.to_string());
        assert_eq!(Config::load(&home.0).unwrap().max_agents, 3);
        for n in [0, 9] {
            value["max_agents"] = n.into();
            write(&home.0, &value.to_string());
            assert!(
                Config::load(&home.0)
                    .unwrap_err()
                    .to_string()
                    .contains("max_agents")
            );
        }
        for n in [1, 8] {
            value["max_agents"] = n.into();
            write(&home.0, &value.to_string());
            assert_eq!(Config::load(&home.0).unwrap().max_agents, n);
        }
    }
}
