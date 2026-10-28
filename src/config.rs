use crate::gate::Risk;
use serde::{Deserialize, Serialize};
use std::path::Path;

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
    /// Name of the active provider.
    pub search: String,
    pub providers: Vec<SearchProvider>,
    pub auto_max: Risk,
    pub retention_days: u32,
}

impl Default for Config {
    fn default() -> Self {
        let provider = |name: &str, url_template: &str| SearchProvider {
            name: name.into(),
            url_template: url_template.into(),
        };
        Config {
            search: "DuckDuckGo".into(),
            providers: vec![
                provider("DuckDuckGo", "https://duckduckgo.com/?q={q}"),
                provider("Kagi", "https://kagi.com/search?q={q}"),
                provider("Brave", "https://search.brave.com/search?q={q}"),
                provider("SearXNG", "https://searx.be/search?q={q}"),
            ],
            auto_max: Risk::Read,
            retention_days: 30,
        }
    }
}

impl Config {
    /// Reads `home/config.json`, writing the default first if it is missing.
    pub fn load(home: &Path) -> anyhow::Result<Config> {
        let path = home.join("config.json");
        if !path.try_exists()? {
            let config = Config::default();
            std::fs::create_dir_all(home)?;
            std::fs::write(&path, serde_json::to_string_pretty(&config)?)?;
            return Ok(config);
        }
        let config: Config = serde_json::from_str(&std::fs::read_to_string(&path)?)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
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
        Ok(config)
    }

    /// Active provider's URL for `q`. `load` guarantees the provider exists.
    pub fn search_url(&self, q: &str) -> String {
        let provider = self.providers.iter().find(|p| p.name == self.search);
        let template = provider
            .expect("active search provider")
            .url_template
            .as_str();
        let mut encoded = String::new();
        for b in q.bytes() {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                encoded.push(b as char);
            } else {
                encoded.push_str(&format!("%{b:02X}"));
            }
        }
        template.replace("{q}", &encoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Home(PathBuf);
    impl Home {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            Self(std::env::temp_dir().join(format!(
                "iksa-config-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
        fn write(&self, json: &str) {
            std::fs::create_dir_all(&self.0).unwrap();
            std::fs::write(self.0.join("config.json"), json).unwrap();
        }
    }
    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
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
        let home = Home::new();
        let first = Config::load(&home.0).unwrap();
        assert_eq!(first, Config::default());
        assert!(home.0.join("config.json").exists());
        assert_eq!(Config::load(&home.0).unwrap(), first);
        let names: Vec<_> = first.providers.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["DuckDuckGo", "Kagi", "Brave", "SearXNG"]);
        assert_eq!(first.search, "DuckDuckGo");
        assert_eq!(first.retention_days, 30);
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
        let home = Home::new();
        home.write(&custom("Mine", "https://example.com/search"));
        let error = Config::load(&home.0).unwrap_err().to_string();
        assert!(error.contains("Mine") && error.contains("{q}"), "{error}");
        home.write(&custom("Mine", "https://example.com/search?q={q}"));
        assert_eq!(
            Config::load(&home.0).unwrap().search_url("x"),
            "https://example.com/search?q=x"
        );
    }

    #[test]
    fn unknown_active_provider_rejected() {
        let home = Home::new();
        home.write(&custom("Nope", "https://example.com/?q={q}"));
        let error = Config::load(&home.0).unwrap_err().to_string();
        assert!(error.contains("Nope"), "{error}");
    }

    #[test]
    fn unknown_config_field_rejected() {
        let home = Home::new();
        let mut json: serde_json::Value = serde_json::to_value(Config::default()).unwrap();
        json["bogus"] = 1.into();
        home.write(&json.to_string());
        let error = format!("{:#}", Config::load(&home.0).unwrap_err());
        assert!(error.contains("bogus"), "{error}");
    }
}
