use std::io;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitStatusOutput {
    pub branch: Option<String>,
    pub clean: bool,
    pub porcelain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitDiffInput {
    pub cached: Option<bool>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitDiffOutput {
    pub cached: bool,
    pub diff: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitCommitInput {
    pub message: Option<String>,
    pub all: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitCommandOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitUndoInput {
    #[serde(rename = "keepChanges")]
    pub keep_changes: Option<bool>,
}

pub fn git_status() -> io::Result<GitStatusOutput> {
    let porcelain = git_stdout(&["status", "--short"])?;
    let branch = git_stdout(&["branch", "--show-current"])
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    Ok(GitStatusOutput {
        branch,
        clean: porcelain.trim().is_empty(),
        porcelain,
    })
}

pub fn git_diff(input: GitDiffInput) -> io::Result<GitDiffOutput> {
    let mut args = vec!["diff"];
    let cached = input.cached.unwrap_or(false);
    if cached {
        args.push("--cached");
    }
    if let Some(path) = input.path.as_deref() {
        args.push("--");
        args.push(path);
    }
    Ok(GitDiffOutput {
        cached,
        diff: git_stdout(&args)?,
    })
}

pub fn git_commit(input: GitCommitInput) -> io::Result<GitCommandOutput> {
    if input.all.unwrap_or(true) {
        let add = run_git(&["add", "-A"])?;
        if !add.success {
            return Ok(add);
        }
    }
    let message = input.message.unwrap_or_else(default_commit_message);
    run_git(&["commit", "-m", &message])
}

pub fn git_undo_last_commit(input: GitUndoInput) -> io::Result<GitCommandOutput> {
    if input.keep_changes.unwrap_or(true) {
        run_git(&["reset", "--soft", "HEAD~1"])
    } else {
        run_git(&["reset", "--hard", "HEAD~1"])
    }
}

fn default_commit_message() -> String {
    match git_status() {
        Ok(status) if !status.porcelain.trim().is_empty() => {
            let changed = status.porcelain.lines().count();
            format!("clawie: update {changed} file(s)")
        }
        _ => "clawie: update workspace".to_string(),
    }
}

fn git_stdout(args: &[&str]) -> io::Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(current_dir()?)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn run_git(args: &[&str]) -> io::Result<GitCommandOutput> {
    let output = Command::new("git")
        .args(args)
        .current_dir(current_dir()?)
        .output()?;
    Ok(GitCommandOutput {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn current_dir() -> io::Result<impl AsRef<Path>> {
    std::env::current_dir()
}

#[cfg(test)]
mod tests {
    use super::{default_commit_message, GitDiffInput, GitUndoInput};

    #[test]
    fn default_commit_message_is_non_empty() {
        assert!(!default_commit_message().trim().is_empty());
    }

    #[test]
    fn input_structs_serialize_with_expected_names() {
        let undo = GitUndoInput {
            keep_changes: Some(true),
        };
        let json = serde_json::to_string(&undo).expect("undo input should serialize");
        assert!(json.contains("keepChanges"));

        let diff = GitDiffInput {
            cached: Some(true),
            path: Some("src/lib.rs".to_string()),
        };
        let json = serde_json::to_string(&diff).expect("diff input should serialize");
        assert!(json.contains("cached"));
    }
}
