use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
struct TeamState {
    agents: Vec<TeamAgent>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TeamAgent {
    name: String,
    task: String,
    branch: String,
    worktree: PathBuf,
    files: Vec<String>,
    status: AgentStatus,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    log: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AgentStatus {
    Active,
    Ready,
    Merged,
}

impl std::fmt::Display for AgentStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Active => "active",
            Self::Ready => "ready to merge",
            Self::Merged => "merged",
        })
    }
}

pub fn handle(args: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
    let root = repository_root()?;
    let input = args.unwrap_or_default().trim();
    let (action, remainder) = input.split_once(char::is_whitespace).unwrap_or((input, ""));
    match action {
        "" | "status" => status(&root),
        "help" | "-h" | "--help" => Ok(usage()),
        "init" => init(&root),
        "spawn" => spawn(&root, remainder),
        "assign" => assign(&root, remainder),
        "run" => run(&root, remainder),
        "context" => context(&root, remainder),
        "ready" => ready(&root, remainder),
        "merge" => merge(&root, remainder),
        other => Ok(format!("Unknown /team action '{other}'.\n\n{}", usage())),
    }
}

fn usage() -> String {
    "Team workflow\n  /team init\n      Create the local coordination state.\n  /team spawn <name> <task>\n      Create an isolated Git worktree and branch for an agent.\n  /team assign <name> <file[,file...]>\n      Reserve files for an agent; active reservations cannot overlap.\n  /team run <name>\n      Start Clawie in that isolated worktree after ownership is assigned.\n  /team context <task>\n      Show a small task-relevant file list (no full-repo indexing).\n  /team ready <name>\n      Run quality gates and add the agent to the merge queue.\n  /team merge <name>\n      Merge one ready branch after a final diff check.\n  /team status\n      Show agents, worktrees, ownership, and merge state.\n\nUse /team assign before editing. Parallelize isolated work; merge one agent at a time."
        .to_string()
}

fn init(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let path = state_path(root);
    if !path.exists() {
        save_state(root, &TeamState::default())?;
    }
    Ok(format!(
        "Team coordination ready\n  State            {}\n  Next             /team spawn <name> <task>",
        path.display()
    ))
}

fn spawn(root: &Path, remainder: &str) -> Result<String, Box<dyn std::error::Error>> {
    let (name, task) = remainder
        .trim()
        .split_once(char::is_whitespace)
        .ok_or("Usage: /team spawn <name> <task>")?;
    let name = validate_name(name)?;
    if task.trim().is_empty() {
        return Err("Usage: /team spawn <name> <task>".into());
    }
    let mut state = load_state(root)?;
    if state.agents.iter().any(|agent| agent.name == name) {
        return Err(format!("Agent '{name}' already exists. Use /team status.").into());
    }
    let branch = format!("clawie/team/{name}");
    let worktree = worktree_root(root).join(&name);
    if worktree.exists() {
        return Err(format!("Worktree path already exists: {}", worktree.display()).into());
    }
    if let Some(parent) = worktree.parent() {
        fs::create_dir_all(parent)?;
    }
    git(
        root,
        [
            "worktree",
            "add",
            "-b",
            &branch,
            &worktree.to_string_lossy(),
            "HEAD",
        ],
    )?;
    state.agents.push(TeamAgent {
        name: name.to_string(),
        task: task.trim().to_string(),
        branch: branch.clone(),
        worktree: worktree.clone(),
        files: Vec::new(),
        status: AgentStatus::Active,
        pid: None,
        log: None,
    });
    save_state(root, &state)?;
    Ok(format!(
        "Agent created\n  Name             {name}\n  Branch           {branch}\n  Worktree         {}\n  Task             {}\n  Next             /team assign {name} <file[,file...]>",
        worktree.display(),
        task.trim()
    ))
}

fn run(root: &Path, name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut state = load_state(root)?;
    let agent = find_agent_mut(&mut state, name.trim())?;
    if agent.status != AgentStatus::Active {
        return Err(format!(
            "Agent '{}' is {} and cannot be started.",
            agent.name, agent.status
        )
        .into());
    }
    if agent.files.is_empty() {
        return Err(format!(
            "Agent '{}' has no file ownership. Run /team assign {} <file[,file...]> first.",
            agent.name, agent.name
        )
        .into());
    }
    let log_dir = root.join(".claw").join("team-logs");
    fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join(format!("{}.log", agent.name));
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let stderr = log.try_clone()?;
    let prompt = format!(
        "You are agent '{}'. Task: {}\nYou own these files only: {}\nWork only in the assigned files, run relevant checks, and summarize the result when done.",
        agent.name,
        agent.task,
        agent.files.join(", ")
    );
    let child = Command::new(env::current_exe()?)
        .args(["prompt", &prompt])
        .current_dir(&agent.worktree)
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr))
        .spawn()?;
    let pid = child.id();
    agent.pid = Some(pid);
    agent.log = Some(log_path.clone());
    let agent_name = agent.name.clone();
    save_state(root, &state)?;
    Ok(format!(
        "Agent started\n  Agent            {agent_name}\n  PID              {pid}\n  Log              {}\n  Next             /team ready {agent_name} after the agent finishes",
        log_path.display()
    ))
}

fn assign(root: &Path, remainder: &str) -> Result<String, Box<dyn std::error::Error>> {
    let (name, paths) = remainder
        .trim()
        .split_once(char::is_whitespace)
        .ok_or("Usage: /team assign <name> <file[,file...]>")?;
    let requested: Vec<String> = paths
        .split(',')
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect();
    if requested.is_empty() {
        return Err("Usage: /team assign <name> <file[,file...]>".into());
    }
    let mut state = load_state(root)?;
    let conflicts: Vec<String> = state
        .agents
        .iter()
        .filter(|agent| agent.name != name && agent.status != AgentStatus::Merged)
        .flat_map(|agent| {
            requested.iter().filter_map(move |path| {
                agent
                    .files
                    .contains(path)
                    .then(|| format!("{path} ({})", agent.name))
            })
        })
        .collect();
    if !conflicts.is_empty() {
        return Err(format!("File reservation conflict: {}", conflicts.join(", ")).into());
    }
    let agent = state
        .agents
        .iter_mut()
        .find(|agent| agent.name == name)
        .ok_or_else(|| format!("Unknown agent '{name}'. Use /team status."))?;
    if agent.status != AgentStatus::Active {
        return Err(format!(
            "Agent '{name}' is {} and cannot receive new files.",
            agent.status
        )
        .into());
    }
    for path in &requested {
        if !agent.files.contains(path) {
            agent.files.push(path.clone());
        }
    }
    save_state(root, &state)?;
    Ok(format!(
        "File ownership assigned\n  Agent            {name}\n  Files            {}",
        requested.join(", ")
    ))
}

fn context(root: &Path, remainder: &str) -> Result<String, Box<dyn std::error::Error>> {
    let task = remainder.trim();
    if task.is_empty() {
        return Err("Usage: /team context <task>".into());
    }
    let tokens: Vec<String> = task
        .to_ascii_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| token.len() >= 3)
        .map(str::to_string)
        .collect();
    let output = git(root, ["ls-files"])?;
    let mut files: Vec<&str> = output
        .lines()
        .filter(|path| {
            let lower = path.to_ascii_lowercase();
            tokens.iter().any(|token| lower.contains(token))
        })
        .take(20)
        .collect();
    for instruction in ["CLAUDE.md", "AGENTS.md", ".claw/settings.json"] {
        if root.join(instruction).exists() && !files.contains(&instruction) {
            files.insert(0, instruction);
        }
    }
    Ok(format!(
        "Scoped context\n  Task             {task}\n  Repository files {}\n  Matching files\n{}\n  Note             This is a lazy path match; use /map or RAG only when deeper context is needed.",
        output.lines().count(),
        if files.is_empty() { "  <no filename matches>".to_string() } else { files.iter().map(|path| format!("  {path}")).collect::<Vec<_>>().join("\n") }
    ))
}

fn ready(root: &Path, name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut state = load_state(root)?;
    let agent = find_agent_mut(&mut state, name.trim())?;
    if agent.status != AgentStatus::Active {
        return Err(format!("Agent '{}' is {}.", agent.name, agent.status).into());
    }
    quality_gate(&agent.worktree)?;
    agent.status = AgentStatus::Ready;
    let agent_name = agent.name.clone();
    let runs_cargo_check = agent.worktree.join("Cargo.toml").exists();
    save_state(root, &state)?;
    Ok(format!(
        "Merge queue updated\n  Agent            {agent_name}\n  Checks           git diff --check{}\n  Next             /team merge {agent_name}",
        if runs_cargo_check { " · cargo check" } else { "" }
    ))
}

fn merge(root: &Path, name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut state = load_state(root)?;
    let agent = find_agent_mut(&mut state, name.trim())?;
    if agent.status != AgentStatus::Ready {
        return Err(format!(
            "Agent '{}' is not ready to merge. Run /team ready {} first.",
            agent.name, agent.name
        )
        .into());
    }
    quality_gate(&agent.worktree)?;
    let branch = agent.branch.clone();
    let agent_name = agent.name.clone();
    git(
        root,
        [
            "merge",
            "--no-ff",
            &branch,
            "-m",
            &format!("Merge Clawie agent {agent_name}"),
        ],
    )?;
    agent.status = AgentStatus::Merged;
    save_state(root, &state)?;
    Ok(format!(
        "Agent merged\n  Agent            {agent_name}\n  Branch           {branch}\n  Result           clean merge completed\n  Next             /team status"
    ))
}

fn quality_gate(worktree: &Path) -> Result<(), Box<dyn std::error::Error>> {
    git(worktree, ["diff", "--check"])?;
    if worktree.join("Cargo.toml").exists() {
        let status = Command::new("cargo")
            .arg("check")
            .current_dir(worktree)
            .status()?;
        if !status.success() {
            return Err("cargo check failed in the agent worktree".into());
        }
    }
    Ok(())
}

fn status(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let state = load_state(root)?;
    if state.agents.is_empty() {
        return Ok(format!("Team\n  Agents           none\n  Next             /team init, then /team spawn <name> <task>"));
    }
    let rows = state
        .agents
        .iter()
        .map(|agent| {
            format!(
            "  {}  {}\n    Task            {}\n    Files           {}\n    Worktree        {}\n    Process         {}\n    Log             {}",
                agent.name,
                agent.status,
                agent.task,
                if agent.files.is_empty() {
                    "<unassigned>".to_string()
                } else {
                    agent.files.join(", ")
                },
            agent.worktree.display(),
            agent.pid.map_or_else(|| "not started".to_string(), |pid| format!("PID {pid}")),
            agent.log.as_ref().map_or_else(|| "<none>".to_string(), |path| path.display().to_string())
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!(
        "Team\n{rows}\n\nMerge queue is serialized: run /team merge <name> one agent at a time."
    ))
}

fn find_agent_mut<'a>(
    state: &'a mut TeamState,
    name: &str,
) -> Result<&'a mut TeamAgent, Box<dyn std::error::Error>> {
    state
        .agents
        .iter_mut()
        .find(|agent| agent.name == name)
        .ok_or_else(|| format!("Unknown agent '{name}'. Use /team status.").into())
}

fn repository_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    Ok(PathBuf::from(git(&cwd, ["rev-parse", "--show-toplevel"])?))
}

fn state_path(root: &Path) -> PathBuf {
    root.join(".claw").join("team.json")
}

fn worktree_root(root: &Path) -> PathBuf {
    let repo_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("clawie");
    root.parent()
        .unwrap_or(root)
        .join(format!(".{repo_name}-clawie-worktrees"))
}

fn load_state(root: &Path) -> Result<TeamState, Box<dyn std::error::Error>> {
    let path = state_path(root);
    if !path.exists() {
        return Ok(TeamState::default());
    }
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

fn save_state(root: &Path, state: &TeamState) -> Result<(), Box<dyn std::error::Error>> {
    let path = state_path(root);
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("team state has no parent"))?;
    fs::create_dir_all(parent)?;
    fs::write(path, serde_json::to_string_pretty(state)?)?;
    Ok(())
}

fn validate_name(name: &str) -> Result<&str, Box<dyn std::error::Error>> {
    if name.is_empty()
        || !name.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
    {
        return Err(
            "Agent names may contain only letters, numbers, hyphens, and underscores.".into(),
        );
    }
    Ok(name)
}

fn git<const N: usize>(cwd: &Path, args: [&str; N]) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("git").args(args).current_dir(cwd).output()?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    Err(format!(
        "git command failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )
    .into())
}
