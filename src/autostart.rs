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

#[cfg(target_os = "linux")]
pub fn enabled() -> Result<bool> {
    Ok(entry_path().try_exists()?)
}

#[cfg(target_os = "linux")]
pub fn set_enabled(enabled: bool) -> Result<()> {
    set_file_entry(enabled)
}

#[cfg(target_os = "macos")]
pub fn enabled() -> Result<bool> {
    match macos::service() {
        Some(service) => Ok(macos::status(&service) == macos::ENABLED),
        None => Ok(entry_path().try_exists()?),
    }
}

#[cfg(target_os = "macos")]
pub fn set_enabled(enabled: bool) -> Result<()> {
    let Some(service) = macos::service() else {
        return set_file_entry(enabled);
    };
    let legacy = macos::has_legacy_entry()?;
    macos::set_enabled(&service, enabled)?;
    if legacy {
        set_file_entry(false)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn migrate_legacy_entry() -> Result<()> {
    let Some(service) = macos::service() else {
        return Ok(());
    };
    if !macos::has_legacy_entry()? {
        return Ok(());
    }
    // Respect a user who disabled the old entry in System Settings.
    if !macos::legacy_requires_approval() {
        macos::set_enabled(&service, true)?;
        if macos::status(&service) != macos::ENABLED {
            return Ok(());
        }
    }
    set_file_entry(false)
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use objc2::{
        msg_send,
        rc::Retained,
        runtime::{AnyClass, AnyObject},
    };
    use objc2_foundation::{NSString, NSURL};

    #[link(name = "ServiceManagement", kind = "framework")]
    unsafe extern "C" {}

    // SMAppServiceStatus is an NSInteger in the SDK.
    const NOT_REGISTERED: isize = 0;
    pub(super) const ENABLED: isize = 1;
    const REQUIRES_APPROVAL: isize = 2;

    fn service_class() -> Option<&'static AnyClass> {
        // The framework also exists on macOS 12, but this class starts at 13.
        AnyClass::get(c"SMAppService")
    }

    pub(super) fn service() -> Option<Retained<AnyObject>> {
        // SAFETY: mainAppService returns an SMAppService on supported systems.
        unsafe { msg_send![service_class()?, mainAppService] }
    }

    pub(super) fn status(service: &AnyObject) -> isize {
        // SAFETY: this object came from mainAppService.
        unsafe { msg_send![service, status] }
    }

    pub(super) fn set_enabled(service: &AnyObject, enabled: bool) -> Result<()> {
        let status = status(service);
        if enabled {
            if status == ENABLED {
                return Ok(());
            }
            // SAFETY: SMAppService uses the standard BOOL + NSError convention.
            let result: Result<(), Retained<objc2_foundation::NSError>> =
                unsafe { msg_send![service, registerAndReturnError: _] };
            result.map_err(|err| anyhow::anyhow!("{}", err.localizedDescription()))?;
        } else if matches!(status, ENABLED | REQUIRES_APPROVAL) {
            // SAFETY: same method signature as registration.
            let result: Result<(), Retained<objc2_foundation::NSError>> =
                unsafe { msg_send![service, unregisterAndReturnError: _] };
            result.map_err(|err| anyhow::anyhow!("{}", err.localizedDescription()))?;
        } else {
            anyhow::ensure!(
                status == NOT_REGISTERED,
                "CodeAny login item could not be found"
            );
        }
        Ok(())
    }

    pub(super) fn has_legacy_entry() -> Result<bool> {
        let entry = match std::fs::read_to_string(entry_path()) {
            Ok(entry) => entry,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(err) => return Err(err.into()),
        };
        // The old filename was shared with CodeAny Dev. Migrate only the entry
        // pointing to this bundle so neither installation changes the other's setting.
        Ok(entry_contents().is_ok_and(|expected| entry == expected))
    }

    pub(super) fn legacy_requires_approval() -> bool {
        let path = entry_path();
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        // SAFETY: called only when SMAppService is available, with a file URL.
        let status: isize = unsafe {
            msg_send![service_class().expect("SMAppService is available"), statusForLegacyURL: &*url]
        };
        status == REQUIRES_APPROVAL
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn set_file_entry(enabled: bool) -> Result<()> {
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
