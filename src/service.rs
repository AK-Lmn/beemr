//! Cleanup of the background service that beemr versions before 0.3 installed
//! to receive messages: a LaunchAgent on macOS, a systemd user unit or XDG
//! autostart entry on Linux, the per-user Run key on Windows, or snap autostart.
//! Messaging and the service were removed; this removes what they left behind.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::config::Config;
use crate::{Context as _, Result};

/// Stop the old background service and remove it from startup, if present.
pub fn remove_legacy(config: &Config) -> Result<()> {
    if std::env::var_os("SNAP").is_some() {
        if let Some(data) = std::env::var_os("SNAP_USER_DATA") {
            let path = PathBuf::from(data).join(".config/autostart/beemr-daemon.desktop");
            if path.exists() {
                std::fs::remove_file(&path).context(path.display())?;
            }
        }
        let _ = run("pkill", &["-f", "beemr daemon run"]);
        return Ok(());
    }
    platform::uninstall(config)
}

fn run(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context(format!("couldn't run {program}"))?;
    if !status.success() {
        bail!("{program} {} failed", args.join(" "));
    }
    Ok(())
}

#[cfg(unix)]
fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("can't find your home folder")
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    const LABEL: &str = "dev.beemr.daemon";

    pub fn uninstall(_config: &Config) -> Result<()> {
        let uid = Command::new("id").arg("-u").output()?;
        let domain = format!("gui/{}", String::from_utf8_lossy(&uid.stdout).trim());
        let _ = run("launchctl", &["bootout", &format!("{domain}/{LABEL}")]);
        let plist = home_dir()?.join(format!("Library/LaunchAgents/{LABEL}.plist"));
        if plist.exists() {
            std::fs::remove_file(&plist).context(plist.display())?;
        }
        Ok(())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod platform {
    use super::*;

    pub fn uninstall(_config: &Config) -> Result<()> {
        let _ = run(
            "systemctl",
            &["--user", "disable", "--now", "beemr.service"],
        );
        let config_home = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(dir) => PathBuf::from(dir),
            None => home_dir()?.join(".config"),
        };
        for path in [
            config_home.join("systemd/user/beemr.service"),
            config_home.join("autostart/beemr.desktop"),
        ] {
            if path.exists() {
                std::fs::remove_file(&path).context(path.display())?;
            }
        }
        let _ = run("systemctl", &["--user", "daemon-reload"]);
        let _ = run("pkill", &["-f", "beemr daemon run"]);
        Ok(())
    }
}

#[cfg(windows)]
mod platform {
    use super::*;

    pub fn uninstall(_config: &Config) -> Result<()> {
        let _ = run(
            "reg",
            &[
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "beemr",
                "/f",
            ],
        );
        Ok(())
    }
}
