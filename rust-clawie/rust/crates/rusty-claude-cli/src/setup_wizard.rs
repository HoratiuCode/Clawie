use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use runtime::{default_config_home, ConfigLoader};
use serde_json::{Map, Value};

const PROVIDERS: &[(&str, &str, &str)] = &[
    ("1", "Anthropic", "anthropic"),
    ("2", "xAI / Grok", "xai"),
    ("3", "OpenAI", "openai"),
    ("4", "Gemini", "gemini"),
    ("5", "DashScope", "dashscope"),
    ("6", "Custom OpenAI-compatible", "openai"),
];

const PROVIDER_MODELS: &[(&str, &[&str])] = &[
    ("anthropic", &["opus", "sonnet", "haiku"]),
    ("xai", &["grok", "grok-mini", "grok-2"]),
    ("openai", &["gpt-4.1", "gpt-4.1-mini", "gpt-4.1-nano"]),
    ("gemini", &["gemini-1.5-pro", "gemini-1.5-flash", "gemini-2.0-pro", "gemini-2.0-flash", "gemini-3.5-flash"]),
    ("dashscope", &["qwen-plus", "qwen-max", "kimi"]),
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
    println!("  Configure provider, API key, base URL, and model.");
    println!("  Press Enter to keep the current/default value.");
    println!();

    let provider = prompt_provider(current.provider())?;
    let model = prompt_model(&provider, current.model())?;
    let api_key = prompt_secret(&provider)?;
    let base_url = prompt_base_url(&provider)?;
    let settings_path = save_settings(
        &provider,
        model.as_deref(),
        api_key.as_deref(),
        base_url.as_deref(),
    )?;

    println!();
    println!("  Provider settings saved");
    println!("  File             {}", settings_path.display());
    println!("  Provider         {provider}");
    println!(
        "  Model            {}",
        model.as_deref().unwrap_or("(unchanged)")
    );
    println!();
    Ok(())
}

fn prompt_provider(current: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
    let current = current.unwrap_or("anthropic");
    println!("  Provider");
    for (num, label, provider) in PROVIDERS {
        let marker = if *provider == current {
            " (current)"
        } else {
            ""
        };
        println!("    [{num}] {label}{marker}");
    }
    let default = PROVIDERS
        .iter()
        .position(|(_, _, provider)| *provider == current)
        .map_or(1, |index| index + 1);
    let input = read_line(&format!("  Select provider [{default}]: "))?;
    let choice = if input.trim().is_empty() {
        default.to_string()
    } else {
        input.trim().to_string()
    };
    PROVIDERS
        .iter()
        .find(|(num, _, _)| *num == choice)
        .map(|(_, _, provider)| (*provider).to_string())
        .ok_or_else(|| format!("invalid provider choice: {choice}").into())
}

fn prompt_model(
    provider: &str,
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
    let current = current.unwrap_or_else(|| models.first().copied().unwrap_or("sonnet"));
    let input = read_line(&format!("  Model [{current}]: "))?;
    Ok(if input.trim().is_empty() {
        Some(current.to_string())
    } else {
        Some(input.trim().to_string())
    })
}

fn prompt_secret(provider: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let env_var = env_var_for(provider, API_KEY_ENV_VARS).unwrap_or("API_KEY");
    if std::env::var(env_var).is_ok_and(|value| !value.is_empty()) {
        println!("  {env_var} is already set in the environment and will take precedence.");
    }
    let input = read_line(&format!("  API key for {env_var} [leave blank to skip]: "))?;
    Ok((!input.trim().is_empty()).then(|| input.trim().to_string()))
}

fn prompt_base_url(provider: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let env_var = env_var_for(provider, BASE_URL_ENV_VARS).unwrap_or("BASE_URL");
    let input = read_line(&format!("  Base URL for {env_var} [leave blank to skip]: "))?;
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
