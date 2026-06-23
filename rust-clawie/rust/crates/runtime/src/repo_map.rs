use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

const DEFAULT_MAX_FILES: usize = 200;
const MAX_INDEX_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoMapOptions {
    pub root: Option<String>,
    #[serde(rename = "maxFiles")]
    pub max_files: Option<usize>,
    #[serde(rename = "includeTests")]
    pub include_tests: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoMapEntry {
    pub path: String,
    pub language: String,
    pub symbols: Vec<String>,
    pub score: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoMapOutput {
    pub root: String,
    pub entries: Vec<RepoMapEntry>,
    pub languages: BTreeMap<String, usize>,
    pub truncated: bool,
    pub summary: String,
}

pub fn build_repo_map(options: RepoMapOptions) -> io::Result<RepoMapOutput> {
    let root = options
        .root
        .as_deref()
        .map(resolve_path)
        .transpose()?
        .unwrap_or(std::env::current_dir()?);
    let root = root.canonicalize()?;
    let max_files = options.max_files.unwrap_or(DEFAULT_MAX_FILES).max(1);
    let include_tests = options.include_tests.unwrap_or(true);

    let mut entries = Vec::new();
    let mut languages = BTreeMap::new();
    for entry in WalkDir::new(&root).into_iter().filter_entry(|entry| {
        !is_ignored_path(entry.path()) && (include_tests || !looks_like_test_path(entry.path()))
    }) {
        let entry = entry.map_err(|error| io::Error::other(error.to_string()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let Some(language) = language_for_path(path) else {
            continue;
        };
        let metadata = entry.metadata()?;
        if metadata.len() > MAX_INDEX_BYTES || is_binary_file(path)? {
            continue;
        }
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        let symbols = extract_symbols(&content, &language);
        let relative = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        let score = score_entry(&relative, &symbols, metadata.len());
        *languages.entry(language.clone()).or_insert(0) += 1;
        entries.push(RepoMapEntry {
            path: relative,
            language,
            symbols,
            score,
            bytes: metadata.len(),
        });
    }

    entries.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.path.cmp(&right.path))
    });
    let truncated = entries.len() > max_files;
    entries.truncate(max_files);
    let symbol_count = entries
        .iter()
        .map(|entry| entry.symbols.len())
        .sum::<usize>();
    let summary = format!(
        "Ranked {} files across {} languages with {} extracted symbols.",
        entries.len(),
        languages.len(),
        symbol_count
    );

    Ok(RepoMapOutput {
        root: root.to_string_lossy().into_owned(),
        entries,
        languages,
        truncated,
        summary,
    })
}

fn resolve_path(path: &str) -> io::Result<PathBuf> {
    let path = Path::new(path);
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn is_ignored_path(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        matches!(
            name.as_ref(),
            ".git"
                | ".hg"
                | ".svn"
                | "target"
                | "node_modules"
                | ".venv"
                | "venv"
                | "__pycache__"
                | ".mypy_cache"
                | ".pytest_cache"
                | "dist"
                | "build"
        )
    })
}

fn looks_like_test_path(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy().to_ascii_lowercase();
        name == "tests" || name == "test" || name.ends_with("_test") || name.ends_with(".test")
    })
}

fn is_binary_file(path: &Path) -> io::Result<bool> {
    use std::io::Read;
    let mut file = fs::File::open(path)?;
    let mut buffer = [0u8; 4096];
    let bytes_read = file.read(&mut buffer)?;
    Ok(buffer[..bytes_read].contains(&0))
}

fn language_for_path(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let language = match extension.as_str() {
        "rs" => "rust",
        "py" => "python",
        "js" | "jsx" => "javascript",
        "ts" | "tsx" => "typescript",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => "cpp",
        "cs" => "csharp",
        "rb" => "ruby",
        "php" => "php",
        "swift" => "swift",
        "scala" => "scala",
        "ex" | "exs" => "elixir",
        "erl" | "hrl" => "erlang",
        "hs" => "haskell",
        "clj" | "cljs" => "clojure",
        "dart" => "dart",
        "lua" => "lua",
        "r" => "r",
        "zig" => "zig",
        "sql" => "sql",
        "sh" | "bash" | "zsh" => "shell",
        "md" | "mdx" => "markdown",
        "toml" | "yaml" | "yml" | "json" => "config",
        _ => return None,
    };
    Some(language.to_string())
}

fn extract_symbols(content: &str, language: &str) -> Vec<String> {
    let patterns = match language {
        "rust" => vec![
            r"(?m)^\s*(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)",
            r"(?m)^\s*(?:pub\s+)?(?:struct|enum|trait|mod)\s+([A-Za-z_][A-Za-z0-9_]*)",
        ],
        "python" => vec![
            r"(?m)^\s*(?:async\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)",
            r"(?m)^\s*class\s+([A-Za-z_][A-Za-z0-9_]*)",
        ],
        "javascript" | "typescript" => vec![
            r"(?m)^\s*(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][A-Za-z0-9_$]*)",
            r"(?m)^\s*(?:export\s+)?class\s+([A-Za-z_$][A-Za-z0-9_$]*)",
            r"(?m)^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=",
        ],
        "go" => vec![r"(?m)^\s*func\s+(?:\([^)]*\)\s*)?([A-Za-z_][A-Za-z0-9_]*)"],
        "java" | "kotlin" | "csharp" | "scala" => vec![
            r"(?m)^\s*(?:public|private|protected|internal|final|open|static|\s)*\s*(?:class|interface|enum|object|record)\s+([A-Za-z_][A-Za-z0-9_]*)",
        ],
        "c" | "cpp" => vec![
            r"(?m)^\s*(?:class|struct|enum)\s+([A-Za-z_][A-Za-z0-9_]*)",
            r"(?m)^\s*[A-Za-z_][A-Za-z0-9_:\<\>\*&\s]+\s+([A-Za-z_][A-Za-z0-9_]*)\s*\([^;]*\)\s*\{?",
        ],
        "ruby" => vec![
            r"(?m)^\s*def\s+([A-Za-z_][A-Za-z0-9_!?=]*)",
            r"(?m)^\s*class\s+([A-Za-z_:][A-Za-z0-9_:]*)",
        ],
        _ => vec![r"(?m)^\s*(?:function|def|fn|class|struct|enum)\s+([A-Za-z_][A-Za-z0-9_]*)"],
    };

    let mut symbols = Vec::new();
    for pattern in patterns {
        let Ok(regex) = Regex::new(pattern) else {
            continue;
        };
        for capture in regex.captures_iter(content) {
            if let Some(name) = capture.get(1) {
                let symbol = name.as_str().to_string();
                if !symbols.contains(&symbol) {
                    symbols.push(symbol);
                }
                if symbols.len() >= 24 {
                    return symbols;
                }
            }
        }
    }
    symbols
}

fn score_entry(path: &str, symbols: &[String], bytes: u64) -> usize {
    let lower = path.to_ascii_lowercase();
    let mut score = symbols.len() * 25;
    if lower.ends_with("main.rs")
        || lower.ends_with("lib.rs")
        || lower.ends_with("main.py")
        || lower.ends_with("index.ts")
        || lower.ends_with("index.js")
        || lower.ends_with("package.json")
        || lower.ends_with("cargo.toml")
        || lower.ends_with("pyproject.toml")
    {
        score += 80;
    }
    if lower.contains("/src/") || lower.starts_with("src/") {
        score += 40;
    }
    if lower.contains("test") {
        score = score.saturating_sub(20);
    }
    score + usize::try_from((MAX_INDEX_BYTES.min(bytes) / 4096).min(20)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{build_repo_map, extract_symbols, RepoMapOptions};

    #[test]
    fn extracts_rust_symbols() {
        let symbols = extract_symbols("pub struct Thing;\nasync fn run_it() {}\n", "rust");
        assert!(symbols.contains(&"Thing".to_string()));
        assert!(symbols.contains(&"run_it".to_string()));
    }

    #[test]
    fn maps_current_repo() {
        let map = build_repo_map(RepoMapOptions {
            root: Some(".".to_string()),
            max_files: Some(5),
            include_tests: Some(true),
        })
        .expect("repo map should build");
        assert!(!map.entries.is_empty());
    }
}
