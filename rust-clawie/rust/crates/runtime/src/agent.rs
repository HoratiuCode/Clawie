use std::collections::{BTreeMap, HashMap};
use std::fmt::{Display, Formatter};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::{
    ResolvedPermissionMode, RuntimeAgentDefinitionConfig, RuntimeConfig,
    RuntimePermissionRuleConfig,
};
use crate::permissions::PermissionMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AgentMode {
    Primary,
    Subagent,
    All,
}

impl AgentMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Subagent => "subagent",
            Self::All => "all",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentModel {
    pub model: String,
    pub provider: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDefinition {
    pub name: String,
    pub description: Option<String>,
    pub mode: AgentMode,
    pub prompt: Option<String>,
    pub model: Option<AgentModel>,
    pub permission_mode: Option<PermissionMode>,
    pub permission_rules: RuntimePermissionRuleConfig,
    pub hidden: bool,
    pub native: bool,
    pub steps: Option<u32>,
}

impl AgentDefinition {
    #[must_use]
    pub fn merged_permission_rules(
        &self,
        base: &RuntimePermissionRuleConfig,
    ) -> RuntimePermissionRuleConfig {
        merge_permission_rule_configs(base, &self.permission_rules)
    }

    #[must_use]
    pub fn summary_line(&self) -> String {
        let mut extras = vec![format!("mode={}", self.mode.as_str())];
        if let Some(permission_mode) = self.permission_mode {
            extras.push(format!("permissions={}", permission_mode.as_str()));
        }
        if let Some(model) = &self.model {
            match &model.provider {
                Some(provider) => extras.push(format!("model={provider}/{}", model.model)),
                None => extras.push(format!("model={}", model.model)),
            }
        }
        if self.hidden {
            extras.push("hidden=true".to_string());
        }
        format!(
            "{}: {} [{}]",
            self.name,
            self.description
                .as_deref()
                .unwrap_or("No description provided."),
            extras.join(", ")
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRegistry {
    default_agent: String,
    agents: BTreeMap<String, AgentDefinition>,
}

impl AgentRegistry {
    #[must_use]
    pub fn from_runtime_config(config: &RuntimeConfig) -> Self {
        let mut agents = default_agents();
        let feature_agents = config.feature_config().agents();

        for (name, override_config) in feature_agents.agents() {
            if let Some(existing) = agents.get_mut(name) {
                apply_override(existing, override_config);
            } else {
                agents.insert(
                    name.clone(),
                    definition_from_config(name, override_config, config),
                );
            }
        }

        let default_agent = feature_agents
            .default_agent()
            .map_or_else(|| "build".to_string(), ToOwned::to_owned);
        let default_agent = if agents.contains_key(&default_agent) {
            default_agent
        } else {
            "build".to_string()
        };

        Self {
            default_agent,
            agents,
        }
    }

    #[must_use]
    pub fn default_agent(&self) -> &str {
        &self.default_agent
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&AgentDefinition> {
        self.agents.get(name)
    }

    #[must_use]
    pub fn list(&self) -> Vec<&AgentDefinition> {
        self.agents.values().collect()
    }

    #[must_use]
    pub fn visible_agents(&self) -> Vec<&AgentDefinition> {
        self.agents.values().filter(|agent| !agent.hidden).collect()
    }

    #[must_use]
    pub fn render_prompt_section(&self) -> String {
        let mut lines = vec![
            "# Agent roles".to_string(),
            format!(" - Default agent: {}", self.default_agent),
            " - Available agents:".to_string(),
        ];
        lines.extend(
            self.visible_agents()
                .into_iter()
                .map(|agent| format!("   - {}", agent.summary_line())),
        );
        lines.join("\n")
    }
}

fn default_agents() -> BTreeMap<String, AgentDefinition> {
    let mut agents = BTreeMap::new();
    agents.insert(
        "build".to_string(),
        AgentDefinition {
            name: "build".to_string(),
            description: Some(
                "Default agent that can execute tools and complete implementation work."
                    .to_string(),
            ),
            mode: AgentMode::Primary,
            prompt: None,
            model: None,
            permission_mode: None,
            permission_rules: RuntimePermissionRuleConfig::new(
                vec!["Question".to_string(), "PlanEnter".to_string()],
                Vec::new(),
                Vec::new(),
            ),
            hidden: false,
            native: true,
            steps: None,
        },
    );
    agents.insert(
        "plan".to_string(),
        AgentDefinition {
            name: "plan".to_string(),
            description: Some(
                "Planning agent that avoids edit tools and focuses on analysis.".to_string(),
            ),
            mode: AgentMode::Primary,
            prompt: Some(
                "Focus on planning, analysis, and decomposition. Do not make edits.".to_string(),
            ),
            model: None,
            permission_mode: Some(PermissionMode::ReadOnly),
            permission_rules: RuntimePermissionRuleConfig::new(
                vec!["Question".to_string(), "PlanExit".to_string()],
                vec![
                    "Edit".to_string(),
                    "Write".to_string(),
                    "ApplyPatch".to_string(),
                ],
                Vec::new(),
            ),
            hidden: false,
            native: true,
            steps: None,
        },
    );
    agents.insert(
        "general".to_string(),
        AgentDefinition {
            name: "general".to_string(),
            description: Some(
                "General-purpose subagent for parallel execution of bounded units of work."
                    .to_string(),
            ),
            mode: AgentMode::Subagent,
            prompt: None,
            model: None,
            permission_mode: None,
            permission_rules: RuntimePermissionRuleConfig::new(
                Vec::new(),
                vec!["TodoWrite".to_string()],
                Vec::new(),
            ),
            hidden: false,
            native: true,
            steps: None,
        },
    );
    agents.insert(
        "explore".to_string(),
        AgentDefinition {
            name: "explore".to_string(),
            description: Some(
                "Read-heavy subagent specialized for codebase exploration and evidence gathering."
                    .to_string(),
            ),
            mode: AgentMode::Subagent,
            prompt: Some(
                "Prioritize searching, reading, and summarizing relevant code with concrete evidence."
                    .to_string(),
            ),
            model: None,
            permission_mode: Some(PermissionMode::ReadOnly),
            permission_rules: RuntimePermissionRuleConfig::new(
                vec![
                    "Read".to_string(),
                    "Grep".to_string(),
                    "Glob".to_string(),
                    "List".to_string(),
                    "Bash".to_string(),
                    "WebFetch".to_string(),
                    "WebSearch".to_string(),
                ],
                vec![
                    "Edit".to_string(),
                    "Write".to_string(),
                    "ApplyPatch".to_string(),
                ],
                Vec::new(),
            ),
            hidden: false,
            native: true,
            steps: None,
        },
    );
    agents.insert(
        "scout".to_string(),
        AgentDefinition {
            name: "scout".to_string(),
            description: Some(
                "External-docs and dependency reconnaissance subagent for remote research."
                    .to_string(),
            ),
            mode: AgentMode::Subagent,
            prompt: Some(
                "Research external docs and dependencies, but do not modify the workspace."
                    .to_string(),
            ),
            model: None,
            permission_mode: Some(PermissionMode::ReadOnly),
            permission_rules: RuntimePermissionRuleConfig::new(
                vec![
                    "Read".to_string(),
                    "Grep".to_string(),
                    "Glob".to_string(),
                    "WebFetch".to_string(),
                    "WebSearch".to_string(),
                    "RepoClone".to_string(),
                    "RepoOverview".to_string(),
                ],
                vec![
                    "Edit".to_string(),
                    "Write".to_string(),
                    "ApplyPatch".to_string(),
                ],
                Vec::new(),
            ),
            hidden: false,
            native: true,
            steps: None,
        },
    );
    agents.insert(
        "compaction".to_string(),
        AgentDefinition {
            name: "compaction".to_string(),
            description: Some(
                "Hidden maintenance agent used to summarize and compact conversation state."
                    .to_string(),
            ),
            mode: AgentMode::Primary,
            prompt: Some(
                "Summarize the session faithfully and compress state without introducing new work."
                    .to_string(),
            ),
            model: None,
            permission_mode: Some(PermissionMode::ReadOnly),
            permission_rules: RuntimePermissionRuleConfig::new(
                Vec::new(),
                vec!["*".to_string()],
                Vec::new(),
            ),
            hidden: true,
            native: true,
            steps: None,
        },
    );
    agents
}

fn apply_override(agent: &mut AgentDefinition, config: &RuntimeAgentDefinitionConfig) {
    if let Some(description) = config.description() {
        agent.description = Some(description.to_string());
    }
    if let Some(mode) = config.mode() {
        agent.mode = mode;
    }
    if let Some(prompt) = config.prompt() {
        agent.prompt = Some(prompt.to_string());
    }
    if let Some(model) = config.model() {
        agent.model = Some(AgentModel {
            model: model.to_string(),
            provider: config.provider().map(ToOwned::to_owned),
        });
    } else if let Some(provider) = config.provider() {
        agent.model = Some(AgentModel {
            model: agent
                .model
                .as_ref()
                .map(|item| item.model.clone())
                .unwrap_or_default(),
            provider: Some(provider.to_string()),
        });
    }
    if let Some(permission_mode) = config.permission_mode() {
        agent.permission_mode = Some(permission_mode_from_resolved(permission_mode));
    }
    agent.permission_rules =
        merge_permission_rule_configs(&agent.permission_rules, config.permission_rules());
    if let Some(hidden) = config.hidden() {
        agent.hidden = hidden;
    }
    if let Some(native) = config.native() {
        agent.native = native;
    }
    if let Some(steps) = config.steps() {
        agent.steps = Some(steps);
    }
}

fn definition_from_config(
    name: &str,
    config: &RuntimeAgentDefinitionConfig,
    runtime_config: &RuntimeConfig,
) -> AgentDefinition {
    AgentDefinition {
        name: name.to_string(),
        description: config.description().map(ToOwned::to_owned),
        mode: config.mode().unwrap_or(AgentMode::Primary),
        prompt: config.prompt().map(ToOwned::to_owned),
        model: config.model().map(|model| AgentModel {
            model: model.to_string(),
            provider: config
                .provider()
                .map(ToOwned::to_owned)
                .or_else(|| runtime_config.provider().map(ToOwned::to_owned)),
        }),
        permission_mode: config.permission_mode().map(permission_mode_from_resolved),
        permission_rules: config.permission_rules().clone(),
        hidden: config.hidden().unwrap_or(false),
        native: config.native().unwrap_or(true),
        steps: config.steps(),
    }
}

fn merge_permission_rule_configs(
    base: &RuntimePermissionRuleConfig,
    overlay: &RuntimePermissionRuleConfig,
) -> RuntimePermissionRuleConfig {
    let mut allow = base.allow().to_vec();
    extend_unique(&mut allow, overlay.allow());
    let mut deny = base.deny().to_vec();
    extend_unique(&mut deny, overlay.deny());
    let mut ask = base.ask().to_vec();
    extend_unique(&mut ask, overlay.ask());
    RuntimePermissionRuleConfig::new(allow, deny, ask)
}

fn extend_unique(target: &mut Vec<String>, values: &[String]) {
    for value in values {
        if !target.contains(value) {
            target.push(value.clone());
        }
    }
}

#[must_use]
pub fn permission_mode_from_resolved(mode: ResolvedPermissionMode) -> PermissionMode {
    match mode {
        ResolvedPermissionMode::ReadOnly => PermissionMode::ReadOnly,
        ResolvedPermissionMode::WorkspaceWrite => PermissionMode::WorkspaceWrite,
        ResolvedPermissionMode::DangerFullAccess => PermissionMode::DangerFullAccess,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RuntimeAgentPath(String);

impl RuntimeAgentPath {
    #[must_use]
    pub fn root() -> Self {
        Self("root".to_string())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_root(&self) -> bool {
        self.0 == "root"
    }

    #[must_use]
    pub fn depth(&self) -> usize {
        if self.is_root() {
            0
        } else {
            self.0.split('/').count().saturating_sub(1)
        }
    }

    #[must_use]
    pub fn child(&self, segment: &str) -> Self {
        if self.is_root() {
            Self(format!("root/{segment}"))
        } else {
            Self(format!("{}/{}", self.0, segment))
        }
    }
}

impl Display for RuntimeAgentPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAgentState {
    Starting,
    Running,
    Completed,
    Failed,
    Stopped,
}

impl RuntimeAgentState {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Stopped)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeAgentMetadata {
    pub agent_id: String,
    pub path: RuntimeAgentPath,
    pub parent_path: Option<RuntimeAgentPath>,
    pub nickname: String,
    pub role: String,
    pub state: RuntimeAgentState,
    pub task_id: Option<String>,
    pub last_task_message: Option<String>,
    pub started_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeAgentRegistryError {
    AgentLimitReached { max_agents: usize },
    ParentNotFound { parent_path: RuntimeAgentPath },
    ParentNotRunning { parent_path: RuntimeAgentPath },
    AgentAlreadyExists { agent_id: String },
    PathAlreadyExists { path: RuntimeAgentPath },
    AgentNotFound { agent_id: String },
}

impl Display for RuntimeAgentRegistryError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AgentLimitReached { max_agents } => {
                write!(f, "agent limit reached: maximum {max_agents} live agents")
            }
            Self::ParentNotFound { parent_path } => {
                write!(f, "parent agent not found for path `{parent_path}`")
            }
            Self::ParentNotRunning { parent_path } => {
                write!(f, "parent agent is not running for path `{parent_path}`")
            }
            Self::AgentAlreadyExists { agent_id } => write!(f, "agent already exists: {agent_id}"),
            Self::PathAlreadyExists { path } => write!(f, "agent path already exists: {path}"),
            Self::AgentNotFound { agent_id } => write!(f, "agent not found: {agent_id}"),
        }
    }
}

impl std::error::Error for RuntimeAgentRegistryError {}

#[derive(Debug, Clone)]
pub struct RuntimeAgentRegistry {
    inner: Arc<Mutex<RuntimeAgentRegistryInner>>,
}

#[derive(Debug, Default)]
struct RuntimeAgentRegistryInner {
    max_agents: Option<usize>,
    total_spawned: usize,
    agent_tree: BTreeMap<RuntimeAgentPath, RuntimeAgentMetadata>,
    id_to_path: HashMap<String, RuntimeAgentPath>,
    nickname_counts: HashMap<String, usize>,
    nickname_index_by_parent: HashMap<RuntimeAgentPath, usize>,
}

impl Default for RuntimeAgentRegistry {
    fn default() -> Self {
        Self::new(None)
    }
}

impl RuntimeAgentRegistry {
    #[must_use]
    pub fn new(max_agents: Option<usize>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RuntimeAgentRegistryInner {
                max_agents,
                ..RuntimeAgentRegistryInner::default()
            })),
        }
    }

    pub fn register_root(
        &self,
        agent_id: impl Into<String>,
        role: impl Into<String>,
    ) -> Result<RuntimeAgentMetadata, RuntimeAgentRegistryError> {
        let mut inner = self.inner.lock().expect("agent registry lock poisoned");
        let agent_id = agent_id.into();
        if inner.id_to_path.contains_key(&agent_id) {
            return Err(RuntimeAgentRegistryError::AgentAlreadyExists { agent_id });
        }
        let path = RuntimeAgentPath::root();
        if inner.agent_tree.contains_key(&path) {
            return Err(RuntimeAgentRegistryError::PathAlreadyExists { path });
        }
        let now = now_secs();
        let metadata = RuntimeAgentMetadata {
            agent_id: agent_id.clone(),
            path: RuntimeAgentPath::root(),
            parent_path: None,
            nickname: "root".to_string(),
            role: role.into(),
            state: RuntimeAgentState::Running,
            task_id: None,
            last_task_message: None,
            started_at: now,
            updated_at: now,
        };
        inner.id_to_path.insert(agent_id, RuntimeAgentPath::root());
        inner.total_spawned += 1;
        inner
            .agent_tree
            .insert(RuntimeAgentPath::root(), metadata.clone());
        Ok(metadata)
    }

    pub fn spawn_subagent(
        &self,
        agent_id: impl Into<String>,
        parent_path: &RuntimeAgentPath,
        role: impl Into<String>,
        preferred_nickname: Option<&str>,
        task_id: Option<&str>,
        last_task_message: Option<&str>,
    ) -> Result<RuntimeAgentMetadata, RuntimeAgentRegistryError> {
        let mut inner = self.inner.lock().expect("agent registry lock poisoned");
        let agent_id = agent_id.into();
        if inner.id_to_path.contains_key(&agent_id) {
            return Err(RuntimeAgentRegistryError::AgentAlreadyExists { agent_id });
        }
        let parent = inner.agent_tree.get(parent_path).cloned().ok_or_else(|| {
            RuntimeAgentRegistryError::ParentNotFound {
                parent_path: parent_path.clone(),
            }
        })?;
        if parent.state.is_terminal() {
            return Err(RuntimeAgentRegistryError::ParentNotRunning {
                parent_path: parent_path.clone(),
            });
        }
        let live_agents = inner
            .agent_tree
            .values()
            .filter(|agent| !agent.state.is_terminal())
            .count();
        if let Some(max_agents) = inner.max_agents {
            if live_agents >= max_agents {
                return Err(RuntimeAgentRegistryError::AgentLimitReached { max_agents });
            }
        }
        let role = role.into();
        let nickname = reserve_nickname(&mut inner, preferred_nickname, &role);
        let index = inner
            .nickname_index_by_parent
            .entry(parent_path.clone())
            .and_modify(|value| *value += 1)
            .or_insert(1);
        let path = parent_path.child(&format!("agent-{index}"));
        if inner.agent_tree.contains_key(&path) {
            return Err(RuntimeAgentRegistryError::PathAlreadyExists { path });
        }
        let now = now_secs();
        let metadata = RuntimeAgentMetadata {
            agent_id: agent_id.clone(),
            path: path.clone(),
            parent_path: Some(parent_path.clone()),
            nickname,
            role,
            state: RuntimeAgentState::Starting,
            task_id: task_id.map(ToOwned::to_owned),
            last_task_message: last_task_message.map(ToOwned::to_owned),
            started_at: now,
            updated_at: now,
        };
        inner.id_to_path.insert(agent_id, path.clone());
        inner.total_spawned += 1;
        inner.agent_tree.insert(path, metadata.clone());
        Ok(metadata)
    }

    pub fn set_state(
        &self,
        agent_id: &str,
        state: RuntimeAgentState,
    ) -> Result<RuntimeAgentMetadata, RuntimeAgentRegistryError> {
        self.update_agent(agent_id, |metadata| metadata.state = state)
    }

    pub fn attach_task(
        &self,
        agent_id: &str,
        task_id: Option<&str>,
        last_task_message: Option<&str>,
    ) -> Result<RuntimeAgentMetadata, RuntimeAgentRegistryError> {
        self.update_agent(agent_id, |metadata| {
            metadata.task_id = task_id.map(ToOwned::to_owned);
            if let Some(message) = last_task_message {
                metadata.last_task_message = Some(message.to_string());
            }
        })
    }

    pub fn update_last_task_message(
        &self,
        agent_id: &str,
        last_task_message: impl Into<String>,
    ) -> Result<RuntimeAgentMetadata, RuntimeAgentRegistryError> {
        let message = last_task_message.into();
        self.update_agent(agent_id, |metadata| {
            metadata.last_task_message = Some(message.clone())
        })
    }

    pub fn get_by_id(&self, agent_id: &str) -> Option<RuntimeAgentMetadata> {
        let inner = self.inner.lock().expect("agent registry lock poisoned");
        inner
            .id_to_path
            .get(agent_id)
            .and_then(|path| inner.agent_tree.get(path))
            .cloned()
    }

    pub fn get_by_path(&self, path: &RuntimeAgentPath) -> Option<RuntimeAgentMetadata> {
        let inner = self.inner.lock().expect("agent registry lock poisoned");
        inner.agent_tree.get(path).cloned()
    }

    #[must_use]
    pub fn live_agents(&self) -> Vec<RuntimeAgentMetadata> {
        let inner = self.inner.lock().expect("agent registry lock poisoned");
        inner
            .agent_tree
            .values()
            .filter(|agent| !agent.path.is_root() && !agent.state.is_terminal())
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn all_agents(&self) -> Vec<RuntimeAgentMetadata> {
        let inner = self.inner.lock().expect("agent registry lock poisoned");
        inner.agent_tree.values().cloned().collect()
    }

    #[must_use]
    pub fn total_spawned(&self) -> usize {
        let inner = self.inner.lock().expect("agent registry lock poisoned");
        inner.total_spawned
    }

    fn update_agent<F>(
        &self,
        agent_id: &str,
        mut update: F,
    ) -> Result<RuntimeAgentMetadata, RuntimeAgentRegistryError>
    where
        F: FnMut(&mut RuntimeAgentMetadata),
    {
        let mut inner = self.inner.lock().expect("agent registry lock poisoned");
        let path = inner.id_to_path.get(agent_id).cloned().ok_or_else(|| {
            RuntimeAgentRegistryError::AgentNotFound {
                agent_id: agent_id.to_string(),
            }
        })?;
        let metadata = inner.agent_tree.get_mut(&path).ok_or_else(|| {
            RuntimeAgentRegistryError::AgentNotFound {
                agent_id: agent_id.to_string(),
            }
        })?;
        update(metadata);
        metadata.updated_at = now_secs();
        Ok(metadata.clone())
    }
}

fn reserve_nickname(
    inner: &mut RuntimeAgentRegistryInner,
    preferred_nickname: Option<&str>,
    role: &str,
) -> String {
    let preferred = preferred_nickname
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| role.to_lowercase().replace(' ', "-"));
    let count = inner.nickname_counts.entry(preferred.clone()).or_insert(0);
    *count += 1;
    if *count == 1 {
        preferred
    } else {
        format!("{preferred}-{}", *count)
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use crate::config::{ConfigLoader, RuntimePermissionRuleConfig};

    use super::{
        AgentMode, AgentRegistry, RuntimeAgentPath, RuntimeAgentRegistry,
        RuntimeAgentRegistryError, RuntimeAgentState,
    };

    #[test]
    fn registry_exposes_default_agents() {
        let root =
            std::env::temp_dir().join(format!("agent-registry-defaults-{}", std::process::id()));
        let config = ConfigLoader::default_for(&root)
            .load()
            .expect("config should load");
        let registry = AgentRegistry::from_runtime_config(&config);
        assert_eq!(registry.default_agent(), "build");
        assert_eq!(
            registry.get("explore").expect("explore agent").mode,
            AgentMode::Subagent
        );
        assert!(registry.get("compaction").expect("compaction agent").hidden);
    }

    #[test]
    fn merged_permission_rules_preserve_base_and_overlay() {
        let root =
            std::env::temp_dir().join(format!("agent-registry-merge-{}", std::process::id()));
        let config = ConfigLoader::default_for(&root)
            .load()
            .expect("config should load");
        let registry = AgentRegistry::from_runtime_config(&config);
        let build = registry.get("build").expect("build agent");
        let merged = build.merged_permission_rules(&RuntimePermissionRuleConfig::new(
            vec!["Read".to_string()],
            vec!["Bash(rm -rf)".to_string()],
            Vec::new(),
        ));
        assert!(merged.allow().contains(&"Read".to_string()));
        assert!(merged.allow().contains(&"Question".to_string()));
        assert!(merged.deny().contains(&"Bash(rm -rf)".to_string()));
    }

    #[test]
    fn runtime_registry_tracks_root_and_spawned_agents() {
        let registry = RuntimeAgentRegistry::new(Some(4));
        let root = registry
            .register_root("root-agent", "build")
            .expect("root should register");
        assert_eq!(root.path, RuntimeAgentPath::root());

        let subagent = registry
            .spawn_subagent(
                "agent-1",
                &RuntimeAgentPath::root(),
                "explore",
                Some("scout"),
                Some("task-1"),
                Some("Find the prompt builder"),
            )
            .expect("subagent should spawn");
        assert_eq!(subagent.path.as_str(), "root/agent-1");
        assert_eq!(subagent.parent_path, Some(RuntimeAgentPath::root()));
        assert_eq!(subagent.nickname, "scout");
        assert_eq!(subagent.state, RuntimeAgentState::Starting);

        let updated = registry
            .set_state("agent-1", RuntimeAgentState::Running)
            .expect("state should update");
        assert_eq!(updated.state, RuntimeAgentState::Running);
        assert_eq!(registry.live_agents().len(), 1);
        assert_eq!(registry.total_spawned(), 2);
    }

    #[test]
    fn runtime_registry_applies_spawn_limit_to_live_subagents() {
        let registry = RuntimeAgentRegistry::new(Some(2));
        registry
            .register_root("root-agent", "build")
            .expect("root should register");
        registry
            .spawn_subagent(
                "agent-1",
                &RuntimeAgentPath::root(),
                "general",
                None,
                None,
                None,
            )
            .expect("first subagent should spawn");

        let error = registry
            .spawn_subagent(
                "agent-2",
                &RuntimeAgentPath::root(),
                "general",
                None,
                None,
                None,
            )
            .expect_err("second live subagent should exceed limit");
        assert_eq!(
            error,
            RuntimeAgentRegistryError::AgentLimitReached { max_agents: 2 }
        );

        registry
            .set_state("agent-1", RuntimeAgentState::Completed)
            .expect("subagent should complete");
        registry
            .spawn_subagent(
                "agent-2",
                &RuntimeAgentPath::root(),
                "general",
                None,
                None,
                None,
            )
            .expect("completed subagent should free capacity");
    }

    #[test]
    fn runtime_registry_rejects_spawns_under_terminal_parent() {
        let registry = RuntimeAgentRegistry::new(None);
        registry
            .register_root("root-agent", "build")
            .expect("root should register");
        registry
            .spawn_subagent(
                "agent-1",
                &RuntimeAgentPath::root(),
                "general",
                None,
                None,
                None,
            )
            .expect("subagent should spawn");
        registry
            .set_state("agent-1", RuntimeAgentState::Failed)
            .expect("subagent should fail");

        let error = registry
            .spawn_subagent(
                "agent-2",
                &RuntimeAgentPath::root().child("agent-1"),
                "explore",
                None,
                None,
                None,
            )
            .expect_err("terminal parent should reject nested spawn");
        assert_eq!(
            error,
            RuntimeAgentRegistryError::ParentNotRunning {
                parent_path: RuntimeAgentPath::root().child("agent-1")
            }
        );
    }

    #[test]
    fn runtime_registry_generates_unique_nicknames_and_updates_task_messages() {
        let registry = RuntimeAgentRegistry::new(None);
        registry
            .register_root("root-agent", "build")
            .expect("root should register");
        let first = registry
            .spawn_subagent(
                "agent-1",
                &RuntimeAgentPath::root(),
                "Explore",
                None,
                None,
                None,
            )
            .expect("first subagent should spawn");
        let second = registry
            .spawn_subagent(
                "agent-2",
                &RuntimeAgentPath::root(),
                "Explore",
                None,
                None,
                None,
            )
            .expect("second subagent should spawn");
        assert_eq!(first.nickname, "explore");
        assert_eq!(second.nickname, "explore-2");

        let updated = registry
            .update_last_task_message("agent-2", "Inspect config loading")
            .expect("message should update");
        assert_eq!(
            updated.last_task_message.as_deref(),
            Some("Inspect config loading")
        );
    }
}
