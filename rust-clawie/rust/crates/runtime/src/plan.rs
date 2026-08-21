//! Persistent multi-step planner and agentic execution mode.
//!
//! The planner is the source of truth for what Clawie is working on. Agentic
//! mode tells the model to keep the plan current and execute steps without
//! waiting for a new user message between tools.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::default_config_home;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStepStatus {
    Pending,
    InProgress,
    Done,
    Blocked,
}

impl PlanStepStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Blocked => "blocked",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "pending" | "todo" => Some(Self::Pending),
            "in_progress" | "doing" | "active" => Some(Self::InProgress),
            "done" | "completed" | "complete" => Some(Self::Done),
            "blocked" | "block" => Some(Self::Blocked),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Draft,
    Active,
    Completed,
}

impl PlanStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Completed => "completed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub status: PlanStepStatus,
    #[serde(default)]
    pub notes: String,
}

impl Default for PlanStepStatus {
    fn default() -> Self {
        Self::Pending
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub goal: String,
    #[serde(default)]
    pub steps: Vec<PlanStep>,
    #[serde(default = "default_plan_status")]
    pub status: PlanStatus,
    #[serde(default)]
    pub updated_at: String,
}

fn default_plan_status() -> PlanStatus {
    PlanStatus::Active
}

impl Plan {
    #[must_use]
    pub fn new(goal: impl Into<String>, steps: Vec<String>) -> Self {
        let steps = steps
            .into_iter()
            .enumerate()
            .map(|(index, title)| PlanStep {
                id: format!("s{}", index + 1),
                title,
                status: PlanStepStatus::Pending,
                notes: String::new(),
            })
            .collect();
        Self {
            goal: goal.into(),
            steps,
            status: PlanStatus::Active,
            updated_at: now_stamp(),
        }
    }

    #[must_use]
    pub fn is_complete(&self) -> bool {
        !self.steps.is_empty()
            && self
                .steps
                .iter()
                .all(|step| step.status == PlanStepStatus::Done)
    }

    #[must_use]
    pub fn next_pending(&self) -> Option<&PlanStep> {
        self.steps.iter().find(|step| {
            matches!(
                step.status,
                PlanStepStatus::Pending | PlanStepStatus::InProgress
            )
        })
    }
}

fn now_stamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

#[must_use]
pub fn plan_store_path() -> PathBuf {
    if let Ok(path) = env::var("CLAWIE_PLAN_STORE") {
        return PathBuf::from(path);
    }
    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".clawie")
        .join("plan.json")
}

pub fn load_plan() -> std::io::Result<Option<Plan>> {
    let path = plan_store_path();
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)?;
    let plan = serde_json::from_str(&raw).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid plan file: {error}"),
        )
    })?;
    Ok(Some(plan))
}

pub fn save_plan(mut plan: Plan) -> std::io::Result<Plan> {
    if plan.is_complete() {
        plan.status = PlanStatus::Completed;
    } else if plan.status == PlanStatus::Completed {
        plan.status = PlanStatus::Active;
    }
    plan.updated_at = now_stamp();
    let path = plan_store_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        path,
        serde_json::to_string_pretty(&plan).map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
        })?,
    )?;
    Ok(plan)
}

pub fn clear_plan() -> std::io::Result<()> {
    let path = plan_store_path();
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub fn set_step_status(
    id: &str,
    status: &str,
    notes: Option<&str>,
) -> std::io::Result<Plan> {
    let parsed = PlanStepStatus::parse(status).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unknown step status '{status}'"),
        )
    })?;
    let mut plan = load_plan()?.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no active plan")
    })?;
    let step = plan
        .steps
        .iter_mut()
        .find(|step| step.id == id || step.title == id)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("plan step '{id}' not found"),
            )
        })?;
    step.status = parsed;
    if let Some(notes) = notes {
        step.notes = notes.to_string();
    }
    save_plan(plan)
}

#[must_use]
pub fn format_plan_report(plan: Option<&Plan>) -> String {
    let Some(plan) = plan else {
        return "Plan
  Status           none
  Hint             /plan <goal> to start, or ask Clawie to use the Plan tool"
            .to_string();
    };
    let mut lines = vec![
        "Plan".to_string(),
        format!("  Status           {}", plan.status.as_str()),
        format!("  Goal             {}", plan.goal),
    ];
    if plan.steps.is_empty() {
        lines.push("  Steps            none yet".to_string());
    } else {
        for step in &plan.steps {
            let marker = match step.status {
                PlanStepStatus::Done => "[x]",
                PlanStepStatus::InProgress => "[>]",
                PlanStepStatus::Blocked => "[!]",
                PlanStepStatus::Pending => "[ ]",
            };
            let mut line = format!("  {marker} {} {}", step.id, step.title);
            if !step.notes.is_empty() {
                line.push_str(" — ");
                line.push_str(&step.notes);
            }
            lines.push(line);
        }
    }
    if let Some(next) = plan.next_pending() {
        lines.push(format!("  Next             {} {}", next.id, next.title));
    }
    lines.join("\n")
}

#[must_use]
pub fn plan_prompt_section() -> Option<String> {
    let plan = load_plan().ok().flatten()?;
    if plan.status == PlanStatus::Completed {
        return None;
    }
    let mut lines = vec![
        "# Active plan".to_string(),
        format!("Goal: {}", plan.goal),
        "Keep this plan current with the Plan tool. Work the next incomplete step. Do not skip to unrelated work.".to_string(),
    ];
    for step in &plan.steps {
        lines.push(format!(
            "- ({}) {} {}",
            step.status.as_str(),
            step.id,
            step.title
        ));
    }
    if let Some(next) = plan.next_pending() {
        lines.push(format!("Next step: {} {}", next.id, next.title));
    }
    Some(lines.join("\n"))
}

fn agentic_path() -> PathBuf {
    default_config_home().join("agentic.env")
}

fn parse_boolish(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
}

#[must_use]
pub fn agentic_enabled() -> bool {
    if let Ok(value) = env::var("CLAWIE_AGENTIC") {
        return parse_boolish(&value).unwrap_or(true);
    }
    if let Ok(contents) = fs::read_to_string(agentic_path()) {
        for line in contents.lines() {
            let line = line.trim();
            if let Some(value) = line.strip_prefix("CLAWIE_AGENTIC=") {
                return parse_boolish(value).unwrap_or(true);
            }
            if let Some(value) = parse_boolish(line) {
                return value;
            }
        }
    }
    true
}

pub fn persist_agentic(enabled: bool) -> std::io::Result<PathBuf> {
    let path = agentic_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = if enabled {
        "CLAWIE_AGENTIC=1\n"
    } else {
        "CLAWIE_AGENTIC=0\n"
    };
    fs::write(&path, body)?;
    Ok(path)
}

#[must_use]
pub fn agentic_prompt_section() -> Option<String> {
    if !agentic_enabled() {
        return None;
    }
    Some(
        "# Agentic mode
You may modify the local computer to complete the user's goal. Prefer this loop:
1. Write or update a Plan with the Plan tool before large work.
2. Execute the next step with tools (files, bash, computer).
3. Mark the step done or blocked, then continue to the next step without waiting for a new user prompt.
4. Use the computer tool for OS-level actions: open apps, files, URLs, reveal in Finder, and desktop notifications.
5. Stay inside the user's request. High-blast-radius actions (delete, install, send, publish) still need a clear user intent.
6. After the last step, summarize what changed and how to verify it."
            .to_string(),
    )
}

#[must_use]
pub fn format_agentic_report(enabled: bool, permission_mode: &str) -> String {
    let state = if enabled { "enabled" } else { "disabled" };
    format!(
        "Agentic mode
  Status           {state}
  Permission mode  {permission_mode}
  Planner          {}
  Computer         use the computer tool for apps, files, URLs, and notifications
  Default          on
  Usage            /agentic [status|on|off]
  Related          /plan [show|clear|<goal>]  /desktop [status|open <target>|notify <text>]",
        if load_plan().ok().flatten().is_some() {
            "active plan on disk"
        } else {
            "no plan yet"
        }
    )
}

#[cfg(test)]
mod tests {
    use super::{
        agentic_enabled, clear_plan, format_plan_report, load_plan, persist_agentic, save_plan,
        set_step_status, Plan, PlanStatus, PlanStepStatus,
    };
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn temp_plan_path() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("clawie-plan-{nanos}.json"))
    }

    #[test]
    fn persists_and_advances_steps() {
        let _guard = env_lock();
        let path = temp_plan_path();
        std::env::set_var("CLAWIE_PLAN_STORE", &path);
        let plan = Plan::new(
            "ship agentic planner",
            vec![
                "write plan module".to_string(),
                "add computer tool".to_string(),
            ],
        );
        save_plan(plan).expect("save");
        let loaded = load_plan().expect("load").expect("present");
        assert_eq!(loaded.goal, "ship agentic planner");
        assert_eq!(loaded.steps.len(), 2);
        assert_eq!(loaded.next_pending().map(|step| step.id.as_str()), Some("s1"));

        let updated = set_step_status("s1", "done", Some("module added")).expect("set");
        assert_eq!(updated.steps[0].status, PlanStepStatus::Done);
        assert_eq!(updated.next_pending().map(|step| step.id.as_str()), Some("s2"));
        let report = format_plan_report(Some(&updated));
        assert!(report.contains("[x] s1"));
        assert!(report.contains("[ ] s2"));

        set_step_status("s2", "done", None).expect("finish");
        let finished = load_plan().expect("load").expect("present");
        assert_eq!(finished.status, PlanStatus::Completed);
        clear_plan().expect("clear");
        assert!(load_plan().expect("load after clear").is_none());
        std::env::remove_var("CLAWIE_PLAN_STORE");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn agentic_mode_defaults_on_and_persists_off() {
        let _guard = env_lock();
        let root = std::env::temp_dir().join(format!(
            "clawie-agentic-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("temp config");
        let original_config_home = std::env::var_os("CLAW_CONFIG_HOME");
        let original_agentic = std::env::var_os("CLAWIE_AGENTIC");
        std::env::set_var("CLAW_CONFIG_HOME", &root);
        std::env::remove_var("CLAWIE_AGENTIC");

        assert!(agentic_enabled());
        persist_agentic(false).expect("persist off");
        assert!(!agentic_enabled());
        persist_agentic(true).expect("persist on");
        assert!(agentic_enabled());
        std::env::set_var("CLAWIE_AGENTIC", "0");
        assert!(!agentic_enabled());

        match original_config_home {
            Some(value) => std::env::set_var("CLAW_CONFIG_HOME", value),
            None => std::env::remove_var("CLAW_CONFIG_HOME"),
        }
        match original_agentic {
            Some(value) => std::env::set_var("CLAWIE_AGENTIC", value),
            None => std::env::remove_var("CLAWIE_AGENTIC"),
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
