use std::env;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

use crate::config::default_config_home;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeanMode {
    Off,
    Lite,
    Full,
    Ultra,
}

impl LeanMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Lite => "lite",
            Self::Full => "full",
            Self::Ultra => "ultra",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "stop" | "normal" | "normal-mode" => Some(Self::Off),
            "lite" | "light" => Some(Self::Lite),
            "full" | "" => Some(Self::Full),
            "ultra" | "extreme" => Some(Self::Ultra),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct LeanConfigFile {
    #[serde(rename = "defaultMode")]
    default_mode: Option<String>,
}

#[must_use]
pub fn active_lean_mode() -> LeanMode {
    persisted_lean_mode()
        .or_else(env_lean_mode)
        .or_else(configured_lean_mode)
        .unwrap_or(LeanMode::Full)
}

pub fn persist_lean_mode(mode: LeanMode) -> std::io::Result<()> {
    let path = clawie_mode_path();
    if mode == LeanMode::Off {
        match fs::remove_file(&path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, mode.as_str())
}

#[must_use]
pub fn format_lean_mode_report(mode: LeanMode) -> String {
    let status = if mode == LeanMode::Off {
        "inactive"
    } else {
        "active"
    };
    format!(
        "Clawie lean mode\n  Status           {status}\n  Mode             {}\n  Commands         /lean [lite|full|ultra|off], /lean-review, /lean-audit, /lean-debt, /lean-gain, /lean-help",
        mode.as_str()
    )
}

#[must_use]
pub fn lean_system_section(mode: LeanMode) -> Option<String> {
    (mode != LeanMode::Off).then(|| {
        format!(
            "# Clawie lean mode: {}\n{}",
            mode.as_str(),
            lean_instruction_body(mode)
        )
    })
}

#[must_use]
pub fn lean_command_prompt(command: &str) -> Option<&'static str> {
    match command {
        "review" => Some(LEAN_REVIEW_PROMPT),
        "audit" => Some(LEAN_AUDIT_PROMPT),
        "debt" => Some(LEAN_DEBT_PROMPT),
        _ => None,
    }
}

#[must_use]
pub fn lean_gain_report() -> &'static str {
    "Clawie lean gain                 benchmark median · not this repo\n\n  Lines of code   baseline  ####################  100%\n                  lean      ####................   6-20%  down 80-94%\n  Cost            baseline  ####################  100%\n                  lean      ##########..........  23-53%  down 47-77%\n  Speed           lean      3-6x faster\n\n  This repo        /lean-debt   shortcut ledger\n                   /lean-audit  cuttable complexity"
}

#[must_use]
pub fn lean_help_report() -> &'static str {
    "Clawie lean help\n  /lean [lite|full|ultra|off]   switch persistent mode\n  /lean-review                  review current diff for over-engineering only\n  /lean-audit                   audit repo for over-engineering only\n  /lean-debt                    list lean comments and missing triggers\n  /lean-gain                    show benchmark impact card\n\nDefault mode is full. Override with CLAWIE_LEAN_MODE."
}

fn lean_instruction_body(mode: LeanMode) -> String {
    match mode {
        LeanMode::Off => String::new(),
        LeanMode::Lite => format!(
            "Lazy senior developer mode, lite. Build what was asked, but name the lazier alternative in one short line. {LEAN_LADDER} No unrequested abstractions, avoidable dependencies, speculative config, or boilerplate. {LEAN_SAFETY}"
        ),
        LeanMode::Full => format!(
            "Lazy senior developer mode, full. Enforce the ladder; shortest working diff wins once the code is understood. Deletion over addition, boring over clever, fewest files possible. {LEAN_LADDER} Ship the lazy version and question complex requirements in the same response instead of stalling. {LEAN_SAFETY}"
        ),
        LeanMode::Ultra => format!(
            "Lazy senior developer mode, ultra. YAGNI extremist: deletion before addition, challenge speculative requirements, and prefer the one-line/native/stdlib answer whenever it truly works. {LEAN_LADDER} If the request is overbuilt, do the smallest useful version and say what was skipped. {LEAN_SAFETY}"
        ),
    }
}

fn persisted_lean_mode() -> Option<LeanMode> {
    fs::read_to_string(clawie_mode_path())
        .ok()
        .and_then(|value| LeanMode::parse(&value))
}

fn env_lean_mode() -> Option<LeanMode> {
    env::var("CLAWIE_LEAN_MODE")
        .or_else(|_| env::var("LEAN_DEFAULT_MODE"))
        .ok()
        .and_then(|value| LeanMode::parse(&value))
}

fn configured_lean_mode() -> Option<LeanMode> {
    fs::read_to_string(lean_config_path())
        .ok()
        .and_then(|value| serde_json::from_str::<LeanConfigFile>(&value).ok())
        .and_then(|config| config.default_mode)
        .and_then(|value| LeanMode::parse(&value))
}

fn clawie_mode_path() -> PathBuf {
    default_config_home().join("lean-mode")
}

fn lean_config_path() -> PathBuf {
    default_config_home().join("lean.json")
}

const LEAN_LADDER: &str = "Before code, stop at the first rung that holds: 1. Does this need to exist? 2. Is it already in this codebase? 3. Does stdlib do it? 4. Does a native platform feature cover it? 5. Does an installed dependency solve it? 6. Can it be one line? 7. Only then write the minimum that works.";

const LEAN_SAFETY: &str = "The ladder runs after understanding the task and tracing the real flow. Bug fixes target the shared root cause, not one symptom path. Never simplify away trust-boundary validation, data-loss error handling, security, accessibility, hardware calibration, or anything explicitly requested. Non-trivial logic leaves one small runnable check. Mark intentional shortcuts with a `clawie:` comment naming the ceiling and upgrade trigger.";

const LEAN_REVIEW_PROMPT: &str = "Review the current code changes for over-engineering only, not correctness. One line per finding: L<line>: <tag> <what to cut>. <replacement>. Tags: delete, stdlib, native, yagni, shrink. End with `net: -<N> lines possible.` If nothing can be cut, say `Lean already. Ship.`";

const LEAN_AUDIT_PROMPT: &str = "Audit the entire repository for over-engineering only, not correctness. Scan the tree and rank findings biggest cut first. One line per finding: <tag> <what to cut>. <replacement>. [path]. Tags: delete, stdlib, native, yagni, shrink. End with `net: -<N> lines, -<M> deps possible.` If nothing can be cut, say `Lean already. Ship.`";

const LEAN_DEBT_PROMPT: &str = "Harvest every `clawie:` comment in this repository into a debt ledger. Grep comment markers while skipping .git, node_modules, target, dist, and build output. Output one row per marker grouped by file: <file>:<line> — <what was simplified>. ceiling: <limit>. upgrade: <trigger>. Tag markers with no upgrade trigger as no-trigger. End with `<N> markers, <M> with no trigger.` Report only; change nothing.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modes() {
        assert_eq!(LeanMode::parse("lite"), Some(LeanMode::Lite));
        assert_eq!(LeanMode::parse(""), Some(LeanMode::Full));
        assert_eq!(LeanMode::parse("normal mode"), None);
        assert_eq!(LeanMode::parse("normal-mode"), Some(LeanMode::Off));
    }

    #[test]
    fn renders_full_prompt() {
        let section = lean_system_section(LeanMode::Full).expect("active section");
        assert!(section.contains("Clawie lean mode: full"));
        assert!(section.contains("Does stdlib do it?"));
        assert!(section.contains("Never simplify away"));
    }
}
