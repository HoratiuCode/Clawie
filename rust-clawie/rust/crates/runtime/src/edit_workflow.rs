use std::fs;
use std::io::{self, Write};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::{edit_file, write_file, EditFileOutput, WriteFileOutput};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EditWorkflowFormat {
    WholeFile,
    SearchReplace,
    UnifiedDiff,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditWorkflowInput {
    pub format: EditWorkflowFormat,
    pub path: Option<String>,
    pub content: Option<String>,
    #[serde(rename = "oldString")]
    pub old_string: Option<String>,
    #[serde(rename = "newString")]
    pub new_string: Option<String>,
    #[serde(rename = "replaceAll")]
    pub replace_all: Option<bool>,
    pub diff: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum EditWorkflowOutput {
    WholeFile {
        result: WriteFileOutput,
    },
    SearchReplace {
        result: EditFileOutput,
    },
    UnifiedDiff {
        applied: bool,
        stdout: String,
        stderr: String,
    },
}

pub fn apply_edit_workflow(input: EditWorkflowInput) -> io::Result<EditWorkflowOutput> {
    match input.format {
        EditWorkflowFormat::WholeFile => {
            let path = required(input.path, "path")?;
            let content = required(input.content, "content")?;
            write_file(&path, &content).map(|result| EditWorkflowOutput::WholeFile { result })
        }
        EditWorkflowFormat::SearchReplace => {
            let path = required(input.path, "path")?;
            let old_string = required(input.old_string, "oldString")?;
            let new_string = required(input.new_string, "newString")?;
            edit_file(
                &path,
                &old_string,
                &new_string,
                input.replace_all.unwrap_or(false),
            )
            .map(|result| EditWorkflowOutput::SearchReplace { result })
        }
        EditWorkflowFormat::UnifiedDiff => {
            let diff = required(input.diff, "diff")?;
            apply_unified_diff(&diff)
        }
    }
}

fn required(value: Option<String>, field: &str) -> io::Result<String> {
    value.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("missing required field `{field}`"),
        )
    })
}

fn apply_unified_diff(diff: &str) -> io::Result<EditWorkflowOutput> {
    let cwd = std::env::current_dir()?;
    let check = run_git_apply(&cwd, diff, true)?;
    if !check.status.success() {
        return Ok(EditWorkflowOutput::UnifiedDiff {
            applied: false,
            stdout: String::from_utf8_lossy(&check.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&check.stderr).into_owned(),
        });
    }

    let output = run_git_apply(&cwd, diff, false)?;
    Ok(EditWorkflowOutput::UnifiedDiff {
        applied: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn run_git_apply(
    cwd: &std::path::Path,
    diff: &str,
    check: bool,
) -> io::Result<std::process::Output> {
    let mut command = Command::new("git");
    command.arg("apply");
    if check {
        command.arg("--check");
    }
    let mut child = command
        .arg("--whitespace=nowarn")
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .as_mut()
        .expect("git apply stdin should be piped")
        .write_all(diff.as_bytes())?;
    child.wait_with_output()
}

#[allow(dead_code)]
fn _read_for_future_editor_modes(path: &str) -> io::Result<String> {
    fs::read_to_string(path)
}

#[cfg(test)]
mod tests {
    use super::{apply_edit_workflow, EditWorkflowFormat, EditWorkflowInput, EditWorkflowOutput};

    #[test]
    fn rejects_missing_whole_file_content() {
        let error = apply_edit_workflow(EditWorkflowInput {
            format: EditWorkflowFormat::WholeFile,
            path: Some("missing.txt".to_string()),
            content: None,
            old_string: None,
            new_string: None,
            replace_all: None,
            diff: None,
        })
        .expect_err("content is required");
        assert!(error.to_string().contains("content"));
    }

    #[test]
    fn output_is_tagged() {
        let output = EditWorkflowOutput::UnifiedDiff {
            applied: false,
            stdout: String::new(),
            stderr: String::new(),
        };
        let json = serde_json::to_string(&output).expect("output should serialize");
        assert!(json.contains("UnifiedDiff"));
    }
}
