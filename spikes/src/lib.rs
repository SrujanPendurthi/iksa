use anyhow::Result;
use chromiumoxide::BrowserConfig;
pub fn config() -> Result<BrowserConfig> {
    let root = std::env::current_dir()?;
    let home = root.join(".spike-home");
    std::fs::create_dir_all(home.join("tmp"))?;
    let profile_name = std::env::var("SPIKE_PROFILE_NAME").unwrap_or("profile".into());
    anyhow::ensure!(
        !profile_name.is_empty()
            && profile_name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "SPIKE_PROFILE_NAME must be a single safe directory name"
    );
    let profile = home.join(profile_name);
    std::fs::create_dir_all(&profile)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o700))?;
    let mut builder = BrowserConfig::builder()
        .disable_default_args()
        .with_head()
        .env("HOME", home.display().to_string())
        .env("CFFIXED_USER_HOME", home.display().to_string())
        .env("TMPDIR", home.join("tmp").display().to_string())
        .chrome_executable(root.join(".spike-home/chromium/Chromium.app/Contents/MacOS/Chromium"))
        .user_data_dir(profile)
        .args([
            "use-mock-keychain",
            "remote-debugging-address=127.0.0.1",
            "disable-background-networking",
            "disable-component-update",
            "disable-sync",
            "no-pings",
            "disable-domain-reliability",
            "disable-field-trial-config",
            "disable-breakpad",
            "metrics-recording-only",
            "no-default-browser-check",
            "no-first-run",
            "safebrowsing-disable-auto-update",
        ]);
    if let Ok(ext) = std::env::var("SPIKE_EXTENSION") {
        builder = builder.extension(ext);
    }
    let config = builder.build().map_err(anyhow::Error::msg)?;
    Ok(config)
}
