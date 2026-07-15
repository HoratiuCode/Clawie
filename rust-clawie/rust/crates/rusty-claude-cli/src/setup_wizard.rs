use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use runtime::{default_config_home, ConfigLoader};
use serde_json::{Map, Value};

#[derive(Clone, Copy)]
struct ProviderOption {
    number: &'static str,
    label: &'static str,
    provider: &'static str,
    requires_base_url: bool,
}

struct ProviderSelection {
    label: &'static str,
    provider: String,
    requires_base_url: bool,
}

const PROVIDERS: &[ProviderOption] = &[
    ProviderOption {
        number: "1",
        label: "Anthropic",
        provider: "anthropic",
        requires_base_url: false,
    },
    ProviderOption {
        number: "2",
        label: "xAI / Grok",
        provider: "xai",
        requires_base_url: false,
    },
    ProviderOption {
        number: "3",
        label: "OpenAI",
        provider: "openai",
        requires_base_url: false,
    },
    ProviderOption {
        number: "4",
        label: "Gemini",
        provider: "gemini",
        requires_base_url: false,
    },
    ProviderOption {
        number: "5",
        label: "DashScope",
        provider: "dashscope",
        requires_base_url: false,
    },
    ProviderOption {
        number: "6",
        label: "Codex CLI",
        provider: "codex",
        requires_base_url: false,
    },
    ProviderOption {
        number: "7",
        label: "Custom OpenAI-compatible",
        provider: "openai",
        requires_base_url: true,
    },
];

const PROVIDER_MODELS: &[(&str, &[&str])] = &[
    ("anthropic", &["opus", "sonnet", "haiku"]),
    ("xai", &["grok", "grok-mini", "grok-2"]),
    ("openai", &["gpt-4.1", "gpt-4.1-mini", "gpt-4.1-nano"]),
    (
        "gemini",
        &[
            "gemini-1.5-pro",
            "gemini-1.5-flash",
            "gemini-2.0-pro",
            "gemini-2.0-flash",
            "gemini-3.5-flash",
        ],
    ),
    ("dashscope", &["qwen-plus", "qwen-max", "kimi"]),
    ("codex", &["codex"]),
];

const API_KEY_ENV_VARS: &[(&str, &str)] = &[
    ("anthropic", "ANTHROPIC_API_KEY"),
    ("xai", "XAI_API_KEY"),
    ("openai", "OPENAI_API_KEY"),
    ("gemini", "GEMINI_API_KEY"),
    ("dashscope", "DASHSCOPE_API_KEY"),
];

const BASE_URL_ENV_VARS: &[(&str, &str)] = &[
    ("anthropic", "ANTHROPIC_BASE_URL"),
    ("xai", "XAI_BASE_URL"),
    ("openai", "OPENAI_BASE_URL"),
    ("gemini", "GEMINI_BASE_URL"),
    ("dashscope", "DASHSCOPE_BASE_URL"),
];

pub fn run_setup_wizard() -> Result<(), Box<dyn std::error::Error>> {
    if !io::stdin().is_terminal() {
        return Err("setup wizard requires an interactive terminal".into());
    }

    let cwd = std::env::current_dir()?;
    let current = ConfigLoader::default_for(&cwd)
        .load()
        .unwrap_or_else(|_| runtime::RuntimeConfig::empty());

    println!();
    println!("  Clawie Setup Wizard");
    println!("  Configure provider, credentials, and model.");
    println!("  Press Enter to keep the current/default value.");
    println!();

    let current_provider = current.provider();
    let selection = prompt_provider(current_provider)?;
    let model = prompt_model(&selection.provider, current_provider, current.model())?;
    let (api_key, base_url) = if selection.provider == "codex" {
        print_codex_login_hint();
        (None, None)
    } else {
        (
            prompt_secret(&selection.provider)?,
            prompt_base_url(&selection.provider, selection.requires_base_url)?,
        )
    };
    let settings_path = save_settings(
        &selection.provider,
        model.as_deref(),
        api_key.as_deref(),
        base_url.as_deref(),
    )?;

    println!();
    println!("  Provider settings saved");
    println!("  File             {}", settings_path.display());
    if selection.requires_base_url {
        println!(
            "  Provider         {} (runtime provider: {})",
            selection.label, selection.provider
        );
    } else {
        println!("  Provider         {}", selection.provider);
    }
    println!(
        "  Model            {}",
        model.as_deref().unwrap_or("(unchanged)")
    );
    println!();
    Ok(())
}

pub fn clear_provider_settings() -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    let settings_path = default_config_home().join("settings.json");
    if !settings_path.exists() {
        return Ok(None);
    }

    let mut root = read_settings_object(&settings_path)?;
    for key in [
        "provider",
        "preferredProvider",
        "model",
        "model_list",
        "modelList",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "XAI_API_KEY",
        "XAI_BASE_URL",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "GEMINI_API_KEY",
        "GEMINI_BASE_URL",
        "GOOGLE_API_KEY",
        "DASHSCOPE_API_KEY",
        "DASHSCOPE_BASE_URL",
        "CODEX_CLI",
        "MOONSHOT_API_KEY",
        "MOONSHOT_BASE_URL",
        "KIMI_API_KEY",
    ] {
        root.remove(key);
    }
    fs::write(
        &settings_path,
        serde_json::to_string_pretty(&Value::Object(root))?,
    )?;
    Ok(Some(settings_path))
}

fn prompt_provider(current: Option<&str>) -> Result<ProviderSelection, Box<dyn std::error::Error>> {
    let current = current.unwrap_or("anthropic");
    println!("  Provider");
    for option in PROVIDERS {
        let marker = if option.provider == current && !option.requires_base_url {
            " (current)"
        } else {
            ""
        };
        println!("    [{}] {}{}", option.number, option.label, marker);
    }
    let default = PROVIDERS
        .iter()
        .position(|option| option.provider == current && !option.requires_base_url)
        .map_or(1, |index| index + 1);
    let input = read_line(&format!("  Select provider [{default}]: "))?;
    let choice = if input.trim().is_empty() {
        default.to_string()
    } else {
        input.trim().to_string()
    };
    PROVIDERS
        .iter()
        .find(|option| option.number == choice)
        .map(|option| ProviderSelection {
            label: option.label,
            provider: option.provider.to_string(),
            requires_base_url: option.requires_base_url,
        })
        .ok_or_else(|| format!("invalid provider choice: {choice}").into())
}

fn prompt_model(
    provider: &str,
    current_provider: Option<&str>,
    current: Option<&str>,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let models = PROVIDER_MODELS
        .iter()
        .find(|(candidate, _)| *candidate == provider)
        .map_or(&[][..], |(_, models)| *models);
    if !models.is_empty() {
        println!();
        println!("  Suggested models: {}", models.join(", "));
    }
    let default_model = default_model_for_provider(provider, current_provider, current, models);
    let input = read_line(&format!("  Model [{default_model}]: "))?;
    Ok(if input.trim().is_empty() {
        Some(default_model.to_string())
    } else {
        Some(input.trim().to_string())
    })
}

fn default_model_for_provider<'a>(
    provider: &str,
    current_provider: Option<&str>,
    current_model: Option<&'a str>,
    provider_models: &'a [&'a str],
) -> &'a str {
    let current_matches_provider = current_provider == Some(provider);
    if current_matches_provider {
        if let Some(model) = current_model {
            let model_has_provider_prefix = model
                .split_once('/')
                .is_some_and(|(candidate, _)| candidate == provider);
            if provider_models.is_empty()
                || provider_models.contains(&model)
                || model_has_provider_prefix
            {
                return model;
            }
        }
    }
    provider_models.first().copied().unwrap_or("sonnet")
}

fn prompt_secret(provider: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let env_var = env_var_for(provider, API_KEY_ENV_VARS).unwrap_or("API_KEY");
    if std::env::var(env_var).is_ok_and(|value| !value.is_empty()) {
        println!("  {env_var} is already set in the environment and will take precedence.");
    }
    let input = read_line(&format!("  API key for {env_var} [leave blank to skip]: "))?;
    Ok((!input.trim().is_empty()).then(|| input.trim().to_string()))
}

fn print_codex_login_hint() {
    println!();
    println!("  Codex CLI uses your existing Codex login instead of an API key.");
    match std::process::Command::new("codex")
        .args(["login", "status"])
        .output()
    {
        Ok(output) if output.status.success() => {
            println!("  Codex login       detected");
        }
        Ok(_) => {
            println!("  Codex login       not detected");
            println!("  Run `codex login` first to use ChatGPT/Codex credits.");
        }
        Err(_) => {
            println!("  Codex CLI         not found in PATH");
            println!("  Install Codex CLI and run `codex login` first.");
        }
    }
}

fn prompt_base_url(
    provider: &str,
    required: bool,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let env_var = env_var_for(provider, BASE_URL_ENV_VARS).unwrap_or("BASE_URL");
    let env_has_value = std::env::var(env_var).is_ok_and(|value| !value.is_empty());
    if env_has_value {
        println!("  {env_var} is already set in the environment and will take precedence.");
    }
    let hint = if required {
        "required for custom endpoint"
    } else {
        "leave blank to skip"
    };
    let input = read_line(&format!("  Base URL for {env_var} [{hint}]: "))?;
    if required && input.trim().is_empty() && !env_has_value {
        return Err(format!(
            "{env_var} is required for Custom OpenAI-compatible. Choose OpenAI for the default OpenAI API, or enter your custom base URL."
        )
        .into());
    }
    Ok((!input.trim().is_empty()).then(|| input.trim().to_string()))
}

fn save_settings(
    provider: &str,
    model: Option<&str>,
    api_key: Option<&str>,
    base_url: Option<&str>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let config_home = default_config_home();
    fs::create_dir_all(&config_home)?;
    let settings_path = config_home.join("settings.json");
    let mut root = read_settings_object(&settings_path)?;
    root.insert("provider".to_string(), Value::String(provider.to_string()));
    if let Some(model) = model {
        root.insert("model".to_string(), Value::String(model.to_string()));
        upsert_model_list_entry(&mut root, provider, model, api_key, base_url);
    }
    if let Some(api_key) = api_key {
        if let Some(env_var) = env_var_for(provider, API_KEY_ENV_VARS) {
            root.insert(env_var.to_string(), Value::String(api_key.to_string()));
        }
    }
    if let Some(base_url) = base_url {
        if let Some(env_var) = env_var_for(provider, BASE_URL_ENV_VARS) {
            root.insert(env_var.to_string(), Value::String(base_url.to_string()));
        }
    }
    fs::write(
        &settings_path,
        serde_json::to_string_pretty(&Value::Object(root))?,
    )?;
    Ok(settings_path)
}

fn upsert_model_list_entry(
    root: &mut Map<String, Value>,
    provider: &str,
    model: &str,
    api_key: Option<&str>,
    base_url: Option<&str>,
) {
    let model_ref = if model.contains('/') {
        model.to_string()
    } else if provider == "codex" {
        model.to_string()
    } else {
        format!("{provider}/{model}")
    };
    let mut entry = Map::new();
    entry.insert("model_name".to_string(), Value::String(model.to_string()));
    entry.insert("model".to_string(), Value::String(model_ref));
    entry.insert("provider".to_string(), Value::String(provider.to_string()));
    if provider == "codex" {
        entry.insert("connect_mode".to_string(), Value::String("cli".to_string()));
    }
    if let Some(base_url) = base_url {
        entry.insert("api_base".to_string(), Value::String(base_url.to_string()));
    }
    if api_key.is_some() {
        entry.insert(
            "auth_method".to_string(),
            Value::String("api_key".to_string()),
        );
    }

    let list = root
        .entry("model_list".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Value::Array(entries) = list else {
        *list = Value::Array(vec![Value::Object(entry)]);
        return;
    };
    if let Some(existing) = entries.iter_mut().find(|value| {
        value
            .as_object()
            .and_then(|object| object.get("model_name"))
            .and_then(Value::as_str)
            == Some(model)
    }) {
        *existing = Value::Object(entry);
    } else {
        entries.push(Value::Object(entry));
    }
}

fn read_settings_object(path: &PathBuf) -> Result<Map<String, Value>, Box<dyn std::error::Error>> {
    if !path.exists() {
        return Ok(Map::new());
    }
    let value: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    Ok(value.as_object().cloned().unwrap_or_default())
}

fn env_var_for<'a>(provider: &str, values: &'a [(&str, &str)]) -> Option<&'a str> {
    values
        .iter()
        .find(|(candidate, _)| *candidate == provider)
        .map(|(_, env_var)| *env_var)
}

fn read_line(prompt: &str) -> io::Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input)
}

#[cfg(test)]
mod tests {
    use super::{default_model_for_provider, upsert_model_list_entry};
    use serde_json::{Map, Value};

    #[test]
    fn switching_provider_uses_new_provider_default_model() {
        let openai_models = ["gpt-4.1", "gpt-4.1-mini", "gpt-4.1-nano"];
        assert_eq!(
            default_model_for_provider(
                "openai",
                Some("gemini"),
                Some("gemini-1.5-flash"),
                &openai_models,
            ),
            "gpt-4.1"
        );
    }

    #[test]
    fn same_provider_keeps_supported_current_model() {
        let openai_models = ["gpt-4.1", "gpt-4.1-mini", "gpt-4.1-nano"];
        assert_eq!(
            default_model_for_provider(
                "openai",
                Some("openai"),
                Some("gpt-4.1-mini"),
                &openai_models
            ),
            "gpt-4.1-mini"
        );
    }

    #[test]
    fn same_provider_replaces_incompatible_current_model() {
        let openai_models = ["gpt-4.1", "gpt-4.1-mini", "gpt-4.1-nano"];
        assert_eq!(
            default_model_for_provider(
                "openai",
                Some("openai"),
                Some("gemini-1.5-flash"),
                &openai_models,
            ),
            "gpt-4.1"
        );
    }

    #[test]
    fn same_provider_keeps_provider_prefixed_model() {
        let openai_models = ["gpt-4.1", "gpt-4.1-mini", "gpt-4.1-nano"];
        assert_eq!(
            default_model_for_provider(
                "openai",
                Some("openai"),
                Some("openai/custom-model"),
                &openai_models,
            ),
            "openai/custom-model"
        );
    }

    #[test]
    fn setup_wizard_writes_model_list_connection_entry() {
        let mut root = Map::new();
        upsert_model_list_entry(
            &mut root,
            "openai",
            "gpt-4.1-mini",
            Some("sk-test"),
            Some("https://example.test/v1"),
        );

        let entries = root
            .get("model_list")
            .and_then(Value::as_array)
            .expect("model_list should be an array");
        let entry = entries[0].as_object().expect("entry should be object");
        assert_eq!(
            entry.get("model_name").and_then(Value::as_str),
            Some("gpt-4.1-mini")
        );
        assert_eq!(
            entry.get("model").and_then(Value::as_str),
            Some("openai/gpt-4.1-mini")
        );
        assert_eq!(
            entry.get("api_base").and_then(Value::as_str),
            Some("https://example.test/v1")
        );
        assert_eq!(
            entry.get("auth_method").and_then(Value::as_str),
            Some("api_key")
        );
    }

    #[test]
    fn setup_wizard_does_not_double_prefix_provider_model_refs() {
        let mut root = Map::new();
        upsert_model_list_entry(&mut root, "openai", "openai/custom-model", None, None);

        let entries = root
            .get("model_list")
            .and_then(Value::as_array)
            .expect("model_list should be an array");
        let entry = entries[0].as_object().expect("entry should be object");
        assert_eq!(
            entry.get("model").and_then(Value::as_str),
            Some("openai/custom-model")
        );
    }
}
