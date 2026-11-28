//! Register the desktop application to open when the current user logs in.

use anyhow::{Context as _, Result};

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(windows)]
pub fn enabled() -> Result<bool> {
    Ok(crate::command("reg.exe")
        .args(["query", RUN_KEY, "/v", "CodeAny"])
        .output()?
        .status
        .success())
}

#[cfg(windows)]
pub fn set_enabled(enabled: bool) -> Result<()> {
    let mut command = crate::command("reg.exe");
    if enabled {
        let exe = std::env::current_exe()?;
        let value = format!("\"{}\"", exe.display());
        command.args([
            "add", RUN_KEY, "/v", "CodeAny", "/t", "REG_SZ", "/d", &value, "/f",
        ]);
    } else {
        command.args(["delete", RUN_KEY, "/v", "CodeAny", "/f"]);
    }
    let output = command
        .output()
        .context("update the Windows startup entry")?;
    anyhow::ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn entry_path() -> std::path::PathBuf {
    crate::paths::user_home().join("Library/LaunchAgents/ai.codeany.login.plist")
}

#[cfg(target_os = "linux")]
fn entry_path() -> std::path::PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| crate::paths::user_home().join(".config"))
        .join("autostart/ai.codeany.desktop")
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn enabled() -> Result<bool> {
    Ok(entry_path().try_exists()?)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn set_enabled(enabled: bool) -> Result<()> {
    let path = entry_path();
    if !enabled {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        };
    }

    let entry = entry_contents()?;
    std::fs::create_dir_all(path.parent().context("find the startup directory")?)?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temp, entry)?;
    std::fs::rename(temp, path)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn entry_contents() -> Result<String> {
    let exe = std::env::current_exe()?;
    let bundle = exe
        .ancestors()
        .nth(3)
        .filter(|path| path.extension().is_some_and(|ext| ext == "app"))
        .context("find the CodeAny application bundle")?;
    let bundle = bundle
        .to_str()
        .context("read the application bundle path")?
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>ai.codeany.login</string>
<key>ProgramArguments</key><array><string>/usr/bin/open</string><string>{bundle}</string></array>
<key>RunAtLoad</key><true/>
</dict></plist>
"#
    ))
}

#[cfg(target_os = "linux")]
fn entry_contents() -> Result<String> {
    let exe = match std::env::var_os("APPIMAGE") {
        Some(path) => std::path::PathBuf::from(path),
        None => std::env::current_exe()?,
    };
    let exe = exe.to_str().context("read the CodeAny executable path")?;
    anyhow::ensure!(!exe.contains(['\n', '\r']), "invalid executable path");
    // Exec quoting is decoded after the desktop entry's string escapes.
    let mut escaped = String::new();
    for ch in exe.chars() {
        match ch {
            '\\' => escaped.push_str(r"\\\\"),
            '"' | '`' | '$' => {
                escaped.push_str(r"\\");
                escaped.push(ch);
            }
            '%' => escaped.push_str("%%"),
            _ => escaped.push(ch),
        }
    }
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName=CodeAny\nExec=\"{escaped}\"\n"
    ))
}
