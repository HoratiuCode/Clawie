//! User-level computer automation: open apps/files/URLs, notify, reveal.
//!
//! These actions talk to the host OS directly. They require danger-full-access
//! and never go through a shell, so the target cannot inject extra commands.

use std::path::Path;
use std::process::Command;

use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
pub struct ComputerInput {
    pub action: String,
    pub target: Option<String>,
    pub text: Option<String>,
}

pub fn run_computer(input: ComputerInput) -> Result<String, String> {
    let action = input.action.trim().to_ascii_lowercase();
    let payload = match action.as_str() {
        "status" | "info" => computer_status(),
        "open" => {
            let target = required_target(input.target.as_deref())?;
            open_target(target)?
        }
        "reveal" => {
            let target = required_target(input.target.as_deref())?;
            reveal_target(target)?
        }
        "notify" => {
            let text = input
                .text
                .as_deref()
                .or(input.target.as_deref())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| String::from("notify requires text"))?;
            notify(text)?
        }
        "apps" => list_apps()?,
        other => {
            return Err(format!(
                "unsupported computer action '{other}'. Use status, open, reveal, notify, or apps."
            ))
        }
    };
    serde_json::to_string_pretty(&payload).map_err(|error| error.to_string())
}

fn required_target(target: Option<&str>) -> Result<&str, String> {
    target
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| String::from("action requires a target"))
}

fn computer_status() -> Value {
    json!({
        "ok": true,
        "platform": std::env::consts::OS,
        "actions": ["status", "open", "reveal", "notify", "apps"],
        "hint": "open a file, app, or URL; reveal a path in the file manager; notify the desktop; list running apps",
        "permission": "danger-full-access"
    })
}

fn open_target(target: &str) -> Result<Value, String> {
    validate_open_target(target)?;
    let mut command = open_command(target, false);
    let output = command
        .output()
        .map_err(|error| format!("failed to open {target}: {error}"))?;
    if output.status.success() {
        Ok(json!({
            "ok": true,
            "action": "open",
            "target": target
        }))
    } else {
        Err(format!(
            "open failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn reveal_target(target: &str) -> Result<Value, String> {
    validate_open_target(target)?;
    if !Path::new(target).exists() {
        return Err(format!("path does not exist: {target}"));
    }
    let mut command = open_command(target, true);
    let output = command
        .output()
        .map_err(|error| format!("failed to reveal {target}: {error}"))?;
    if output.status.success() {
        Ok(json!({
            "ok": true,
            "action": "reveal",
            "target": target
        }))
    } else {
        Err(format!(
            "reveal failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn notify(text: &str) -> Result<Value, String> {
    if text.chars().count() > 280 {
        return Err(String::from("notification text is too long"));
    }
    if text.contains('\0') {
        return Err(String::from("notification text is invalid"));
    }
    let output = if cfg!(target_os = "macos") {
        let script = format!(
            "display notification \"{}\" with title \"Clawie\"",
            escape_applescript(text)
        );
        Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .map_err(|error| error.to_string())?
    } else if cfg!(target_os = "linux") {
        Command::new("notify-send")
            .arg("Clawie")
            .arg(text)
            .output()
            .map_err(|error| error.to_string())?
    } else {
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null; Write-Output '{}'",
                    text.replace('\'', "''")
                ),
            ])
            .output()
            .map_err(|error| error.to_string())?
    };
    if output.status.success() {
        Ok(json!({
            "ok": true,
            "action": "notify",
            "text": text
        }))
    } else {
        Err(format!(
            "notify failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn list_apps() -> Result<Value, String> {
    let output = if cfg!(target_os = "macos") {
        Command::new("osascript")
            .arg("-e")
            .arg("tell application \"System Events\" to get name of every process whose background only is false")
            .output()
            .map_err(|error| error.to_string())?
    } else {
        Command::new("ps")
            .args(["-axo", "comm="])
            .output()
            .map_err(|error| error.to_string())?
    };
    if !output.status.success() {
        return Err(format!(
            "apps listing failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut apps: Vec<String> = stdout
        .split([',', '\n'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    apps.sort();
    apps.dedup();
    apps.truncate(80);
    Ok(json!({
        "ok": true,
        "action": "apps",
        "apps": apps
    }))
}

fn open_command(target: &str, reveal: bool) -> Command {
    if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        if reveal {
            command.arg("-R");
        }
        command.arg(target);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", target]);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(target);
        command
    }
}

pub fn validate_open_target(target: &str) -> Result<(), String> {
    let target = target.trim();
    if target.is_empty() {
        return Err(String::from("target cannot be empty"));
    }
    if target.contains('\0') || target.chars().any(|ch| ch.is_control()) {
        return Err(String::from("target contains invalid characters"));
    }
    if let Some(scheme) = url_scheme(target) {
        if !matches!(scheme.as_str(), "http" | "https" | "file" | "mailto") {
            return Err(format!("blocked URL scheme '{scheme}'"));
        }
    }
    Ok(())
}

fn url_scheme(target: &str) -> Option<String> {
    let (scheme, rest) = target.split_once(':')?;
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '+' || ch == '.' || ch == '-')
    {
        return None;
    }
    let scheme = scheme.to_ascii_lowercase();
    if rest.starts_with("//") || scheme == "mailto" {
        return Some(scheme);
    }
    // javascript:alert(1) — not a Windows drive letter like C:\path
    if scheme.len() > 1 && !rest.starts_with('\\') && !rest.starts_with('/') {
        return Some(scheme);
    }
    None
}

fn escape_applescript(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::validate_open_target;

    #[test]
    fn accepts_files_and_safe_urls() {
        assert!(validate_open_target("/tmp/notes.md").is_ok());
        assert!(validate_open_target("https://example.com").is_ok());
        assert!(validate_open_target("mailto:user@example.com").is_ok());
    }

    #[test]
    fn rejects_empty_and_dangerous_schemes() {
        assert!(validate_open_target("").is_err());
        assert!(validate_open_target("javascript:alert(1)").is_err());
        assert!(validate_open_target("data:text/html,hi").is_err());
    }
}
