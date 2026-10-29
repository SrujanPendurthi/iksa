use anyhow::Context;
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};
pub const PIN: &str = "156.0.8078.47-1.1";
const SHA256: &str = "5439d292078e260928d202145d0a20f19f227424888e84beb46973c16d40a186";
fn installed(home: &Path) -> PathBuf {
    home.join("chromium/Chromium.app/Contents/MacOS/Chromium")
}
pub fn chromium_path(home: &Path) -> Option<PathBuf> {
    std::env::var_os("CHROMIUM").map(PathBuf::from).or_else(|| {
        let p = installed(home);
        p.is_file().then_some(p)
    })
}
fn checked(c: &mut Command) -> anyhow::Result<Output> {
    let o = c.output().with_context(|| format!("running {c:?}"))?;
    anyhow::ensure!(
        o.status.success(),
        "{c:?}: {}: {}",
        o.status,
        String::from_utf8_lossy(&o.stderr)
    );
    Ok(o)
}
fn verify_checksum(file: &Path, expected: &str) -> anyhow::Result<()> {
    let output = checked(Command::new("shasum").args(["-a", "256"]).arg(file))?;
    if String::from_utf8(output.stdout)?.split_whitespace().next() != Some(expected) {
        std::fs::remove_file(file)?;
        anyhow::bail!("Chromium SHA-256 checksum mismatch; download deleted");
    }
    Ok(())
}
pub fn install(home: &Path) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(
        cfg!(all(target_os = "macos", target_arch = "aarch64")),
        "Chromium setup supports macOS arm64 only"
    );
    let binary = installed(home);
    let root = home.join("chromium");
    let version = root.join("version");
    // Written only after a checksum-verified install completes.
    if binary.is_file() && std::fs::read_to_string(&version).ok().as_deref() == Some(PIN) {
        return Ok(binary);
    }
    std::fs::create_dir_all(&root)?;
    let dmg = root.join("download.dmg");
    let url = format!(
        "https://github.com/ungoogled-software/ungoogled-chromium-macos/releases/download/{PIN}/ungoogled-chromium_{PIN}_arm64-macos.dmg"
    );
    checked(
        Command::new("curl")
            .arg("-fL")
            .arg(&url)
            .arg("-o")
            .arg(&dmg),
    )?;
    verify_checksum(&dmg, SHA256)?;
    let mount = root.join("mount");
    std::fs::create_dir_all(&mount)?;
    // An interrupted earlier setup can leave the image mounted here.
    let _ = Command::new("hdiutil")
        .args(["detach", "-force"])
        .arg(&mount)
        .output();
    checked(
        Command::new("hdiutil")
            .args(["attach", "-nobrowse"])
            .arg(&dmg)
            .arg("-mountpoint")
            .arg(&mount),
    )?;
    let copy = (|| {
        let app = root.join("Chromium.app");
        if app.exists() {
            std::fs::remove_dir_all(&app)?;
        }
        checked(
            Command::new("cp")
                .arg("-R")
                .arg(mount.join("Chromium.app"))
                .arg(&root),
        )?;
        Ok::<_, anyhow::Error>(())
    })();
    let detach = checked(Command::new("hdiutil").arg("detach").arg(&mount));
    copy?;
    detach?;
    checked(
        Command::new("xattr")
            .args(["-dr", "com.apple.quarantine"])
            .arg(root.join("Chromium.app")),
    )?;
    anyhow::ensure!(binary.is_file(), "installed Chromium binary missing");
    std::fs::write(version, PIN)?;
    std::fs::remove_file(dmg)?;
    std::fs::remove_dir(mount)?;
    Ok(binary)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checksum_mismatch_aborts_install() {
        let h = crate::testutil::temp_dir("checksum");
        let p = h.0.join("download.dmg");
        std::fs::write(&p, b"local invalid download").unwrap();
        assert!(
            verify_checksum(
                &p,
                "5439d292078e260928d202145d0a20f19f227424888e84beb46973c16d40a186"
            )
            .is_err()
        );
        assert!(!p.exists());
    }
}
