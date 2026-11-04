use anyhow::Context;
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    process::Command,
};

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Only a complete, valid placeholder names a Keychain secret.
pub fn secret_name(text: &str) -> Option<&str> {
    text.strip_prefix("{{secret:")
        .and_then(|s| s.strip_suffix("}}"))
        .filter(|name| valid_name(name))
}

fn fake_path(service: &str, account: &str) -> anyhow::Result<Option<PathBuf>> {
    anyhow::ensure!(
        valid_name(service) && valid_name(account),
        "invalid Keychain service or account"
    );
    if std::env::var("IKSA_MOCK_KEYCHAIN").as_deref() != Ok("1") {
        return Ok(None);
    }
    let root = std::env::var_os("IKSA_KEYCHAIN_FAKE")
        .context("set IKSA_KEYCHAIN_FAKE for mock keychain")?;
    Ok(Some(PathBuf::from(root).join(service).join(account)))
}

pub fn get(service: &str, account: &str) -> anyhow::Result<String> {
    if let Some(path) = fake_path(service, account)? {
        return fs::read_to_string(path)
            .with_context(|| format!("Keychain entry {service}/{account} unavailable"));
    }
    let output = Command::new("security")
        .args(["find-generic-password", "-s", service, "-a", account, "-w"])
        .output()
        .context("reading Keychain")?;
    anyhow::ensure!(
        output.status.success(),
        "Keychain entry {service}/{account} unavailable"
    );
    Ok(String::from_utf8(output.stdout)
        .context("reading Keychain")?
        .trim_end_matches(['\r', '\n'])
        .to_owned())
}

pub fn set_interactive(service: &str, account: &str) -> anyhow::Result<()> {
    if let Some(path) = fake_path(service, account)? {
        let directory = path.parent().unwrap();
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)?;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        let mut value = String::new();
        io::stdin().read_line(&mut value)?;
        anyhow::ensure!(!value.is_empty(), "no secret on stdin");
        if value.ends_with('\n') {
            value.pop();
            if value.ends_with('\r') {
                value.pop();
            }
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(value.as_bytes())?;
        return Ok(());
    }
    // -w last delegates the private TTY prompt to security; the value is never in argv.
    let status = Command::new("security")
        .args([
            "add-generic-password",
            "-U",
            "-s",
            service,
            "-a",
            account,
            "-w",
        ])
        .status()
        .context("writing Keychain")?;
    anyhow::ensure!(
        status.success(),
        "could not set Keychain entry {service}/{account}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
    use std::{
        io::Write,
        os::unix::fs::PermissionsExt,
        process::{Command, Stdio},
    };

    #[test]
    fn fake_roundtrip() {
        if std::env::var("IKSA_KEYCHAIN_TEST_CHILD").as_deref() == Ok("roundtrip") {
            set_interactive("iksa", "anthropic").unwrap();
            assert_eq!(get("iksa", "anthropic").unwrap(), "fake-api-key");
            assert_eq!(crate::llm::api_key().unwrap(), "fake-api-key");
            return;
        }
        let dir = temp_dir("fake-keychain");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "keychain::tests::fake_roundtrip", "--nocapture"])
            .env("IKSA_KEYCHAIN_TEST_CHILD", "roundtrip")
            .env("IKSA_MOCK_KEYCHAIN", "1")
            .env("IKSA_KEYCHAIN_FAKE", &dir.0)
            .env_remove("ANTHROPIC_API_KEY")
            .env("PATH", "/nonexistent")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"fake-api-key\n")
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let file = dir.0.join("iksa/anthropic");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "fake-api-key");
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
