//! Start the background service automatically at login, natively on each OS:
//! a LaunchAgent on macOS, a systemd user unit on Linux (XDG autostart as a
//! fallback), and the per-user Run key on Windows.

#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::config::Config;
use crate::{Context as _, Error, Result};

const LOG_FILE: &str = "daemon.log";

/// The command line that starts the service, plus any profile override.
struct Launch {
    exe: PathBuf,
    home_override: Option<PathBuf>,
}

impl Launch {
    fn current() -> Result<Launch> {
        Ok(Launch {
            exe: std::env::current_exe().context("can't locate the beemr executable")?,
            home_override: std::env::var_os("BEEMR_HOME").map(PathBuf::from),
        })
    }
}

/// Whether beemr runs as a snap. Snaps can't register systemd units, so the
/// background service uses snap's own autostart instead: the `daemon` app
/// declares `autostart: beemr-daemon.desktop`, and snapd launches it at
/// desktop login once that file exists in the snap's user data.
fn in_snap() -> bool {
    std::env::var_os("SNAP").is_some()
}

fn snap_autostart_file() -> Result<PathBuf> {
    let data = std::env::var_os("SNAP_USER_DATA").context("SNAP_USER_DATA is not set")?;
    Ok(PathBuf::from(data).join(".config/autostart/beemr-daemon.desktop"))
}

/// Install autostart and start the service now.
pub fn install(config: &Config) -> Result<String> {
    let launch = Launch::current()?;
    if in_snap() {
        let path = snap_autostart_file()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).context(dir.display())?;
        }
        std::fs::write(
            &path,
            "[Desktop Entry]\nType=Application\nName=beemr background service\nExec=beemr.daemon\nNoDisplay=true\n",
        )
        .context(path.display())?;
        if !crate::daemon::is_running(config) {
            spawn_detached(config, &launch)?;
        }
        return Ok("starts automatically when you log in (snap autostart)".into());
    }
    platform::install(config, &launch)
}

/// Stop the service and remove autostart.
pub fn uninstall(config: &Config) -> Result<()> {
    if in_snap() {
        let path = snap_autostart_file()?;
        if path.exists() {
            std::fs::remove_file(&path).context(path.display())?;
        }
        let _ = run("pkill", &["-f", "beemr daemon run"]);
        return Ok(());
    }
    platform::uninstall(config)
}

pub fn log_path(config: &Config) -> PathBuf {
    config.path(LOG_FILE)
}

fn run(program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context(format!("couldn't run {program}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::new(format!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// Start the service as a detached background process (fallback when the
/// platform's service manager isn't available).
fn spawn_detached(config: &Config, launch: &Launch) -> Result<()> {
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path(config))?;
    let mut command = Command::new(&launch.exe);
    command
        .args(["daemon", "run"])
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    if let Some(home) = &launch.home_override {
        command.env("BEEMR_HOME", home);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }
    command
        .spawn()
        .context("couldn't start the background service")?;
    Ok(())
}

#[cfg(unix)]
fn home_dir() -> Result<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .context("can't find your home folder")
}

#[cfg(unix)]
fn write_file(path: &Path, contents: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).context(dir.display())?;
    }
    std::fs::write(path, contents).context(path.display())
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    const LABEL: &str = "dev.beemr.daemon";

    fn plist_path() -> Result<PathBuf> {
        Ok(home_dir()?.join(format!("Library/LaunchAgents/{LABEL}.plist")))
    }

    fn gui_domain() -> Result<String> {
        let output = Command::new("id").arg("-u").output()?;
        Ok(format!(
            "gui/{}",
            String::from_utf8_lossy(&output.stdout).trim()
        ))
    }

    fn escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    pub fn install(config: &Config, launch: &Launch) -> Result<String> {
        let log = escape(&log_path(config).to_string_lossy());
        let env = launch.home_override.as_ref().map_or(String::new(), |home| {
            format!(
                "<key>EnvironmentVariables</key><dict><key>BEEMR_HOME</key><string>{}</string></dict>",
                escape(&home.to_string_lossy())
            )
        });
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array><string>{}</string><string>daemon</string><string>run</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
  {env}
</dict>
</plist>
"#,
            escape(&launch.exe.to_string_lossy())
        );
        let path = plist_path()?;
        write_file(&path, &plist)?;
        let domain = gui_domain()?;
        // Replace any previous registration.
        let _ = run("launchctl", &["bootout", &format!("{domain}/{LABEL}")]);
        run(
            "launchctl",
            &["bootstrap", &domain, &path.to_string_lossy()],
        )?;
        Ok("starts automatically when you log in (macOS LaunchAgent)".into())
    }

    pub fn uninstall(_config: &Config) -> Result<()> {
        let domain = gui_domain()?;
        let _ = run("launchctl", &["bootout", &format!("{domain}/{LABEL}")]);
        let path = plist_path()?;
        if path.exists() {
            std::fs::remove_file(&path).context(path.display())?;
        }
        Ok(())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod platform {
    use super::*;

    const UNIT: &str = "beemr.service";

    fn config_home() -> Result<PathBuf> {
        match std::env::var_os("XDG_CONFIG_HOME") {
            Some(dir) => Ok(dir.into()),
            None => Ok(home_dir()?.join(".config")),
        }
    }

    fn quote(s: &str) -> String {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }

    pub fn install(config: &Config, launch: &Launch) -> Result<String> {
        let exe = launch.exe.to_string_lossy();
        let env = launch.home_override.as_ref().map_or(String::new(), |home| {
            format!(
                "Environment={}\n",
                quote(&format!("BEEMR_HOME={}", home.to_string_lossy()))
            )
        });
        let unit = format!(
            "[Unit]\nDescription=beemr background service\nAfter=network-online.target\n\n\
             [Service]\nExecStart={} daemon run\n{env}Restart=always\nRestartSec=5\n\n\
             [Install]\nWantedBy=default.target\n",
            quote(&exe)
        );
        let unit_path = config_home()?.join("systemd/user").join(UNIT);
        let systemd = write_file(&unit_path, &unit)
            .and_then(|()| run("systemctl", &["--user", "daemon-reload"]))
            .and_then(|()| run("systemctl", &["--user", "enable", "--now", UNIT]));
        if systemd.is_ok() {
            return Ok("starts automatically when you log in (systemd user service)".into());
        }
        let _ = std::fs::remove_file(&unit_path);

        // No systemd user session (containers, some distros): use XDG autostart.
        let env_prefix = launch.home_override.as_ref().map_or(String::new(), |home| {
            format!("env BEEMR_HOME={} ", quote(&home.to_string_lossy()))
        });
        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName=beemr\nExec={env_prefix}{} daemon run\n\
             NoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
            quote(&exe)
        );
        write_file(&config_home()?.join("autostart/beemr.desktop"), &desktop)?;
        spawn_detached(config, launch)?;
        Ok("starts automatically when you log in (XDG autostart)".into())
    }

    pub fn uninstall(_config: &Config) -> Result<()> {
        let _ = run("systemctl", &["--user", "disable", "--now", UNIT]);
        let home = config_home()?;
        for path in [
            home.join("systemd/user").join(UNIT),
            home.join("autostart/beemr.desktop"),
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

    const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

    pub fn install(config: &Config, launch: &Launch) -> Result<String> {
        let command = format!("\"{}\" daemon run", launch.exe.to_string_lossy());
        run(
            "reg",
            &[
                "add", RUN_KEY, "/v", "beemr", "/t", "REG_SZ", "/d", &command, "/f",
            ],
        )?;
        if !crate::daemon::is_running(config) {
            spawn_detached(config, launch)?;
        }
        Ok("starts automatically when you sign in to Windows".into())
    }

    pub fn uninstall(_config: &Config) -> Result<()> {
        let _ = run("reg", &["delete", RUN_KEY, "/v", "beemr", "/f"]);
        let _ = run(
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                "Get-CimInstance Win32_Process -Filter \"Name='beemr.exe'\" | \
                 Where-Object { $_.CommandLine -like '*daemon run*' } | \
                 ForEach-Object { Stop-Process -Id $_.ProcessId -Force }",
            ],
        );
        Ok(())
    }
}
