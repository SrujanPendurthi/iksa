use crate::config::Config;
use anyhow::Context;
use serde_json::{Value, json};
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub const EXTENSION_ID: &str = "fedajodcbhcmcgboakaacifkdogfdhil";
const BASE: &str = include_str!("../extension/manifest.base.json");
const BACKGROUND: &str = include_str!("../extension/background.js");

fn search_origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.chars().any(char::is_whitespace) {
        return None;
    }
    Some(format!("{scheme}://{authority}"))
}

pub fn manifest(cfg: &Config) -> Value {
    let mut manifest: Value = serde_json::from_str(BASE).expect("embedded extension manifest");
    let provider = cfg
        .providers
        .iter()
        .find(|p| p.name == cfg.search)
        .expect("active search provider");
    // Chromium requires an HTTP(S) favicon for a custom search provider.
    let favicon = search_origin(&provider.url_template)
        .map(|origin| format!("{origin}/favicon.ico"))
        .unwrap_or_default();
    manifest["chrome_settings_overrides"] = json!({"search_provider":{
        "name":provider.name, "keyword":"iksa",
        "search_url":provider.url_template.replace("{q}", "{searchTerms}"),
        "encoding":"UTF-8", "is_default":true, "favicon_url":favicon
    }});
    manifest
}

/// Install managed extension files and an owner-only native-host registration.
pub fn install(home: &Path, cfg: &Config, exe: &Path) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(
        exe.is_absolute(),
        "native host executable path must be absolute"
    );
    let exe = exe
        .to_str()
        .context("native host executable path must be UTF-8")?;
    let provider = cfg
        .providers
        .iter()
        .find(|p| p.name == cfg.search)
        .context("active search provider missing")?;
    anyhow::ensure!(
        search_origin(&provider.url_template).is_some(),
        "extension search provider requires an absolute HTTP(S) URL"
    );
    let extension = home.join("extension");
    let profile = home.join("profile");
    let hosts = profile.join("NativeMessagingHosts");
    for dir in [
        home,
        extension.as_path(),
        profile.as_path(),
        hosts.as_path(),
    ] {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    for (path, body) in [
        (
            extension.join("manifest.json"),
            serde_json::to_string_pretty(&manifest(cfg))?,
        ),
        (extension.join("background.js"), BACKGROUND.to_owned()),
        (
            hosts.join("dev.iksa.host.json"),
            serde_json::to_string_pretty(&json!({
                "name":"dev.iksa.host", "description":"iksa native messaging host",
                "path":exe, "type":"stdio", "allowed_origins":[format!("chrome-extension://{EXTENSION_ID}/")]
            }))?,
        ),
    ] {
        std::fs::write(&path, body)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(extension)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, testutil::temp_dir};
    use serde_json::{Value, json};
    use std::{
        io::Write,
        os::unix::fs::PermissionsExt,
        path::Path,
        process::{Command, Stdio},
    };

    #[test]
    fn manifest_follows_search_config() {
        let cfg = Config {
            search: "Kagi".into(),
            ..Config::default()
        };
        let m = manifest(&cfg);
        assert_eq!(m["manifest_version"], 3);
        assert_eq!(m["permissions"], json!(["nativeMessaging", "sidePanel"]));
        assert!(m.get("host_permissions").is_none());
        assert!(m.get("externally_connectable").is_none());
        let p = &m["chrome_settings_overrides"]["search_provider"];
        assert_eq!(p["search_url"], "https://kagi.com/search?q={searchTerms}");
        assert_eq!(p["is_default"], true);
        assert_eq!(p["name"], "Kagi");
        assert_eq!(p["encoding"], "UTF-8");
        assert_eq!(p["favicon_url"], "https://kagi.com/favicon.ico");
        assert!(p["keyword"].as_str().is_some_and(|s| !s.is_empty()));
    }
    #[test]
    fn install_rejects_invalid_search_urls_without_writes() {
        let home = temp_dir("extension-invalid-provider");
        let exe = std::env::current_exe().unwrap();
        for url in ["search?q={q}", "ftp://example.com/?q={q}", "https://?q={q}"] {
            let mut cfg = Config::default();
            cfg.providers[0].url_template = url.into();
            assert!(install(&home.0, &cfg, &exe).is_err(), "accepted {url}");
            assert!(!home.0.join("extension").exists());
        }
    }
    fn command_input(program: &str, args: &[&str], input: &[u8]) -> Vec<u8> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }
    #[test]
    fn extension_id_matches_key() {
        let m = manifest(&Config::default());
        let der = command_input(
            "openssl",
            &["base64", "-d", "-A"],
            m["key"].as_str().unwrap().as_bytes(),
        );
        assert!(!der.is_empty());
        let hash = command_input("shasum", &["-a", "256"], &der);
        let hash = String::from_utf8(hash).unwrap();
        let derived: String = hash[..32]
            .chars()
            .map(|c| char::from(b'a' + c.to_digit(16).unwrap() as u8))
            .collect();
        assert_eq!(EXTENSION_ID, derived);
    }
    #[test]
    fn manifests_restrict_origins() {
        let home = temp_dir("extension-install");
        let exe = std::env::current_exe().unwrap();
        let cfg = Config {
            search: "Kagi".into(),
            ..Config::default()
        };
        let dir = install(&home.0, &cfg, &exe).unwrap();
        assert_eq!(dir, home.0.join("extension"));
        let installed: Value =
            serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(installed, manifest(&cfg));
        assert!(
            !std::fs::read_to_string(dir.join("background.js"))
                .unwrap()
                .is_empty()
        );
        let host: Value = serde_json::from_slice(
            &std::fs::read(
                home.0
                    .join("profile/NativeMessagingHosts/dev.iksa.host.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(host["name"], "dev.iksa.host");
        assert_eq!(host["type"], "stdio");
        assert_eq!(
            host["allowed_origins"],
            json!([format!("chrome-extension://{EXTENSION_ID}/")])
        );
        assert_eq!(host["path"].as_str(), exe.to_str());
        assert!(Path::new(host["path"].as_str().unwrap()).is_absolute());
        for path in [&home.0, &home.0.join("profile"), &dir] {
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        assert!(install(&home.0, &cfg, Path::new("relative/iksa")).is_err());
        let default = Config::default();
        install(&home.0, &default, &exe).unwrap();
        let replaced: Value =
            serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(replaced, manifest(&default));
    }
}
