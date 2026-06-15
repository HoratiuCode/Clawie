use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::thread;

use serde::Deserialize;
use serde_json::json;

const MAX_REQUEST_BYTES: usize = 5 * 1024 * 1024;
static SERVER_PORT: Mutex<Option<u16>> = Mutex::new(None);

#[derive(Debug, Deserialize)]
struct SaveRequest {
    directory: String,
    filename: String,
    code: String,
    improvements: String,
}

#[derive(Debug, Deserialize)]
struct LoadRequest {
    directory: String,
    filename: String,
}

#[derive(Debug, Deserialize)]
struct DirectoryRequest {
    directory: String,
}

#[derive(Debug, Deserialize)]
struct ChatRequest {
    message: String,
    model: Option<String>,
    openai_api_key: Option<String>,
    anthropic_api_key: Option<String>,
    openai_base_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UploadRequest {
    directory: String,
    filename: String,
    content: String,
}

pub fn launch() -> Result<(String, PathBuf), Box<dyn std::error::Error>> {
    let output_dir = documents_output_dir()?;
    fs::create_dir_all(&output_dir)?;

    let mut server_port = SERVER_PORT
        .lock()
        .map_err(|_| io::Error::other("web UI server lock is unavailable"))?;
    let port = match *server_port {
        Some(port) => port,
        None => {
            let listener = TcpListener::bind(("127.0.0.1", 0))?;
            let port = listener.local_addr()?.port();
            let server_output_dir = output_dir.clone();
            thread::Builder::new()
                .name("clawie-webui".to_string())
                .spawn(move || serve(listener, server_output_dir))?;
            *server_port = Some(port);
            port
        }
    };

    let url = format!("http://127.0.0.1:{port}/");
    open_browser(&url)?;
    Ok((url, output_dir))
}

fn documents_output_dir() -> io::Result<PathBuf> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not configured"))?;
    Ok(home.join("Documents").join("Clawie WebUI"))
}

fn serve(listener: TcpListener, output_dir: PathBuf) {
    for connection in listener.incoming() {
        match connection {
            Ok(mut stream) => {
                if let Err(error) = handle_connection(&mut stream, &output_dir) {
                    let _ = write_json_response(
                        &mut stream,
                        "500 Internal Server Error",
                        &json!({"ok": false, "error": error.to_string()}).to_string(),
                    );
                }
            }
            Err(error) => eprintln!("webui: connection failed: {error}"),
        }
    }
}

fn handle_connection(stream: &mut TcpStream, output_dir: &Path) -> io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    let request = read_request(stream)?;
    let header_end = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid HTTP request"))?;
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let request_line = headers.lines().next().unwrap_or_default();

    match request_line {
        line if line.starts_with("GET / ") => write_html_response(stream, WEB_UI_HTML),
        line if line.starts_with("GET /health ") => {
            write_json_response(stream, "200 OK", r#"{"ok":true}"#)
        }
        line if line.starts_with("GET /locations ") => {
            let locations = suggested_locations(output_dir)?;
            write_json_response(
                stream,
                "200 OK",
                &json!({"ok": true, "locations": locations}).to_string(),
            )
        }
        line if line.starts_with("POST /files ") => {
            let payload: DirectoryRequest = parse_json_body(&request, header_end, "files")?;
            let directory = resolve_output_directory(output_dir, &payload.directory)?;
            write_json_response(
                stream,
                "200 OK",
                &json!({
                    "ok": true,
                    "directory": directory.display().to_string(),
                    "files": list_code_files(&directory)?,
                })
                .to_string(),
            )
        }
        line if line.starts_with("POST /load ") => {
            let payload: LoadRequest = parse_json_body(&request, header_end, "load")?;
            let directory = resolve_output_directory(output_dir, &payload.directory)?;
            let (code, improvements) = load_workspace_files(&directory, &payload.filename)?;
            write_json_response(
                stream,
                "200 OK",
                &json!({
                    "ok": true,
                    "filename": payload.filename,
                    "code": code,
                    "improvements": improvements,
                })
                .to_string(),
            )
        }
        line if line.starts_with("POST /save ") => {
            let payload: SaveRequest = parse_json_body(&request, header_end, "save")?;
            let directory = resolve_output_directory(output_dir, &payload.directory)?;
            let saved = save_workspace_files(&directory, &payload)?;
            write_json_response(
                stream,
                "200 OK",
                &json!({
                    "ok": true,
                    "code_path": saved.0.display().to_string(),
                    "improvements_path": saved.1.display().to_string(),
                })
                .to_string(),
            )
        }
        line if line.starts_with("POST /select-directory ") => {
            let selected = select_directory_via_dialog()?;
            write_json_response(
                stream,
                "200 OK",
                &json!({
                    "ok": true,
                    "directory": selected,
                })
                .to_string(),
            )
        }
        line if line.starts_with("POST /chat ") => {
            let payload: ChatRequest = parse_json_body(&request, header_end, "chat")?;
            let response_data = run_clawie_prompt(
                &payload.message,
                payload.model.as_deref(),
                payload.openai_api_key.as_deref(),
                payload.anthropic_api_key.as_deref(),
                payload.openai_base_url.as_deref(),
            )?;
            write_json_response(
                stream,
                "200 OK",
                &json!({
                    "ok": true,
                    "reply": response_data["reply"],
                    "input_tokens": response_data["input_tokens"],
                    "output_tokens": response_data["output_tokens"],
                    "estimated_cost": response_data["estimated_cost"],
                })
                .to_string(),
            )
        }
        line if line.starts_with("POST /upload ") => {
            let payload: UploadRequest = parse_json_body(&request, header_end, "upload")?;
            let directory = resolve_output_directory(output_dir, &payload.directory)?;
            let file_path = directory.join(safe_filename(&payload.filename)?);
            fs::write(&file_path, &payload.content)?;
            write_json_response(
                stream,
                "200 OK",
                &json!({
                    "ok": true,
                    "filename": payload.filename,
                    "path": file_path.display().to_string(),
                })
                .to_string(),
            )
        }
        _ => write_json_response(
            stream,
            "404 Not Found",
            r#"{"ok":false,"error":"not found"}"#,
        ),
    }
}

fn select_directory_via_dialog() -> io::Result<Option<String>> {
    if cfg!(target_os = "macos") {
        let run_result = Command::new("osascript")
            .arg("-e")
            .arg("POSIX path of (choose folder with prompt \"Select Clawie Workspace Folder\")")
            .output();
        match run_result {
            Ok(output) => {
                if output.status.success() {
                    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !path.is_empty() {
                        return Ok(Some(path));
                    }
                }
            }
            Err(e) => {
                eprintln!("webui: failed to spawn osascript: {e}");
            }
        }
    }
    Ok(None)
}

fn parse_json_body<T>(request: &[u8], header_end: usize, name: &str) -> io::Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_slice(&request[header_end + 4..]).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {name} request: {error}"),
        )
    })
}

fn suggested_locations(default_output_dir: &Path) -> io::Result<Vec<serde_json::Value>> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not configured"))?;
    let cwd = env::current_dir()?;
    Ok([
        ("Clawie WebUI", default_output_dir.to_path_buf()),
        ("Documents", home.join("Documents")),
        ("Desktop", home.join("Desktop")),
        ("Downloads", home.join("Downloads")),
        ("Current project", cwd),
    ]
    .into_iter()
    .map(|(label, path)| json!({"label": label, "path": path.display().to_string()}))
    .collect())
}

fn resolve_output_directory(default_output_dir: &Path, requested: &str) -> io::Result<PathBuf> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Ok(default_output_dir.to_path_buf());
    }
    let expanded = if requested == "~" || requested.starts_with("~/") {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not configured"))?;
        if requested == "~" {
            home
        } else {
            home.join(&requested[2..])
        }
    } else {
        PathBuf::from(requested)
    };
    if !expanded.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "save location must be an absolute folder path",
        ));
    }
    Ok(expanded)
}

fn list_code_files(output_dir: &Path) -> io::Result<Vec<String>> {
    if !output_dir.exists() {
        return Ok(Vec::new());
    }
    let mut files = fs::read_dir(output_dir)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.ends_with(".improvements.md"))
        .collect::<Vec<_>>();
    files.sort_by_key(|name| name.to_ascii_lowercase());
    Ok(files)
}

fn load_workspace_files(
    output_dir: &Path,
    requested_filename: &str,
) -> io::Result<(String, String)> {
    let filename = safe_filename(requested_filename)?;
    let code_path = output_dir.join(&filename);
    let stem = Path::new(&filename)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("code");
    let improvements_path = output_dir.join(format!("{stem}.improvements.md"));
    let code = fs::read_to_string(code_path)?;
    let improvements = fs::read_to_string(improvements_path)
        .unwrap_or_default()
        .lines()
        .skip_while(|line| line.starts_with('#') || line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Ok((code, improvements))
}

fn read_request(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 8192];
    let mut expected_len = None;

    loop {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..count]);
        if request.len() > MAX_REQUEST_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request is larger than 5 MB",
            ));
        }

        if expected_len.is_none() {
            if let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                });
                expected_len = Some(header_end + 4 + content_length.unwrap_or(0));
            }
        }

        if expected_len.is_some_and(|length| request.len() >= length) {
            request.truncate(expected_len.unwrap_or(request.len()));
            break;
        }
    }

    Ok(request)
}

fn save_workspace_files(
    output_dir: &Path,
    payload: &SaveRequest,
) -> io::Result<(PathBuf, PathBuf)> {
    let filename = safe_filename(&payload.filename)?;
    fs::create_dir_all(output_dir)?;
    let code_path = output_dir.join(&filename);
    let stem = Path::new(&filename)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("code");
    let improvements_path = output_dir.join(format!("{stem}.improvements.md"));

    fs::write(&code_path, &payload.code)?;
    fs::write(
        &improvements_path,
        format!(
            "# Improvements for `{filename}`\n\n{}\n",
            payload.improvements.trim()
        ),
    )?;
    Ok((code_path, improvements_path))
}

fn safe_filename(input: &str) -> io::Result<String> {
    let filename = input.trim();
    let path = Path::new(filename);
    let is_single_normal_component = path.components().count() == 1
        && path.file_name().and_then(|value| value.to_str()) == Some(filename)
        && filename != "."
        && filename != ".."
        && !filename.contains(['/', '\\', '\0']);
    if !is_single_normal_component {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "filename must be a single file name without folders",
        ));
    }
    Ok(filename.to_string())
}

fn run_clawie_prompt(
    message: &str,
    model: Option<&str>,
    openai_api_key: Option<&str>,
    anthropic_api_key: Option<&str>,
    openai_base_url: Option<&str>,
) -> io::Result<serde_json::Value> {
    let prompt = message.trim();
    if prompt.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "message cannot be empty",
        ));
    }

    let mut cmd = Command::new(env::current_exe()?);
    if let Some(m) = model {
        if !m.trim().is_empty() {
            cmd.arg("--model").arg(m);
        }
    }
    if let Some(key) = openai_api_key {
        if !key.trim().is_empty() {
            cmd.env("OPENAI_API_KEY", key.trim());
        }
    }
    if let Some(key) = anthropic_api_key {
        if !key.trim().is_empty() {
            cmd.env("ANTHROPIC_API_KEY", key.trim());
        }
    }
    if let Some(url) = openai_base_url {
        if !url.trim().is_empty() {
            cmd.env("OPENAI_BASE_URL", url.trim());
        }
    }
    let output = cmd
        .arg("--output-format")
        .arg("json")
        .arg("prompt")
        .arg(prompt)
        .output()?;

    if output.status.success() {
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        let parsed: serde_json::Value = serde_json::from_str(&stdout_str).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to parse json response: {e}"),
            )
        })?;
        let reply = parsed["message"].as_str().unwrap_or("").trim().to_string();
        let input_tokens = parsed["usage"]["input_tokens"].as_u64().unwrap_or(0);
        let output_tokens = parsed["usage"]["output_tokens"].as_u64().unwrap_or(0);
        let estimated_cost = parsed["estimated_cost"].as_str().unwrap_or("$0.00").to_string();
        
        return Ok(json!({
            "reply": if reply.is_empty() { "Clawie finished without text output.".to_string() } else { reply },
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "estimated_cost": estimated_cost,
        }));
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(io::Error::other(if stderr.is_empty() {
        format!("Clawie prompt failed with status {}", output.status)
    } else {
        stderr
    }))
}

fn write_html_response(stream: &mut TcpStream, body: &str) -> io::Result<()> {
    write_response(stream, "200 OK", "text/html; charset=utf-8", body)
}

fn write_json_response(stream: &mut TcpStream, status: &str, body: &str) -> io::Result<()> {
    write_response(stream, status, "application/json; charset=utf-8", body)
}

fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\ncache-control: no-store\r\ncontent-security-policy: default-src 'self'; style-src 'unsafe-inline'; script-src 'unsafe-inline'\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn open_browser(url: &str) -> io::Result<()> {
    let (program, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(target_os = "windows") {
        ("cmd", vec!["/C", "start", "", url])
    } else {
        ("xdg-open", vec![url])
    };
    Command::new(program).args(args).spawn().map(|_| ())
}

const WEB_UI_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>Clawie Workspace</title>
  <link rel="icon" href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'%3E%3Ctext y='.9em' font-size='90'%3E%F0%9F%A6%90%3C/text%3E%3C/svg%3E">
  <link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500;600&display=swap" rel="stylesheet">
  <style>
    :root {
      color-scheme: dark;
      --bg-main: #09090b;       /* Zinc 950 */
      --bg-sidebar: #0f0f11;    /* Sleek sidebar */
      --bg-card: #18181b;       /* Zinc 900 */
      --bg-input: #09090b;      /* Zinc 950 input */
      --border: #27272a;        /* Zinc 800 */
      --border-hover: #3f3f46;  /* Zinc 700 */
      --text-primary: #f4f4f5;  /* Zinc 100 */
      --text-secondary: #d4d4d8;/* Zinc 300 */
      --text-muted: #71717a;    /* Zinc 500 */
      --text-disabled: #52525b; /* Zinc 600 */
      --accent-rgb: 249, 115, 22; /* Vibrant Orange RGB */
      --accent: rgb(var(--accent-rgb));
      --accent-hover: #ea580c;
      --accent-soft: rgba(var(--accent-rgb), 0.15);
      --ok: #10b981;            /* Emerald 500 */
      --warn: #f59e0b;          /* Amber 500 */
      --radius-lg: 12px;
      --radius-md: 8px;
      --radius-sm: 6px;
      --font-ui: "Inter", system-ui, -apple-system, sans-serif;
      --font-code: "JetBrains Mono", ui-monospace, monospace;
    }

    * { box-sizing: border-box; margin: 0; padding: 0; }

    body {
      background: var(--bg-main);
      color: var(--text-primary);
      font-family: var(--font-ui);
      font-size: 14px;
      line-height: 1.5;
      min-height: 100vh;
      overflow: hidden;
      -webkit-font-smoothing: antialiased;
    }

    /* Layout structure */
    .app {
      display: flex;
      height: 100vh;
      overflow: hidden;
    }

    @media (max-width: 980px) {
      .right-sidebar {
        display: none !important;
      }
      #toggle-folders-btn {
        display: none !important;
      }
    }

    /* Sidebar Styling */
    .sidebar {
      background: var(--bg-sidebar);
      border-right: 1px solid var(--border);
      padding: 1.5rem 1.25rem;
      display: flex;
      flex-direction: column;
      gap: 1.25rem;
      overflow: hidden;
      width: 260px;
      flex: none;
      transition: width 0.25s cubic-bezier(0.4, 0, 0.2, 1), padding 0.25s cubic-bezier(0.4, 0, 0.2, 1), border-color 0.25s ease;
    }

    .sidebar.collapsed {
      width: 0 !important;
      padding-left: 0 !important;
      padding-right: 0 !important;
      border-right-width: 0 !important;
    }

    .sidebar > *, .right-sidebar > * {
      transition: opacity 0.15s ease;
    }

    .sidebar.collapsed > *, .right-sidebar.collapsed > * {
      opacity: 0 !important;
      pointer-events: none !important;
    }

    .sidebar-title {
      display: flex;
      align-items: center;
      justify-content: space-between;
    }

    .sidebar-title h2 {
      font-size: 0.75rem;
      font-weight: 700;
      text-transform: uppercase;
      letter-spacing: 0.08em;
      color: var(--text-muted);
    }

    .icon-btn-circle {
      width: 28px;
      height: 28px;
      border-radius: 50%;
      border: 1px solid var(--border);
      background: var(--bg-card);
      color: var(--text-secondary);
      display: grid;
      place-items: center;
      cursor: pointer;
      transition: all 0.15s ease;
    }

    .icon-btn-circle:hover {
      border-color: var(--border-hover);
      color: var(--text-primary);
      background: var(--border-hover);
    }

    #current-folder {
      font-size: 0.75rem;
      color: var(--text-muted);
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
      padding: 0.4rem 0.6rem;
      background: rgba(255, 255, 255, 0.02);
      border: 1px solid var(--border);
      border-radius: var(--radius-sm);
      display: flex;
      align-items: center;
      gap: 0.4rem;
    }

    .file-list {
      flex: 1;
      display: flex;
      flex-direction: column;
      gap: 0.25rem;
      overflow-y: auto;
    }

    .file-list::-webkit-scrollbar {
      width: 4px;
    }
    .file-list::-webkit-scrollbar-thumb {
      background: var(--border);
      border-radius: 2px;
    }

    .file {
      width: 100%;
      background: transparent;
      border: none;
      color: var(--text-secondary);
      padding: 0.5rem 0.75rem;
      text-align: left;
      border-radius: var(--radius-md);
      cursor: pointer;
      font-family: var(--font-code);
      font-size: 0.8rem;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
      display: flex;
      align-items: center;
      gap: 0.5rem;
      transition: all 0.15s ease;
    }

    .file:hover {
      background: rgba(255, 255, 255, 0.04);
      color: var(--text-primary);
    }

    .file.active {
      background: var(--accent-soft);
      color: var(--text-primary);
      font-weight: 600;
    }

    .hint {
      font-size: 0.75rem;
      color: var(--text-muted);
      text-align: center;
      padding: 1rem 0;
    }

    /* Workspace Content Area */
    .workspace {
      display: flex;
      flex-direction: column;
      height: 100vh;
      overflow: hidden;
      background: radial-gradient(circle at top right, rgba(var(--accent-rgb), 0.03), transparent 45%), var(--bg-main);
      flex: 1;
      min-width: 0;
    }

    header {
      height: 56px;
      border-bottom: 1px solid var(--border);
      display: flex;
      align-items: center;
      justify-content: space-between;
      padding: 0 2rem;
      background: rgba(9, 9, 11, 0.8);
      backdrop-filter: blur(8px);
      flex: none;
    }

    .top-brand {
      font-weight: 700;
      font-size: 0.85rem;
      letter-spacing: 0.1em;
      color: var(--text-primary);
      display: flex;
      align-items: center;
      gap: 0.5rem;
    }

    .brand-dot {
      width: 6px;
      height: 6px;
      border-radius: 50%;
      background: var(--accent-hover);
      box-shadow: 0 0 6px var(--accent-hover);
    }

    .plan-pill {
      font-size: 0.75rem;
      color: var(--text-muted);
      padding: 0.25rem 0.75rem;
      border: 1px solid var(--border);
      border-radius: 99px;
      background: rgba(255, 255, 255, 0.02);
    }

    .plan-pill strong {
      color: var(--text-secondary);
      font-weight: 600;
    }

    .status-pill {
      font-size: 0.75rem;
      color: var(--text-muted);
      padding: 0.35rem 0.75rem;
      border: 1px solid var(--border);
      border-radius: var(--radius-sm);
      background: rgba(255, 255, 255, 0.01);
      max-width: 250px;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
      transition: all 0.2s ease;
    }

    .status-pill.unsaved {
      border-color: rgba(245, 158, 11, 0.25);
      background: rgba(245, 158, 11, 0.05);
      color: var(--warn);
    }

    .status-pill.saved {
      border-color: rgba(16, 185, 129, 0.25);
      background: rgba(16, 185, 129, 0.05);
      color: var(--ok);
    }

    /* Main view container */
    main {
      flex: 1;
      overflow: hidden;
      padding: 1.5rem;
      display: flex;
      flex-direction: column;
      align-items: center;
    }

    .workspace-content-wrap {
      width: 100%;
      max-width: 1400px;
      height: 100%;
      display: grid;
      grid-template-columns: 1fr 1fr;
      gap: 1.5rem;
    }

    @media (max-width: 1200px) {
      .workspace-content-wrap {
        grid-template-columns: 1fr;
        overflow-y: auto;
      }
      .editor-panel {
        height: 450px !important;
      }
    }

    .welcome {
      text-align: center;
      margin-bottom: 0.5rem;
    }

    .welcome h1 {
      font-size: 1.75rem;
      font-weight: 700;
      color: var(--text-primary);
      letter-spacing: -0.02em;
    }

    .welcome h1 span.name {
      color: var(--accent-hover);
    }

    .welcome p {
      font-size: 0.85rem;
      color: var(--text-muted);
      margin-top: 0.25rem;
    }

    /* Code formatting styles in chat messages */
    .code-block {
      background: #050507;
      border: 1px solid var(--border);
      border-radius: var(--radius-md);
      margin: 0.5rem 0;
      overflow: hidden;
      font-family: var(--font-code);
      font-size: 0.8rem;
    }
    .code-header {
      background: var(--bg-sidebar);
      padding: 0.35rem 0.75rem;
      font-size: 0.7rem;
      font-weight: 600;
      color: var(--text-muted);
      border-bottom: 1px solid var(--border);
      text-transform: uppercase;
    }
    .code-block code {
      display: block;
      padding: 0.75rem;
      overflow-x: auto;
      white-space: pre;
      color: var(--text-secondary);
    }
    code {
      background: rgba(255, 255, 255, 0.06);
      padding: 0.1rem 0.3rem;
      border-radius: 4px;
      font-family: var(--font-code);
      font-size: 0.8rem;
    }
    .message strong {
      color: var(--text-primary);
      font-weight: 600;
    }

    input, select {
      background: var(--bg-input);
      border: 1px solid var(--border);
      border-radius: var(--radius-md);
      color: var(--text-primary);
      font-family: inherit;
      font-size: 0.8rem;
      padding: 0.5rem 0.75rem;
      outline: none;
      transition: all 0.15s ease;
    }

    input:focus, select:focus {
      border-color: var(--accent-hover);
      box-shadow: 0 0 0 2px rgba(var(--accent-rgb), 0.2);
    }

    /* Right Sidebar Styling */
    .right-sidebar {
      background: var(--bg-sidebar);
      border-left: 1px solid var(--border);
      padding: 1.5rem 0.85rem;
      display: flex;
      flex-direction: column;
      gap: 1rem;
      overflow: hidden;
      width: 200px;
      flex: none;
      transition: width 0.25s cubic-bezier(0.4, 0, 0.2, 1), padding 0.25s cubic-bezier(0.4, 0, 0.2, 1), border-color 0.25s ease;
    }

    .right-sidebar.collapsed {
      width: 0 !important;
      padding-left: 0 !important;
      padding-right: 0 !important;
      border-left-width: 0 !important;
    }

    .right-sidebar h2 {
      font-size: 0.7rem;
      font-weight: 700;
      text-transform: uppercase;
      letter-spacing: 0.08em;
      color: var(--text-muted);
    }

    #location-preset {
      width: 100%;
      background: var(--bg-input);
      border: 1px solid var(--border);
      border-radius: var(--radius-md);
      color: var(--text-primary);
      font-family: inherit;
      font-size: 0.75rem;
      outline: none;
      overflow-y: auto;
      height: 180px;
      padding: 0.25rem;
    }

    #location-preset option {
      padding: 0.35rem 0.5rem;
      border-radius: var(--radius-sm);
      cursor: pointer;
      font-size: 0.75rem;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }

    #location-preset option:hover {
      background: rgba(255, 255, 255, 0.04);
      color: var(--text-primary);
    }

    #location-preset option:checked {
      background: var(--accent-soft);
      color: var(--text-primary);
    }

    #location-path {
      font-size: 0.75rem;
      padding: 0.4rem 0.6rem;
      width: 100%;
    }

    /* Chat Interface Styling */
    .chat-panel {
      border: 1px solid var(--border);
      background: var(--bg-card);
      border-radius: var(--radius-lg);
      flex: 1;
      min-height: 0;
      display: flex;
      flex-direction: column;
      overflow: hidden;
      box-shadow: 0 12px 30px rgba(0, 0, 0, 0.25);
    }

    .chat-messages {
      flex: 1;
      overflow-y: auto;
      padding: 1.25rem;
      display: flex;
      flex-direction: column;
      gap: 1rem;
      scroll-behavior: smooth;
    }

    .chat-messages::-webkit-scrollbar {
      width: 6px;
    }
    .chat-messages::-webkit-scrollbar-thumb {
      background: var(--border);
      border-radius: 3px;
    }

    .message {
      max-width: 85%;
      padding: 0.75rem 1rem;
      border-radius: var(--radius-lg);
      line-height: 1.5;
      font-size: 0.85rem;
      display: flex;
      flex-direction: column;
      gap: 0.25rem;
    }

    .message-label {
      font-size: 0.65rem;
      font-weight: 700;
      text-transform: uppercase;
      letter-spacing: 0.05em;
      color: var(--text-muted);
    }

    .message.clawie {
      align-self: flex-start;
      background: rgba(255, 255, 255, 0.02);
      border: 1px solid var(--border);
      color: var(--text-secondary);
      border-top-left-radius: 4px;
    }

    .message.user {
      align-self: flex-end;
      background: var(--accent);
      color: #ffffff;
      border-top-right-radius: 4px;
    }

    .message.user .message-label {
      color: rgba(255, 255, 255, 0.7);
    }

    .chat-input-row {
      border-top: 1px solid var(--border);
      background: rgba(0, 0, 0, 0.15);
      padding: 1rem;
      display: flex;
      align-items: flex-end;
      gap: 0.75rem;
    }

    .chat-input-row textarea {
      background: var(--bg-input);
      border: 1px solid var(--border);
      border-radius: var(--radius-md);
      color: var(--text-primary);
      padding: 0.75rem 1rem;
      font-family: var(--font-ui);
      font-size: 0.9rem;
      min-height: 44px;
      max-height: 160px;
      flex: 1;
      outline: none;
      resize: none;
      transition: border-color 0.15s ease;
    }

    .chat-input-row textarea:focus {
      border-color: var(--accent-hover);
    }

    .icon-btn-round {
      width: 34px;
      height: 34px;
      border-radius: 50%;
      background: var(--accent);
      color: white;
      border: none;
      display: grid;
      place-items: center;
      cursor: pointer;
      transition: all 0.15s ease;
      flex: none;
    }

    .icon-btn-round:hover {
      background: var(--accent-hover);
    }

    .icon-btn-round:disabled {
      opacity: 0.5;
      cursor: not-allowed;
    }
    .accent-btn {
      background: var(--accent);
      color: #ffffff;
      border: none;
      padding: 0.5rem 1rem;
      border-radius: var(--radius-md);
      font-weight: 600;
      font-size: 0.8rem;
      cursor: pointer;
      display: flex;
      align-items: center;
      justify-content: center;
      gap: 0.5rem;
      transition: all 0.15s ease;
      width: 100%;
    }
    .accent-btn:hover {
      background: var(--accent-hover);
      transform: translateY(-1px);
    }
    .accent-btn:active {
      transform: translateY(0);
    }

    .suggestion-chip:hover {
      background: var(--accent-soft) !important;
      border-color: var(--accent) !important;
      color: var(--text-primary) !important;
      transform: translateY(-1px);
    }
    .suggestions-row::-webkit-scrollbar {
      display: none;
    }

    #editor-line-numbers::-webkit-scrollbar {
      display: none;
    }
    #editor-line-numbers {
      -ms-overflow-style: none;
      scrollbar-width: none;
    }

    #editor-textarea::-webkit-scrollbar {
      width: 6px;
      height: 6px;
    }
    #editor-textarea::-webkit-scrollbar-thumb {
      background: var(--border);
      border-radius: 3px;
    }
    #editor-textarea::-webkit-scrollbar-track {
      background: transparent;
    }

    @keyframes pulse-mic {
      0% { box-shadow: 0 0 0 0 rgba(239, 68, 68, 0.4); }
      70% { box-shadow: 0 0 0 8px rgba(239, 68, 68, 0); }
      100% { box-shadow: 0 0 0 0 rgba(239, 68, 68, 0); }
    }
    .listening-active {
      background: #ef4444 !important;
      color: white !important;
      border-color: #ef4444 !important;
      animation: pulse-mic 1.5s infinite;
    }

    /* Modal Styles */
    .modal-overlay {
      position: fixed;
      top: 0;
      left: 0;
      width: 100vw;
      height: 100vh;
      background: rgba(0, 0, 0, 0.6);
      backdrop-filter: blur(4px);
      display: grid;
      place-items: center;
      z-index: 1000;
      opacity: 1;
      transition: opacity 0.2s ease;
    }
    .modal-overlay[hidden] {
      display: none;
      opacity: 0;
      pointer-events: none;
    }
    .modal-content {
      background: var(--bg-card);
      border: 1px solid var(--border);
      border-radius: var(--radius-lg);
      width: min(400px, 90%);
      box-shadow: 0 20px 40px rgba(0, 0, 0, 0.5);
      overflow: hidden;
      animation: modalFadeIn 0.2s ease;
    }
    @keyframes modalFadeIn {
      from { transform: scale(0.95); opacity: 0; }
      to { transform: scale(1); opacity: 1; }
    }
    .modal-header {
      padding: 1.25rem 1.5rem;
      border-bottom: 1px solid var(--border);
      display: flex;
      align-items: center;
      justify-content: space-between;
    }
    .modal-header h3 {
      font-size: 1rem;
      font-weight: 600;
      color: var(--text-primary);
    }
    .close-btn {
      background: transparent;
      border: none;
      color: var(--text-muted);
      font-size: 1.5rem;
      cursor: pointer;
      line-height: 1;
      transition: color 0.15s ease;
    }
    .close-btn:hover {
      color: var(--text-primary);
    }
    .modal-body {
      padding: 1.5rem;
      display: flex;
      flex-direction: column;
      gap: 1.25rem;
    }
    .settings-group {
      display: flex;
      flex-direction: column;
      gap: 0.5rem;
    }
    .settings-group label {
      font-size: 0.75rem;
      font-weight: 700;
      text-transform: uppercase;
      letter-spacing: 0.05em;
      color: var(--text-muted);
    }
    .settings-group select {
      width: 100%;
    }
    .theme-options {
      display: flex;
      gap: 0.75rem;
      padding-top: 0.25rem;
    }
    .theme-opt {
      width: 32px;
      height: 32px;
      border-radius: 50%;
      border: 2px solid transparent;
      cursor: pointer;
      position: relative;
      transition: transform 0.15s ease, border-color 0.15s ease;
    }
    .theme-opt:hover {
      transform: scale(1.1);
    }
    .theme-opt.active {
      border-color: var(--text-primary);
      transform: scale(1.1);
    }
    .theme-opt.orange { background: #f97316; }
    .theme-opt.blue { background: #2563eb; }
    .theme-opt.purple { background: #8b5cf6; }
    .theme-opt.green { background: #10b981; }
    
    .modal-footer {
      padding: 1rem 1.5rem;
      border-top: 1px solid var(--border);
      background: rgba(0, 0, 0, 0.1);
      display: flex;
      justify-content: flex-end;
    }
    .settings-btn-save {
      background: var(--accent);
      color: #ffffff;
      border: none;
      padding: 0.5rem 1rem;
      border-radius: var(--radius-md);
      font-weight: 600;
      font-size: 0.8rem;
      cursor: pointer;
      transition: background 0.15s ease;
    }
    .settings-btn-save:hover {
      background: var(--accent-hover);
    }
  </style>
</head>
<body>
  <div class="app">
    <!-- Sidebar -->
    <aside class="sidebar">
      <div class="sidebar-title">
        <h2>Files</h2>
        <button class="icon-btn-circle" id="new-file" title="Create a new file">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><line x1="12" y1="5" x2="12" y2="19"></line><line x1="5" y1="12" x2="19" y2="12"></line></svg>
        </button>
      </div>
      <div id="current-folder">
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path></svg>
        <span style="overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">Choose a save location</span>
      </div>
      <div class="file-list" id="file-list">
        <span class="hint">Loading files...</span>
      </div>
    </aside>

    <!-- Main Workspace -->
    <div class="workspace">
      <header>
        <div class="top-brand">
          <button class="icon-btn-circle" id="toggle-files-btn" title="Toggle Files Panel" style="margin-right: 0.5rem; width: 26px; height: 26px; border: none; background: transparent; color: var(--text-secondary); display: flex; align-items: center; justify-content: center;">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect><line x1="9" y1="3" x2="9" y2="21"></line></svg>
          </button>
          <span class="brand-dot"></span>
          CLAWIE
        </div>
        <div class="plan-pill">Workspace · <strong>Clawie WebUI</strong></div>
        <div class="usage-container" id="usage-container" style="display: flex; align-items: center; gap: 0.75rem; font-size: 0.75rem; border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 0.3rem 0.6rem; background: rgba(255, 255, 255, 0.01);">
          <span style="color: var(--text-muted);">Session Usage:</span>
          <strong id="usage-text" style="color: var(--text-secondary);">0 / 12,000</strong>
          <div class="usage-bar-bg" style="width: 60px; height: 6px; background: rgba(255,255,255,0.08); border-radius: 99px; overflow: hidden; position: relative;">
            <div id="usage-bar-fill" style="width: 0%; height: 100%; background: var(--ok); border-radius: 99px; transition: width 0.3s ease, background 0.3s ease;"></div>
          </div>
          <span style="color: var(--text-muted); border-left: 1px solid var(--border); padding-left: 0.75rem;">Est. Cost:</span>
          <strong id="cost-text" style="color: var(--ok);">$0.0000</strong>
        </div>
        <div style="display: flex; align-items: center; gap: 0.75rem;">
          <div id="status" class="status-pill">Ready</div>
          <button class="icon-btn-circle" id="toggle-folders-btn" title="Toggle Folders Panel">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect><line x1="15" y1="3" x2="15" y2="21"></line></svg>
          </button>
          <button class="icon-btn-circle" id="settings-toggle" title="Settings">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path></svg>
          </button>
        </div>
      </header>
      
      <main>
        <div class="workspace-content-wrap">
          <!-- Left Pane: Chat Panel -->
          <div class="chat-panel">
            <div class="chat-header" style="height: 44px; border-bottom: 1px solid var(--border); background: rgba(0,0,0,0.15); display: flex; align-items: center; justify-content: space-between; padding: 0 1rem; flex: none;">
              <span style="font-size: 0.85rem; font-weight: 600; color: var(--text-secondary); display: flex; align-items: center; gap: 0.5rem;">
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path></svg>
                Chat Assistant
              </span>
              <button id="chat-clear-btn" class="icon-btn-circle" title="Clear Chat" style="width: 24px; height: 24px;">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"></polyline><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"></path></svg>
              </button>
            </div>
            <div class="chat-messages" id="chat-messages">
              <div class="message clawie">
                <span class="message-label">Clawie</span>
                <span>Hello! 👋 How can I assist you today with your software engineering tasks?</span>
              </div>
            </div>
            <div class="chat-input-row" style="flex-direction: column; align-items: stretch; gap: 0.75rem;">
              <div class="suggestions-row" style="display: flex; gap: 0.5rem; overflow-x: auto; padding-bottom: 0.25rem; scrollbar-width: none;">
                <button class="suggestion-chip" onclick="applySuggestion('Explain this code')" style="background: rgba(255,255,255,0.03); border: 1px solid var(--border); border-radius: 99px; color: var(--text-secondary); padding: 0.35rem 0.85rem; font-size: 0.7rem; cursor: pointer; white-space: nowrap; transition: all 0.15s ease; font-weight: 500; display: flex; align-items: center; gap: 0.25rem;">💡 Explain Code</button>
                <button class="suggestion-chip" onclick="applySuggestion('Find bugs in this code')" style="background: rgba(255,255,255,0.03); border: 1px solid var(--border); border-radius: 99px; color: var(--text-secondary); padding: 0.35rem 0.85rem; font-size: 0.7rem; cursor: pointer; white-space: nowrap; transition: all 0.15s ease; font-weight: 500; display: flex; align-items: center; gap: 0.25rem;">🐛 Find Bugs</button>
                <button class="suggestion-chip" onclick="applySuggestion('Write unit tests for this code')" style="background: rgba(255,255,255,0.03); border: 1px solid var(--border); border-radius: 99px; color: var(--text-secondary); padding: 0.35rem 0.85rem; font-size: 0.7rem; cursor: pointer; white-space: nowrap; transition: all 0.15s ease; font-weight: 500; display: flex; align-items: center; gap: 0.25rem;">🧪 Write Tests</button>
                <button class="suggestion-chip" onclick="applySuggestion('Refactor and clean this code')" style="background: rgba(255,255,255,0.03); border: 1px solid var(--border); border-radius: 99px; color: var(--text-secondary); padding: 0.35rem 0.85rem; font-size: 0.7rem; cursor: pointer; white-space: nowrap; transition: all 0.15s ease; font-weight: 500; display: flex; align-items: center; gap: 0.25rem;">⚡ Refactor</button>
              </div>
              <div style="display: flex; align-items: flex-end; gap: 0.75rem; width: 100%;">
                <textarea id="chat-input" rows="1" placeholder="Ask Clawie to create or edit code..."></textarea>
                <button class="icon-btn-circle" id="voice-input-btn" title="Start voice input" aria-label="Start voice input" style="width: 34px; height: 34px; border-radius: 50%; flex: none; display: flex; align-items: center; justify-content: center;">
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z"></path><path d="M19 10v2a7 7 0 0 1-14 0v-2"></path><line x1="12" y1="19" x2="12" y2="23"></line><line x1="8" y1="23" x2="16" y2="23"></line></svg>
                </button>
                <button class="icon-btn-round" id="chat-send" title="Send message" aria-label="Send message">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><line x1="22" y1="2" x2="11" y2="13"></line><polygon points="22 2 15 22 11 13 2 9 22 2"></polygon></svg>
                </button>
              </div>
            </div>
          </div>

          <!-- Right Pane: Code Editor Panel -->
          <div class="editor-panel" id="editor-panel" style="border: 1px solid var(--border); background: var(--bg-card); border-radius: var(--radius-lg); display: flex; flex-direction: column; overflow: hidden; box-shadow: 0 12px 30px rgba(0, 0, 0, 0.25);">
            <div class="editor-header" style="height: 44px; border-bottom: 1px solid var(--border); background: rgba(0,0,0,0.15); display: flex; align-items: center; justify-content: space-between; padding: 0 1rem; flex: none;">
              <span id="editor-filename" style="font-family: var(--font-code); font-size: 0.85rem; color: var(--text-secondary); display: flex; align-items: center; gap: 0.5rem;">
                <span class="brand-dot" style="width: 6px; height: 6px; border-radius: 50%; background: var(--text-muted);"></span>
                No file open
              </span>
              <button id="editor-save-btn" class="accent-btn" style="width: auto; padding: 0.25rem 0.75rem; font-size: 0.75rem; display: none;">Save</button>
            </div>
            
            <div class="editor-content-container" style="flex: 1; position: relative; display: flex; flex-direction: row; background: #050507; overflow: hidden;">
              <div id="editor-placeholder" style="position: absolute; top: 0; left: 0; width: 100%; height: 100%; display: flex; flex-direction: column; align-items: center; justify-content: center; color: var(--text-muted); gap: 0.75rem; background: var(--bg-card); z-index: 5;">
                <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z"></path><polyline points="14 2 14 8 20 8"></polyline><line x1="16" y1="13" x2="8" y2="13"></line><line x1="16" y1="17" x2="8" y2="17"></line><line x1="10" y1="9" x2="8" y2="9"></line></svg>
                <span style="font-size: 0.85rem;">Select a file from the sidebar to view and edit</span>
              </div>
              
              <div id="editor-line-numbers" style="display: none; width: 45px; padding: 1rem 0; text-align: right; color: var(--text-muted); font-family: var(--font-code); font-size: 0.85rem; line-height: 1.5; background: rgba(0,0,0,0.25); border-right: 1px solid var(--border); user-select: none; overflow-y: hidden; box-sizing: border-box; padding-right: 0.75rem;">1</div>
              <textarea id="editor-textarea" style="display: none; flex: 1; height: 100%; background: transparent; border: none; outline: none; resize: none; color: #a9b1d6; font-family: var(--font-code); font-size: 0.85rem; line-height: 1.5; padding: 1rem; box-sizing: border-box; overflow-y: auto; white-space: pre; overflow-wrap: normal;" spellcheck="false"></textarea>
            </div>
          </div>
        </div>
      </main>
    </div>

    <!-- Right Sidebar for Folder Selection -->
    <aside class="right-sidebar">
      <div class="right-sidebar-title">
        <h2>Folders</h2>
      </div>
      
      <div class="field-wrap" style="min-width: 0;">
        <select id="location-preset" size="8" aria-label="Save location preset"></select>
      </div>

      <div class="field-wrap" style="min-width: 0; display: flex; flex-direction: column; gap: 0.5rem;">
        <button id="choose-folder-btn" class="accent-btn">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path></svg>
          Choose Folder...
        </button>
      </div>

      <div class="field-wrap" style="min-width: 0; display: flex; flex-direction: column; gap: 0.25rem;">
        <label for="location-path" style="font-size: 0.65rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-muted);">Folder Path</label>
        <input id="location-path" placeholder="/absolute/path/to/folder" autocomplete="off">
      </div>
      <div class="hint" style="font-size: 0.7rem; padding: 0.5rem 0; margin-top: auto;">Double-click a folder preset to load its files.</div>
    </aside>
  </div>

  <div id="drop-overlay" style="display: none; position: fixed; top: 0; left: 0; width: 100vw; height: 100vh; background: rgba(var(--accent-rgb), 0.15); backdrop-filter: blur(8px); border: 4px dashed var(--accent); z-index: 10000; align-items: center; justify-content: center; pointer-events: none; transition: all 0.2s ease;">
    <div style="text-align: center; color: var(--text-primary); background: var(--bg-card); padding: 2rem; border-radius: var(--radius-lg); border: 1px solid var(--border); box-shadow: 0 20px 40px rgba(0,0,0,0.5);">
      <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="margin-bottom: 1rem;"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path><polyline points="17 8 12 3 7 8"></polyline><line x1="12" y1="3" x2="12" y2="15"></line></svg>
      <h3>Drop files to add to workspace</h3>
      <p style="font-size: 0.8rem; color: var(--text-muted); margin-top: 0.5rem;">They will be saved to the selected folder</p>
    </div>
  </div>

  <!-- Settings Modal Overlay -->
  <div id="settings-modal" class="modal-overlay" hidden>
    <div class="modal-content">
      <div class="modal-header">
        <h3>Settings</h3>
        <button id="settings-close" class="close-btn">&times;</button>
      </div>
      <div class="modal-body">
        <div class="settings-group">
          <label>Theme Accent</label>
          <div class="theme-options">
            <button class="theme-opt orange active" data-color="orange" title="Orange"></button>
            <button class="theme-opt blue" data-color="blue" title="Blue"></button>
            <button class="theme-opt purple" data-color="purple" title="Purple"></button>
            <button class="theme-opt green" data-color="green" title="Green"></button>
          </div>
        </div>
        <div class="settings-group">
          <label for="settings-model">Active AI Model</label>
          <select id="settings-model">
            <option value="gpt-4.1">gpt-4.1 (Default)</option>
            <option value="claude-3-5-sonnet">claude-3-5-sonnet</option>
            <option value="gpt-4o">gpt-4o</option>
            <option value="gemini-1.5-pro">gemini-1.5-pro</option>
          </select>
        </div>
        <div class="settings-group" style="border-top: 1px solid var(--border); padding-top: 1rem;">
          <label style="margin-bottom: 0.25rem;">Connections</label>
          <div style="display: flex; flex-direction: column; gap: 0.75rem; margin-top: 0.25rem;">
            <div style="display: flex; flex-direction: column; gap: 0.25rem;">
              <label for="settings-openai-key" style="font-size: 0.65rem; color: var(--text-muted);">OpenAI API Key</label>
              <input id="settings-openai-key" type="password" placeholder="sk-..." autocomplete="off">
            </div>
            <div style="display: flex; flex-direction: column; gap: 0.25rem;">
              <label for="settings-anthropic-key" style="font-size: 0.65rem; color: var(--text-muted);">Anthropic API Key</label>
              <input id="settings-anthropic-key" type="password" placeholder="sk-ant-..." autocomplete="off">
            </div>
            <div style="display: flex; flex-direction: column; gap: 0.25rem;">
              <label for="settings-openai-url" style="font-size: 0.65rem; color: var(--text-muted);">Custom OpenAI Base URL (optional)</label>
              <input id="settings-openai-url" placeholder="https://api.openai.com/v1" autocomplete="off">
            </div>
          </div>
        </div>
      </div>
      <div class="modal-footer">
        <button id="settings-save-btn" class="settings-btn-save">Apply Settings</button>
      </div>
    </div>
  </div>

  <!-- New File Modal Overlay -->
  <div id="new-file-modal" class="modal-overlay" hidden>
    <div class="modal-content" style="width: min(350px, 90%);">
      <div class="modal-header">
        <h3>Create New File</h3>
        <button id="new-file-close" class="close-btn">&times;</button>
      </div>
      <div class="modal-body" style="padding: 1.25rem;">
        <div class="settings-group">
          <label for="new-filename-input">Filename</label>
          <input id="new-filename-input" placeholder="e.g. index.html, main.py" autocomplete="off" style="width: 100%;">
        </div>
      </div>
      <div class="modal-footer" style="padding: 0.75rem 1.25rem;">
        <button id="new-file-create-btn" class="settings-btn-save">Create File</button>
      </div>
    </div>
  </div>

  <script>
    const status = document.querySelector('#status');
    const fileList = document.querySelector('#file-list');
    const locationPreset = document.querySelector('#location-preset');
    const locationPath = document.querySelector('#location-path');
    const currentFolder = document.querySelector('#current-folder').querySelector('span');
    const chatMessages = document.querySelector('#chat-messages');
    const chatInput = document.querySelector('#chat-input');
    const chatSend = document.querySelector('#chat-send');

    let activeFileName = null;

    let totalInputTokens = 0;
    let totalOutputTokens = 0;
    let totalCost = 0.0;
    const maxTokensLimit = 12000;

    function setStatus(message, state = '') {
      status.textContent = message;
      status.className = 'status-pill' + (state ? ' ' + state : '');
    }

    function updateUsageDisplay(inputTokens = 0, outputTokens = 0, cost = 0.0) {
      totalInputTokens += inputTokens;
      totalOutputTokens += outputTokens;
      totalCost += cost;
      const totalUsed = totalInputTokens + totalOutputTokens;
      
      const usageText = document.querySelector('#usage-text');
      const usageBarFill = document.querySelector('#usage-bar-fill');
      const costText = document.querySelector('#cost-text');
      
      if (usageText && usageBarFill) {
        usageText.textContent = `${totalUsed.toLocaleString()} / ${maxTokensLimit.toLocaleString()}`;
        
        const pct = Math.min((totalUsed / maxTokensLimit) * 100, 100);
        usageBarFill.style.width = `${pct}%`;
        
        if (pct < 50) {
          usageBarFill.style.background = 'var(--ok)';
        } else if (pct < 80) {
          usageBarFill.style.background = 'var(--warn)';
        } else {
          usageBarFill.style.background = 'var(--accent)';
        }
      }

      if (costText) {
        costText.textContent = `$${totalCost.toFixed(4)}`;
      }
    }

    function formatMarkdown(text) {
      if (!text) return '';
      let html = text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
      html = html.replace(/```([\s\S]*?)```/g, (match, codeBlock) => {
        const lines = codeBlock.split('\n');
        let lang = 'code', code = codeBlock;
        if (lines[0] && lines[0].trim().length < 15 && !lines[0].includes(' ') && !lines[0].includes('\n')) {
          lang = lines[0].trim();
          code = lines.slice(1).join('\n');
        }
        const cleanCode = code.trim();
        const escapedCode = btoa(unescape(encodeURIComponent(cleanCode)));
        
        return `<pre class="code-block">
          <div class="code-header" style="display: flex; justify-content: space-between; align-items: center;">
            <span>${lang}</span>
            <div style="display: flex; gap: 0.5rem;">
              <button class="code-action-btn copy-btn" onclick="copyCodeBlock(this, '${escapedCode}')" style="background: transparent; border: none; color: var(--text-muted); font-size: 0.65rem; cursor: pointer; display: flex; align-items: center; gap: 0.25rem;">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg>
                Copy
              </button>
              <button class="code-action-btn apply-btn" onclick="applyToEditor(this, '${escapedCode}')" style="background: transparent; border: none; color: var(--text-muted); font-size: 0.65rem; cursor: pointer; display: flex; align-items: center; gap: 0.25rem;">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><polyline points="14 2 14 8 20 8"></polyline><line x1="16" y1="13" x2="8" y2="13"></line><line x1="16" y1="17" x2="8" y2="17"></line><line x1="10" y1="9" x2="8" y2="9"></line></svg>
                Open in Editor
              </button>
            </div>
          </div>
          <code>${cleanCode}</code>
        </pre>`;
      });
      html = html.replace(/`([^`]+)`/g, '<code>$1</code>');
      html = html.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
      html = html.replace(/\n/g, '<br>');
      return html;
    }

    function appendChatMessage(role, text) {
      const message = document.createElement('div');
      message.className = 'message ' + role;
      const label = document.createElement('span');
      label.className = 'message-label';
      label.textContent = role === 'user' ? 'You' : 'Clawie';
      message.append(label);
      const content = document.createElement('span');
      content.innerHTML = formatMarkdown(text);
      message.append(content);
      chatMessages.append(message);
      chatMessages.scrollTop = chatMessages.scrollHeight;
      return message;
    }

    async function sendChatMessage() {
      const text = chatInput.value.trim();
      if (!text) return;
      appendChatMessage('user', text);
      chatInput.value = '';
      chatInput.style.height = 'auto';
      chatSend.disabled = true;
      setStatus('Clawie is thinking...');
      const pending = appendChatMessage('clawie', 'Thinking...');
      try {
        const response = await fetch('/chat', {
          method: 'POST',
          headers: {'Content-Type': 'application/json'},
          body: JSON.stringify({
            message: text,
            model: selectedModel,
            openai_api_key: localStorage.getItem('clawie-openai-key') || '',
            anthropic_api_key: localStorage.getItem('clawie-anthropic-key') || '',
            openai_base_url: localStorage.getItem('clawie-openai-url') || ''
          })
        });
        const result = await response.json();
        if (!response.ok || !result.ok) throw new Error(result.error || 'Chat failed');
        pending.replaceChildren();
        const label = document.createElement('span');
        label.className = 'message-label';
        label.textContent = 'Clawie';
        pending.append(label);
        const content = document.createElement('span');
        content.innerHTML = formatMarkdown(result.reply);
        pending.append(content);
        
        let costVal = 0.0;
        if (result.estimated_cost) {
          costVal = parseFloat(result.estimated_cost.replace('$', '')) || 0.0;
        }
        updateUsageDisplay(result.input_tokens || 0, result.output_tokens || 0, costVal);
        setStatus('Clawie replied', 'saved');
      } catch (error) {
        pending.replaceChildren();
        const label = document.createElement('span');
        label.className = 'message-label';
        label.textContent = 'Clawie';
        pending.append(label);
        const content = document.createElement('span');
        content.innerHTML = formatMarkdown(error.message);
        
        // If it is an API key error (e.g. 401, unauthorized, incorrect api key, dummy etc.), append settings helper
        const isApiKeyError = /401|unauthorized|api key|incorrect api key|invalid_request_error|dummy/i.test(error.message);
        if (isApiKeyError) {
          const helperDiv = document.createElement('div');
          helperDiv.style.marginTop = '1rem';
          helperDiv.style.padding = '0.75rem';
          helperDiv.style.background = 'rgba(245, 158, 11, 0.1)';
          helperDiv.style.border = '1px solid rgba(245, 158, 11, 0.2)';
          helperDiv.style.borderRadius = 'var(--radius-sm)';
          
          const textSpan = document.createElement('span');
          textSpan.style.display = 'block';
          textSpan.style.marginBottom = '0.5rem';
          textSpan.style.fontSize = '0.85rem';
          textSpan.style.color = 'var(--text-secondary)';
          textSpan.textContent = 'It looks like the API key is invalid or not configured. You can set a valid API key in settings without restarting Clawie.';
          helperDiv.appendChild(textSpan);
          
          const settingsBtn = document.createElement('button');
          settingsBtn.textContent = '⚙️ Open Settings';
          settingsBtn.style.background = 'var(--accent)';
          settingsBtn.style.color = 'var(--text-primary)';
          settingsBtn.style.border = 'none';
          settingsBtn.style.padding = '0.4rem 0.8rem';
          settingsBtn.style.borderRadius = 'var(--radius-sm)';
          settingsBtn.style.cursor = 'pointer';
          settingsBtn.style.fontSize = '0.85rem';
          settingsBtn.style.fontWeight = '500';
          settingsBtn.style.transition = 'background 0.2s';
          
          settingsBtn.addEventListener('mouseenter', () => {
            settingsBtn.style.background = 'var(--accent-hover)';
          });
          settingsBtn.addEventListener('mouseleave', () => {
            settingsBtn.style.background = 'var(--accent)';
          });
          settingsBtn.addEventListener('click', () => {
            document.querySelector('#settings-toggle').click();
          });
          helperDiv.appendChild(settingsBtn);
          content.appendChild(helperDiv);
        }
        
        pending.append(content);
        setStatus(error.message);
      } finally {
        chatSend.disabled = false;
        chatInput.focus();
        chatMessages.scrollTop = chatMessages.scrollHeight;
      }
    }

    chatInput.addEventListener('input', () => {
      chatInput.style.height = 'auto';
      chatInput.style.height = Math.min(chatInput.scrollHeight, 150) + 'px';
    });
    chatInput.addEventListener('keydown', e => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sendChatMessage(); } });

    document.querySelector('#chat-clear-btn').addEventListener('click', () => {
      chatMessages.innerHTML = `
        <div class="message clawie">
          <span class="message-label">Clawie</span>
          <span>Chat cleared. How else can I help you?</span>
        </div>
      `;
      setStatus('Chat history cleared', 'saved');
    });

    window.copyCodeBlock = function(btn, encodedCode) {
      try {
        const code = decodeURIComponent(escape(atob(encodedCode)));
        navigator.clipboard.writeText(code);
        const originalHtml = btn.innerHTML;
        btn.innerHTML = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="var(--ok)" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg> <span style="color: var(--ok)">Copied!</span>`;
        setTimeout(() => { btn.innerHTML = originalHtml; }, 2000);
      } catch (e) {
        setStatus('Copy failed: ' + e.message);
      }
    };

    window.applyToEditor = function(btn, encodedCode) {
      try {
        const code = decodeURIComponent(escape(atob(encodedCode)));
        const textarea = document.querySelector('#editor-textarea');
        const placeholder = document.querySelector('#editor-placeholder');
        const lineNumbers = document.querySelector('#editor-line-numbers');
        
        let targetFile = activeFileName;
        if (!targetFile) {
          const filename = prompt('Enter a filename to load this code into (e.g. main.py):');
          if (!filename || !filename.trim()) return;
          targetFile = filename.trim();
          activeFileName = targetFile;
          
          document.querySelector('#editor-filename').innerHTML = `
            <span class="brand-dot" style="width: 6px; height: 6px; border-radius: 50%; background: var(--ok); box-shadow: 0 0 6px var(--ok);"></span>
            ${targetFile}
          `;
        }
        
        textarea.value = code;
        textarea.style.display = 'block';
        lineNumbers.style.display = 'block';
        placeholder.style.display = 'none';
        document.querySelector('#editor-save-btn').style.display = 'block';
        
        updateLineNumbers();
        setStatus('Loaded code into ' + targetFile + ' (unsaved)', 'unsaved');
      } catch (e) {
        setStatus('Open in editor failed: ' + e.message);
      }
    };

    window.applySuggestion = function(text) {
      if (activeFileName) {
        chatInput.value = text + ' in ' + activeFileName;
      } else {
        chatInput.value = text;
      }
      chatInput.focus();
      chatInput.style.height = 'auto';
      chatInput.style.height = Math.min(chatInput.scrollHeight, 150) + 'px';
    };

    function getFileIcon(filename) {
      const ext = filename.split('.').pop().toLowerCase();
      switch(ext) {
        case 'rs':
          return `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="#f97316" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="flex: none;"><path d="M12 2L2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5"/></svg>`;
        case 'py':
          return `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="#e5c07b" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="flex: none;"><path d="M12 2v20M17 5H9.5a3.5 3.5 0 0 0 0 7h5a3.5 3.5 0 0 1 0 7H6"/></svg>`;
        case 'js':
        case 'ts':
        case 'jsx':
        case 'tsx':
          return `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="#61afef" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="flex: none;"><circle cx="12" cy="12" r="10"></circle><path d="M8 12h8M12 8v8"/></svg>`;
        case 'html':
        case 'xml':
          return `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="#98c379" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="flex: none;"><polyline points="16 18 22 12 16 6"></polyline><polyline points="8 6 2 12 8 18"></polyline></svg>`;
        case 'css':
          return `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="#c678dd" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="flex: none;"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"></path></svg>`;
        case 'json':
        case 'toml':
        case 'yaml':
        case 'yml':
          return `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="#56b6c2" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="flex: none;"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><polyline points="14 2 14 8 20 8"></polyline><line x1="16" y1="13" x2="8" y2="13"></line><line x1="16" y1="17" x2="8" y2="17"></line><line x1="10" y1="9" x2="8" y2="9"></line></svg>`;
        case 'md':
          return `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="#abb2bf" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="flex: none;"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"></path><path d="M18.5 2.5a2.121 2.121 0 1 1 3 3L12 15l-4 1 1-4 9.5-9.5z"></path></svg>`;
        default:
          return `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="flex: none;"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><polyline points="14 2 14 8 20 8"></polyline></svg>`;
      }
    }

    async function refreshFiles() {
      try {
        const response = await fetch('/files', { method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify({directory: locationPath.value}) });
        const result = await response.json();
        if (!response.ok || !result.ok) throw new Error(result.error || 'Could not load files');
        locationPath.value = result.directory;
        currentFolder.textContent = result.directory;
        fileList.replaceChildren();
        result.files.forEach(name => {
          const item = document.createElement('button');
          item.className = 'file';
          if (activeFileName === name) item.classList.add('active');
          item.innerHTML = getFileIcon(name) + `<span style="overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${name}</span>`;
          item.addEventListener('click', () => loadFile(name, item));
          fileList.append(item);
        });
      } catch (_) { fileList.textContent = 'Could not load files.'; }
    }

    const textarea = document.querySelector('#editor-textarea');
    const lineNumbers = document.querySelector('#editor-line-numbers');

    function updateLineNumbers() {
      const lines = textarea.value.split('\n');
      const count = Math.max(lines.length, 1);
      let numbersHtml = '';
      for (let i = 1; i <= count; i++) {
        numbersHtml += i + '<br>';
      }
      lineNumbers.innerHTML = numbersHtml;
    }

    textarea.addEventListener('input', updateLineNumbers);
    textarea.addEventListener('scroll', () => {
      lineNumbers.scrollTop = textarea.scrollTop;
    });

    async function loadFile(name, item) {
      setStatus('Opening ' + name + '...');
      try {
        const response = await fetch('/load', { method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify({directory: locationPath.value, filename: name}) });
        const result = await response.json();
        if (!response.ok || !result.ok) throw new Error(result.error || 'Open failed');
        
        activeFileName = name;
        document.querySelector('#editor-filename').innerHTML = `
          <span class="brand-dot" style="width: 6px; height: 6px; border-radius: 50%; background: var(--ok); box-shadow: 0 0 6px var(--ok);"></span>
          ${name}
        `;
        document.querySelector('#editor-placeholder').style.display = 'none';
        
        textarea.value = result.code;
        textarea.style.display = 'block';
        lineNumbers.style.display = 'block';
        document.querySelector('#editor-save-btn').style.display = 'block';

        document.querySelectorAll('.file').forEach(f => f.classList.remove('active'));
        item.classList.add('active');
        updateLineNumbers();
        setStatus('Opened ' + name, 'saved');
      } catch (error) { setStatus(error.message); }
    }

    async function saveCurrentFile() {
      if (!activeFileName) return;
      const saveBtn = document.querySelector('#editor-save-btn');
      saveBtn.disabled = true;
      setStatus('Saving ' + activeFileName + '...');
      try {
        const response = await fetch('/save', {
          method: 'POST',
          headers: {'Content-Type': 'application/json'},
          body: JSON.stringify({
            directory: locationPath.value,
            filename: activeFileName,
            code: textarea.value,
            improvements: 'Manually edited in Code Viewer'
          })
        });
        const result = await response.json();
        if (!response.ok || !result.ok) throw new Error(result.error || 'Save failed');
        setStatus('Saved ' + activeFileName, 'saved');
      } catch (error) {
        setStatus(error.message);
      } finally {
        saveBtn.disabled = false;
      }
    }

    chatSend.addEventListener('click', sendChatMessage);
    document.querySelector('#editor-save-btn').addEventListener('click', saveCurrentFile);
    locationPreset.addEventListener('change', () => { if (locationPreset.value) locationPath.value = locationPreset.value; });
    locationPreset.addEventListener('dblclick', () => { if (locationPreset.value) { locationPath.value = locationPreset.value; refreshFiles(); } });
    locationPath.addEventListener('change', refreshFiles);

    const chooseFolderBtn = document.querySelector('#choose-folder-btn');
    chooseFolderBtn.addEventListener('click', async () => {
      setStatus('Choosing folder...');
      try {
        const response = await fetch('/select-directory', {
          method: 'POST',
          headers: {'Content-Type': 'application/json'}
        });
        const result = await response.json();
        if (!response.ok || !result.ok) throw new Error(result.error || 'Folder selection failed');
        if (result.directory) {
          locationPath.value = result.directory;
          localStorage.setItem('clawie-location', result.directory);
          await refreshFiles();
          setStatus('Folder updated: ' + result.directory, 'saved');
        } else {
          setStatus('Folder selection cancelled');
        }
      } catch (error) {
        setStatus('Failed to choose folder. Please enter the absolute path directly in the field below.', 'unsaved');
      }
    });

    const settingsToggle = document.querySelector('#settings-toggle');
    const settingsModal = document.querySelector('#settings-modal');
    const settingsClose = document.querySelector('#settings-close');
    const settingsSaveBtn = document.querySelector('#settings-save-btn');
    const settingsModel = document.querySelector('#settings-model');
    const settingsOpenAiKey = document.querySelector('#settings-openai-key');
    const settingsAnthropicKey = document.querySelector('#settings-anthropic-key');
    const settingsOpenAiUrl = document.querySelector('#settings-openai-url');

    const themes = {
      orange: { rgb: '249, 115, 22', hover: '#ea580c' },
      blue: { rgb: '37, 99, 235', hover: '#1d4ed8' },
      purple: { rgb: '139, 92, 246', hover: '#7c3aed' },
      green: { rgb: '16, 185, 129', hover: '#059669' }
    };

    let selectedTheme = localStorage.getItem('clawie-theme') || 'orange';
    let selectedModel = localStorage.getItem('clawie-model-setting') || 'gpt-4.1';

    function applyTheme(colorName) {
      const theme = themes[colorName] || themes.orange;
      document.documentElement.style.setProperty('--accent-rgb', theme.rgb);
      document.documentElement.style.setProperty('--accent-hover', theme.hover);
      localStorage.setItem('clawie-theme', colorName);
      selectedTheme = colorName;
      document.querySelectorAll('.theme-opt').forEach(opt => {
        opt.classList.toggle('active', opt.dataset.color === colorName);
      });
    }

    settingsToggle.addEventListener('click', () => {
      settingsModel.value = selectedModel;
      settingsOpenAiKey.value = localStorage.getItem('clawie-openai-key') || '';
      settingsAnthropicKey.value = localStorage.getItem('clawie-anthropic-key') || '';
      settingsOpenAiUrl.value = localStorage.getItem('clawie-openai-url') || '';
      applyTheme(selectedTheme);
      settingsModal.hidden = false;
    });

    settingsClose.addEventListener('click', () => {
      settingsModal.hidden = true;
    });

    window.addEventListener('click', event => {
      if (event.target === settingsModal) {
        settingsModal.hidden = true;
      }
    });

    document.querySelectorAll('.theme-opt').forEach(opt => {
      opt.addEventListener('click', () => {
        applyTheme(opt.dataset.color);
      });
    });

    settingsSaveBtn.addEventListener('click', () => {
      selectedModel = settingsModel.value;
      localStorage.setItem('clawie-model-setting', selectedModel);
      localStorage.setItem('clawie-openai-key', settingsOpenAiKey.value.trim());
      localStorage.setItem('clawie-anthropic-key', settingsAnthropicKey.value.trim());
      localStorage.setItem('clawie-openai-url', settingsOpenAiUrl.value.trim());
      settingsModal.hidden = true;
      setStatus('Settings applied successfully', 'saved');
    });

    const newFileModal = document.querySelector('#new-file-modal');
    const newFileClose = document.querySelector('#new-file-close');
    const newFilenameInput = document.querySelector('#new-filename-input');
    const newFileCreateBtn = document.querySelector('#new-file-create-btn');

    document.querySelector('#new-file').addEventListener('click', () => {
      newFilenameInput.value = '';
      newFileModal.hidden = false;
      newFilenameInput.focus();
    });

    newFileClose.addEventListener('click', () => {
      newFileModal.hidden = true;
    });

    window.addEventListener('click', event => {
      if (event.target === newFileModal) {
        newFileModal.hidden = true;
      }
    });

    newFilenameInput.addEventListener('keydown', e => {
      if (e.key === 'Enter') {
        newFileCreateBtn.click();
      }
    });

    newFileCreateBtn.addEventListener('click', async () => {
      const filename = newFilenameInput.value.trim();
      if (!filename) return;
      newFileModal.hidden = true;
      try {
        setStatus('Creating ' + filename + '...');
        const response = await fetch('/save', {
          method: 'POST',
          headers: {'Content-Type': 'application/json'},
          body: JSON.stringify({
            directory: locationPath.value,
            filename: filename,
            code: '',
            improvements: 'Created empty file'
          })
        });
        const result = await response.json();
        if (!response.ok || !result.ok) throw new Error(result.error || 'Create failed');
        await refreshFiles();
        
        const fileButtons = document.querySelectorAll('.file');
        for (const btn of fileButtons) {
          const span = btn.querySelector('span');
          const name = span ? span.textContent : btn.textContent;
          if (name === filename) {
            btn.click();
            break;
          }
        }
        setStatus('Created ' + filename, 'saved');
      } catch (error) {
        setStatus(error.message);
      }
    });

    applyTheme(selectedTheme);
    updateUsageDisplay(0, 0);

    async function initializeLocations() {
      try {
        const response = await fetch('/locations');
        const result = await response.json();
        result.locations.forEach(loc => {
          const opt = document.createElement('option');
          opt.value = loc.path;
          opt.textContent = loc.label;
          locationPreset.append(opt);
        });
        locationPath.value = localStorage.getItem('clawie-location') || result.locations[0].path;
        await refreshFiles();
      } catch (error) { setStatus(error.message); }
    }
    initializeLocations();

    const dropOverlay = document.querySelector('#drop-overlay');
    window.addEventListener('dragenter', e => {
      e.preventDefault();
      dropOverlay.style.display = 'flex';
    });
    window.addEventListener('dragover', e => {
      e.preventDefault();
      dropOverlay.style.display = 'flex';
    });
    window.addEventListener('dragleave', e => {
      e.preventDefault();
      if (e.relatedTarget === null || e.clientX <= 0 || e.clientY <= 0 || e.clientX >= window.innerWidth || e.clientY >= window.innerHeight) {
        dropOverlay.style.display = 'none';
      }
    });
    window.addEventListener('drop', async e => {
      e.preventDefault();
      dropOverlay.style.display = 'none';
      
      const files = e.dataTransfer.files;
      if (!files || files.length === 0) return;
      
      setStatus(`Uploading ${files.length} file(s)...`);
      
      for (let i = 0; i < files.length; i++) {
        const file = files[i];
        try {
          const content = await readFileAsText(file);
          const response = await fetch('/upload', {
            method: 'POST',
            headers: {'Content-Type': 'application/json'},
            body: JSON.stringify({
              directory: locationPath.value,
              filename: file.name,
              content: content
            })
          });
          const result = await response.json();
          if (!response.ok || !result.ok) throw new Error(result.error || 'Upload failed');
          
          appendChatMessage('clawie', `Added file **${file.name}** to workspace.`);
        } catch (error) {
          appendChatMessage('clawie', `Failed to upload **${file.name}**: ${error.message}`);
        }
      }
      
      await refreshFiles();
      setStatus('Files uploaded successfully', 'saved');
    });

    function readFileAsText(file) {
      return new Promise((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve(reader.result);
        reader.onerror = () => reject(reader.error);
        reader.readAsText(file);
      });
    }

    // Speech Recognition Setup
    const voiceInputBtn = document.querySelector('#voice-input-btn');
    const SpeechRecognition = window.SpeechRecognition || window.webkitSpeechRecognition;

    if (!SpeechRecognition) {
      voiceInputBtn.style.display = 'none';
    } else {
      const recognition = new SpeechRecognition();
      recognition.continuous = true;
      recognition.interimResults = true;
      recognition.lang = 'en-US';
      
      let isListening = false;
      let initialText = '';

      function startListening() {
        try {
          initialText = chatInput.value.trim();
          recognition.start();
          isListening = true;
          voiceInputBtn.classList.add('listening-active');
          voiceInputBtn.title = 'Stop voice input';
          setStatus('Listening for speech...');
        } catch (e) {
          setStatus('Failed to start speech recognition: ' + e.message);
        }
      }

      function stopListening() {
        if (!isListening) return;
        try {
          recognition.stop();
        } catch (_) {}
        isListening = false;
        voiceInputBtn.classList.remove('listening-active');
        voiceInputBtn.title = 'Start voice input';
        setStatus('Stopped listening', 'saved');
      }

      voiceInputBtn.addEventListener('click', () => {
        if (isListening) {
          stopListening();
        } else {
          startListening();
        }
      });

      recognition.onresult = event => {
        let finalTranscript = '';
        let interimTranscript = '';
        for (let i = 0; i < event.results.length; ++i) {
          if (event.results[i].isFinal) {
            finalTranscript += event.results[i][0].transcript + ' ';
          } else {
            interimTranscript += event.results[i][0].transcript;
          }
        }
        const fullTranscript = (finalTranscript + interimTranscript).trim();
        chatInput.value = (initialText ? initialText + ' ' : '') + fullTranscript;
        
        chatInput.focus();
        chatInput.style.height = 'auto';
        chatInput.style.height = Math.min(chatInput.scrollHeight, 150) + 'px';
      };

      recognition.onerror = event => {
        if (event.error === 'no-speech' || event.error === 'aborted') {
          return;
        }
        setStatus('Speech error: ' + event.error);
        stopListening();
      };

      recognition.onend = () => {
        stopListening();
      };
    }

    // Panels Collapsible Setup
    const toggleFilesBtn = document.querySelector('#toggle-files-btn');
    const toggleFoldersBtn = document.querySelector('#toggle-folders-btn');
    const sidebar = document.querySelector('.sidebar');
    const rightSidebar = document.querySelector('.right-sidebar');

    toggleFilesBtn.addEventListener('click', () => {
      sidebar.classList.toggle('collapsed');
      const collapsed = sidebar.classList.contains('collapsed');
      localStorage.setItem('clawie-files-collapsed', collapsed);
    });

    toggleFoldersBtn.addEventListener('click', () => {
      rightSidebar.classList.toggle('collapsed');
      const collapsed = rightSidebar.classList.contains('collapsed');
      localStorage.setItem('clawie-folders-collapsed', collapsed);
    });

    if (localStorage.getItem('clawie-files-collapsed') === 'true') {
      sidebar.classList.add('collapsed');
    }
    if (localStorage.getItem('clawie-folders-collapsed') === 'true') {
      rightSidebar.classList.add('collapsed');
    }
  </script>
</body>
</html>"##;

#[cfg(test)]
mod tests {
    use super::{
        list_code_files, load_workspace_files, resolve_output_directory, safe_filename,
        save_workspace_files, SaveRequest,
    };
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn rejects_paths_outside_the_documents_workspace() {
        assert!(safe_filename("../secret.txt").is_err());
        assert!(safe_filename("folder/code.rs").is_err());
        assert!(safe_filename("code.rs").is_ok());
    }

    #[test]
    fn saves_code_and_improvements_as_separate_files() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("clawie-webui-{nonce}"));
        let payload = SaveRequest {
            directory: root.display().to_string(),
            filename: "demo.rs".to_string(),
            code: "fn main() {}\n".to_string(),
            improvements: "Add a test.".to_string(),
        };

        let (code_path, notes_path) =
            save_workspace_files(&root, &payload).expect("save workspace files");

        assert_eq!(
            fs::read_to_string(code_path).expect("read code"),
            payload.code
        );
        assert!(fs::read_to_string(notes_path)
            .expect("read notes")
            .contains("Add a test."));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn lists_and_reopens_created_code_files() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("clawie-webui-list-{nonce}"));
        let payload = SaveRequest {
            directory: root.display().to_string(),
            filename: "main.py".to_string(),
            code: "print('created')\n".to_string(),
            improvements: "Add argument parsing.".to_string(),
        };
        save_workspace_files(&root, &payload).expect("save workspace files");

        assert_eq!(list_code_files(&root).expect("list files"), vec!["main.py"]);
        let (code, improvements) =
            load_workspace_files(&root, "main.py").expect("load workspace file");
        assert_eq!(code, payload.code);
        assert_eq!(improvements, payload.improvements);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn accepts_absolute_save_locations_and_rejects_relative_ones() {
        let default = std::env::temp_dir().join("clawie-default");
        let custom = std::env::temp_dir().join("clawie-custom");

        assert_eq!(
            resolve_output_directory(&default, "").expect("default directory"),
            default
        );
        assert_eq!(
            resolve_output_directory(&default, custom.to_str().expect("custom path"))
                .expect("absolute directory"),
            custom
        );
        assert!(resolve_output_directory(&default, "relative/folder").is_err());
    }
}
