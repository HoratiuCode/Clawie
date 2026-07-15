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
    gemini_api_key: Option<String>,
    xai_api_key: Option<String>,
    kimi_api_key: Option<String>,
    openai_base_url: Option<String>,
    gemini_base_url: Option<String>,
    xai_base_url: Option<String>,
    kimi_base_url: Option<String>,
    lean_mode: Option<String>,
    max_turns: Option<u32>,
    token_budget: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct TestConnectionRequest {
    provider: String,
    api_key: String,
    base_url: Option<String>,
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UploadRequest {
    directory: String,
    filename: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct InstanceActionRequest {
    pid: u32,
    action: String,
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
            let mut bound_listener = None;
            for p in 4242..=4250 {
                if let Ok(l) = TcpListener::bind(("127.0.0.1", p)) {
                    bound_listener = Some(l);
                    break;
                }
            }
            let listener = bound_listener.unwrap_or_else(|| {
                TcpListener::bind(("127.0.0.1", 0)).expect("failed to bind any port")
            });
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
        line if line.starts_with("GET /manifest.webmanifest ") => write_response(
            stream,
            "200 OK",
            "application/manifest+json",
            WEB_APP_MANIFEST,
        ),
        line if line.starts_with("GET /service-worker.js ") => write_response(
            stream,
            "200 OK",
            "application/javascript; charset=utf-8",
            SERVICE_WORKER_JS,
        ),
        line if line.starts_with("GET /icon.svg ") => {
            write_response(stream, "200 OK", "image/svg+xml", WEB_APP_ICON_SVG)
        }
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
        line if line.starts_with("GET /instances ") => {
            let instances = running_clawie_instances()?;
            write_json_response(
                stream,
                "200 OK",
                &json!({"ok": true, "instances": instances}).to_string(),
            )
        }
        line if line.starts_with("GET /instance-log?") => {
            let pid = query_value(request_line, "pid")
                .and_then(|value| value.parse::<u32>().ok())
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing pid"))?;
            let log = instance_log(pid)?;
            write_json_response(
                stream,
                "200 OK",
                &json!({"ok": true, "log": log}).to_string(),
            )
        }
        line if line.starts_with("GET /ws-log?") || line.starts_with("GET /ws-log ") => {
            let mut ws_key = None;
            for header_line in headers.lines() {
                let parts: Vec<&str> = header_line.splitn(2, ':').collect();
                if parts.len() == 2 && parts[0].trim().to_ascii_lowercase() == "sec-websocket-key" {
                    ws_key = Some(parts[1].trim().to_string());
                    break;
                }
            }

            if let Some(key) = ws_key {
                let magic = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
                let mut input = key.as_bytes().to_vec();
                input.extend_from_slice(magic);
                let hash = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, &input);

                use base64::Engine;
                let accept = base64::prelude::BASE64_STANDARD.encode(hash.as_ref());

                let response = format!(
                    "HTTP/1.1 101 Switching Protocols\r\n\
                     Upgrade: websocket\r\n\
                     Connection: Upgrade\r\n\
                     Sec-WebSocket-Accept: {}\r\n\r\n",
                    accept
                );
                stream.write_all(response.as_bytes())?;

                let pid = query_value(request_line, "pid")
                    .and_then(|value| value.parse::<u32>().ok())
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing pid"))?;

                stream.set_nonblocking(true)?;
                let mut last_len = 0;
                loop {
                    let mut buf = [0u8; 1];
                    match stream.peek(&mut buf) {
                        Ok(0) => break,
                        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {}
                        Err(_) => break,
                        Ok(_) => {
                            let _ = stream.read(&mut buf);
                        }
                    }

                    if let Ok(log) = instance_log(pid) {
                        let events = log["events"].as_array().cloned().unwrap_or_default();
                        if events.len() > last_len {
                            for event in &events[last_len..] {
                                if let Some(event_str) = event.as_str() {
                                    let frame = make_ws_text_frame(event_str);
                                    stream.set_nonblocking(false)?;
                                    if stream.write_all(&frame).is_err() {
                                        return Ok(());
                                    }
                                    stream.set_nonblocking(true)?;
                                }
                            }
                            last_len = events.len();
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(300));
                }
                return Ok(());
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Missing Sec-WebSocket-Key header",
                ));
            }
        }
        line if line.starts_with("POST /instance-action ") => {
            let payload: InstanceActionRequest =
                parse_json_body(&request, header_end, "instance action")?;
            run_instance_action(&payload)?;
            write_json_response(
                stream,
                "200 OK",
                &json!({"ok": true, "pid": payload.pid, "action": payload.action}).to_string(),
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
        line if line.starts_with("GET /api/settings ") => {
            let config_home = runtime::default_config_home();
            let settings_path = config_home.join("settings.json");
            let content = if settings_path.exists() {
                fs::read_to_string(&settings_path).unwrap_or_else(|_| "{}".to_string())
            } else {
                "{}".to_string()
            };
            write_json_response(
                stream,
                "200 OK",
                &json!({
                    "ok": true,
                    "settings": serde_json::from_str::<serde_json::Value>(&content).unwrap_or(json!({}))
                }).to_string(),
            )
        }
        line if line.starts_with("POST /api/settings ") => {
            let payload: serde_json::Value =
                parse_json_body(&request, header_end, "save settings")?;
            let config_home = runtime::default_config_home();
            fs::create_dir_all(&config_home)?;
            let settings_path = config_home.join("settings.json");
            let mut current = if settings_path.exists() {
                let content = fs::read_to_string(&settings_path)?;
                serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&content)
                    .unwrap_or_default()
            } else {
                serde_json::Map::new()
            };
            if let Some(obj) = payload.as_object() {
                for (k, v) in obj {
                    current.insert(k.clone(), v.clone());
                }
            }
            fs::write(
                &settings_path,
                serde_json::to_string_pretty(&serde_json::Value::Object(current))?,
            )?;
            write_json_response(stream, "200 OK", r#"{"ok":true}"#)
        }
        line if line.starts_with("POST /test-connection ") => {
            let payload: TestConnectionRequest =
                parse_json_body(&request, header_end, "test connection")?;
            match test_api_connection(&payload) {
                Ok(_) => write_json_response(stream, "200 OK", r#"{"ok":true}"#),
                Err(e) => write_json_response(
                    stream,
                    "200 OK",
                    &json!({"ok": false, "error": e.to_string()}).to_string(),
                ),
            }
        }
        line if line.starts_with("POST /chat ") => {
            let payload: ChatRequest = parse_json_body(&request, header_end, "chat")?;
            let response_data = run_clawie_prompt(
                &payload.message,
                payload.model.as_deref(),
                payload.openai_api_key.as_deref(),
                payload.anthropic_api_key.as_deref(),
                payload.gemini_api_key.as_deref(),
                payload.xai_api_key.as_deref(),
                payload.kimi_api_key.as_deref(),
                payload.openai_base_url.as_deref(),
                payload.gemini_base_url.as_deref(),
                payload.xai_base_url.as_deref(),
                payload.kimi_base_url.as_deref(),
                payload.lean_mode.as_deref(),
                payload.max_turns,
                payload.token_budget,
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
            let file_path = directory.join(safe_relative_path(&payload.filename)?);
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent)?;
            }
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

fn make_ws_text_frame(text: &str) -> Vec<u8> {
    let payload = text.as_bytes();
    let len = payload.len();
    let mut frame = Vec::new();
    frame.push(0x81); // FIN + Text opcode
    if len <= 125 {
        frame.push(len as u8);
    } else if len <= 65535 {
        frame.push(126);
        frame.push((len >> 8) as u8);
        frame.push((len & 0xff) as u8);
    } else {
        frame.push(127);
        for i in (0..8).rev() {
            frame.push((len >> (i * 8)) as u8);
        }
    }
    frame.extend_from_slice(payload);
    frame
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

fn running_clawie_instances() -> io::Result<Vec<serde_json::Value>> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid=,stat=,etime=,command="])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("failed to list running processes"));
    }

    let current_pid = std::process::id();
    let processes = String::from_utf8_lossy(&output.stdout);
    let rows = processes
        .lines()
        .filter_map(parse_process_line)
        .collect::<Vec<_>>();
    let candidate_pids = rows
        .iter()
        .filter(|process| is_clawie_instance_candidate(process, current_pid))
        .map(|process| process.pid)
        .collect::<std::collections::HashSet<_>>();
    let mut instances = rows
        .iter()
        .filter(|process| candidate_pids.contains(&process.pid))
        .filter(|process| !candidate_pids.contains(&process.ppid))
        .map(clawie_process_to_json)
        .collect::<Vec<_>>();

    instances.sort_by_key(|instance| {
        instance
            .get("pid")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default()
    });
    Ok(instances)
}

#[derive(Debug, Clone)]
struct ProcessInfo {
    pid: u32,
    ppid: u32,
    stat: String,
    elapsed: String,
    command: String,
}

fn parse_process_line(line: &str) -> Option<ProcessInfo> {
    let mut parts = line.split_whitespace();
    let pid = parts.next()?.parse::<u32>().ok()?;
    let ppid = parts.next()?.parse::<u32>().ok()?;
    let stat = parts.next()?.to_string();
    let elapsed = parts.next()?.to_string();
    let command = parts.collect::<Vec<_>>().join(" ");
    Some(ProcessInfo {
        pid,
        ppid,
        stat,
        elapsed,
        command,
    })
}

fn is_clawie_instance_candidate(process: &ProcessInfo, current_pid: u32) -> bool {
    let command_lower = process.command.to_ascii_lowercase();

    if command_lower.contains("ps -axo")
        || command_lower.contains("node --check")
        || command_lower.contains("rg -n")
        || command_lower.contains("rustc ")
        || process.pid == current_pid
        || command_lower.contains(" webui")
        || command_lower.contains(" web-ui")
    {
        return false;
    }

    command_lower.contains("target/debug/claw")
        || command_lower.contains("target/release/claw")
        || command_lower.contains("rusty-claude-cli")
        || command_lower.contains("cargo run")
        || command_lower.contains("/claw ")
        || command_lower.contains("./clawie")
        || command_lower.ends_with("/clawie")
        || command_lower.starts_with("claw ")
}

fn clawie_process_to_json(process: &ProcessInfo) -> serde_json::Value {
    let command = process.command.clone();
    let command_lower = command.to_ascii_lowercase();

    let kind = if command_lower.contains("target/debug/claw")
        || command_lower.contains("target/release/claw")
        || command_lower.contains("/claw ")
    {
        "Clawie CLI"
    } else if command_lower.contains("cargo run") {
        "Clawie Cargo"
    } else if command_lower.contains("./clawie") || command_lower.ends_with("/clawie") {
        "Clawie Launcher"
    } else {
        "Clawie"
    };
    let active = process.stat.contains('R') || command_lower.contains(" prompt ");
    let display_command = if command.chars().count() > 180 {
        format!("{}...", command.chars().take(180).collect::<String>())
    } else {
        command
    };

    json!({
        "pid": process.pid,
        "ppid": process.ppid,
        "stat": process.stat,
        "elapsed": process.elapsed,
        "elapsed_seconds": parse_elapsed_seconds(&process.elapsed),
        "kind": kind,
        "current": false,
        "active": active,
        "command": display_command,
    })
}

fn parse_elapsed_seconds(value: &str) -> u64 {
    let mut day_split = value.split('-');
    let first = day_split.next().unwrap_or_default();
    let (days, time_part) = if let Some(rest) = day_split.next() {
        (first.parse::<u64>().unwrap_or_default(), rest)
    } else {
        (0, first)
    };
    let parts = time_part
        .split(':')
        .filter_map(|part| part.parse::<u64>().ok())
        .collect::<Vec<_>>();
    let time_seconds = match parts.as_slice() {
        [hours, minutes, seconds] => hours * 3600 + minutes * 60 + seconds,
        [minutes, seconds] => minutes * 60 + seconds,
        [seconds] => *seconds,
        _ => 0,
    };
    days * 86_400 + time_seconds
}

fn query_value(request_line: &str, key: &str) -> Option<String> {
    let path = request_line.split_whitespace().nth(1)?;
    let query = path.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then(|| value.to_string())
    })
}

fn instance_log(pid: u32) -> io::Result<serde_json::Value> {
    let output = Command::new("ps")
        .args([
            "-p",
            &pid.to_string(),
            "-o",
            "pid=,ppid=,stat=,etime=,lstart=,command=",
        ])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("process {pid} is not running"),
        ));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next().unwrap_or_default().trim();
    if line.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("process {pid} is not running"),
        ));
    }

    let mut parts = line.split_whitespace();
    let parsed_pid = parts.next().unwrap_or_default();
    let ppid = parts.next().unwrap_or_default();
    let stat = parts.next().unwrap_or_default();
    let elapsed = parts.next().unwrap_or_default();
    let started = (0..5)
        .filter_map(|_| parts.next())
        .collect::<Vec<_>>()
        .join(" ");
    let command = parts.collect::<Vec<_>>().join(" ");
    let events = vec![
        format!("Detected process PID {parsed_pid}."),
        format!("Parent process PID {ppid}."),
        format!("State {stat}, elapsed {elapsed}."),
        format!("Started {started}."),
        format!("Command: {command}"),
    ];

    Ok(json!({
        "pid": parsed_pid,
        "ppid": ppid,
        "stat": stat,
        "elapsed": elapsed,
        "started": started,
        "command": command,
        "events": events,
    }))
}

fn run_instance_action(payload: &InstanceActionRequest) -> io::Result<()> {
    if payload.action != "terminate" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported instance action: {}", payload.action),
        ));
    }
    let instances = running_clawie_instances()?;
    let is_known_instance = instances.iter().any(|instance| {
        instance.get("pid").and_then(serde_json::Value::as_u64) == Some(payload.pid as u64)
    });
    if !is_known_instance {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("pid {} is not a running Clawie CLI instance", payload.pid),
        ));
    }

    let output = Command::new("kill")
        .args(["-TERM", &payload.pid.to_string()])
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(io::Error::other(if stderr.is_empty() {
            format!("failed to terminate pid {}", payload.pid)
        } else {
            stderr
        }))
    }
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

fn safe_relative_path(input: &str) -> io::Result<PathBuf> {
    let trimmed = input.trim().replace('\\', "/");
    if trimmed.is_empty() || trimmed.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "upload path cannot be empty",
        ));
    }
    let path = Path::new(&trimmed);
    if path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "upload path must be relative",
        ));
    }

    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => {
                let part = part.to_str().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "upload path must be utf-8")
                })?;
                if part.is_empty() || part == "." || part == ".." {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "upload path contains an invalid segment",
                    ));
                }
                safe.push(part);
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "upload path contains an invalid segment",
                ));
            }
        }
    }

    if safe.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "upload path must include a file name",
        ));
    }
    Ok(safe)
}

fn test_api_connection(req: &TestConnectionRequest) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    if req.provider == "anthropic" {
        let url = req
            .base_url
            .as_deref()
            .unwrap_or("https://api.anthropic.com")
            .trim_end_matches('/');
        let url = format!("{}/v1/messages", url);
        let model = req.model.as_deref().unwrap_or("claude-3-5-sonnet-20240620");

        let response = client
            .post(&url)
            .header("x-api-key", &req.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&json!({
                "model": model,
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "ping"}]
            }))
            .send()?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let text = response
                .text()
                .unwrap_or_else(|_| "Unknown error".to_string());
            let err_msg = if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                parsed["error"]["message"]
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or(text)
            } else {
                text
            };
            Err(format!(
                "Anthropic API returned status {}: {}",
                status.as_u16(),
                err_msg
            )
            .into())
        }
    } else if req.provider == "openai" {
        let url = req
            .base_url
            .as_deref()
            .unwrap_or("https://api.openai.com/v1")
            .trim_end_matches('/');
        let url = format!("{}/chat/completions", url);
        let model = req.model.as_deref().unwrap_or("gpt-4o");

        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", req.api_key))
            .header("content-type", "application/json")
            .json(&json!({
                "model": model,
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "ping"}]
            }))
            .send()?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let text = response
                .text()
                .unwrap_or_else(|_| "Unknown error".to_string());
            let err_msg = if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                parsed["error"]["message"]
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or(text)
            } else {
                text
            };
            Err(format!(
                "OpenAI API returned status {}: {}",
                status.as_u16(),
                err_msg
            )
            .into())
        }
    } else if req.provider == "gemini" {
        let url = req
            .base_url
            .as_deref()
            .unwrap_or("https://generativelanguage.googleapis.com/v1beta/openai")
            .trim_end_matches('/');
        let url = format!("{}/chat/completions", url);
        let model = req.model.as_deref().unwrap_or("gemini-1.5-pro");

        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", req.api_key))
            .header("content-type", "application/json")
            .json(&json!({
                "model": model,
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "ping"}]
            }))
            .send()?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let text = response
                .text()
                .unwrap_or_else(|_| "Unknown error".to_string());
            let err_msg = if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                parsed["error"]["message"]
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or(text)
            } else {
                text
            };
            Err(format!(
                "Gemini API returned status {}: {}",
                status.as_u16(),
                err_msg
            )
            .into())
        }
    } else if req.provider == "xai" || req.provider == "kimi" {
        let default_url = if req.provider == "xai" {
            "https://api.x.ai/v1"
        } else {
            "https://api.moonshot.cn/v1"
        };
        let url = req
            .base_url
            .as_deref()
            .unwrap_or(default_url)
            .trim_end_matches('/');
        let url = format!("{}/chat/completions", url);
        let default_model = if req.provider == "xai" {
            "grok-3"
        } else {
            "moonshot-v1-auto"
        };
        let model = req.model.as_deref().unwrap_or(default_model);

        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", req.api_key))
            .header("content-type", "application/json")
            .json(&json!({
                "model": model,
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "ping"}]
            }))
            .send()?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let text = response
                .text()
                .unwrap_or_else(|_| "Unknown error".to_string());
            let err_msg = if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                parsed["error"]["message"]
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or(text)
            } else {
                text
            };
            Err(format!(
                "{} API returned status {}: {}",
                req.provider,
                status.as_u16(),
                err_msg
            )
            .into())
        }
    } else {
        Err("Unsupported provider".into())
    }
}

fn run_clawie_prompt(
    message: &str,
    model: Option<&str>,
    openai_api_key: Option<&str>,
    anthropic_api_key: Option<&str>,
    gemini_api_key: Option<&str>,
    xai_api_key: Option<&str>,
    kimi_api_key: Option<&str>,
    openai_base_url: Option<&str>,
    gemini_base_url: Option<&str>,
    xai_base_url: Option<&str>,
    kimi_base_url: Option<&str>,
    lean_mode: Option<&str>,
    max_turns: Option<u32>,
    token_budget: Option<u32>,
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
    if let Some(key) = clean_api_key(openai_api_key) {
        cmd.env("OPENAI_API_KEY", key);
    } else if inherited_api_key_is_placeholder("OPENAI_API_KEY") {
        cmd.env_remove("OPENAI_API_KEY");
    }
    if let Some(key) = clean_api_key(anthropic_api_key) {
        cmd.env("ANTHROPIC_API_KEY", key);
    } else if inherited_api_key_is_placeholder("ANTHROPIC_API_KEY") {
        cmd.env_remove("ANTHROPIC_API_KEY");
    }
    if let Some(key) = clean_api_key(gemini_api_key) {
        cmd.env("GEMINI_API_KEY", key);
    } else if inherited_api_key_is_placeholder("GEMINI_API_KEY") {
        cmd.env_remove("GEMINI_API_KEY");
    }
    if let Some(key) = clean_api_key(xai_api_key) {
        cmd.env("XAI_API_KEY", key);
    } else if inherited_api_key_is_placeholder("XAI_API_KEY") {
        cmd.env_remove("XAI_API_KEY");
    }
    if let Some(key) = clean_api_key(kimi_api_key) {
        cmd.env("MOONSHOT_API_KEY", key);
    } else if inherited_api_key_is_placeholder("MOONSHOT_API_KEY") {
        cmd.env_remove("MOONSHOT_API_KEY");
    }
    if let Some(url) = openai_base_url {
        if !url.trim().is_empty() {
            cmd.env("OPENAI_BASE_URL", url.trim());
        }
    }
    if let Some(url) = gemini_base_url {
        if !url.trim().is_empty() {
            cmd.env("GEMINI_BASE_URL", url.trim());
        }
    }
    if let Some(url) = xai_base_url {
        if !url.trim().is_empty() {
            cmd.env("XAI_BASE_URL", url.trim());
        }
    }
    if let Some(url) = kimi_base_url {
        if !url.trim().is_empty() {
            cmd.env("MOONSHOT_BASE_URL", url.trim());
        }
    }
    if let Some(lm) = lean_mode {
        if !lm.trim().is_empty() {
            cmd.env("CLAWIE_LEAN_MODE", lm);
        }
    }
    if let Some(mt) = max_turns {
        cmd.env("CLAWIE_MAX_TURNS", mt.to_string());
    }
    if let Some(tb) = token_budget {
        cmd.env("CLAWIE_MAX_BUDGET_TOKENS", tb.to_string());
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
        let estimated_cost = parsed["estimated_cost"]
            .as_str()
            .unwrap_or("$0.00")
            .to_string();

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

fn clean_api_key(key: Option<&str>) -> Option<&str> {
    let key = key?.trim();
    if key.is_empty() || is_placeholder_api_key(key) {
        None
    } else {
        Some(key)
    }
}

fn inherited_api_key_is_placeholder(name: &str) -> bool {
    env::var(name)
        .map(|value| is_placeholder_api_key(value.trim()))
        .unwrap_or(false)
}

fn is_placeholder_api_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower == "dummy" || lower.contains("dummy") || lower.starts_with("test-")
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
  <link rel="manifest" href="/manifest.webmanifest">
  <meta name="theme-color" content="#09090b">
  <meta name="apple-mobile-web-app-capable" content="yes">
  <meta name="apple-mobile-web-app-title" content="Clawie">
  <link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=Outfit:wght@400;500;600;700&family=Fira+Sans:wght@400;500;600&family=JetBrains+Mono:wght@400;500;600&family=Fira+Code:wght@400;500;600&family=Source+Code+Pro:wght@400;500;600&display=swap" rel="stylesheet">
  <style>
    :root {
      color-scheme: dark;
      --bg-main: #09090b;       /* Zinc 950 */
      --bg-sidebar: #0f0f11;    /* Sleek sidebar */
      --bg-card: #18181b;       /* Zinc 900 */
      --bg-input: #09090b;      /* Zinc 950 input */
      --bg-code: #050507;
      --header-bg: rgba(9, 9, 11, 0.8);
      --panel-overlay: rgba(0, 0, 0, 0.15);
      --subtle-bg: rgba(255, 255, 255, 0.02);
      --hover-bg: rgba(255, 255, 255, 0.04);
      --inline-code-bg: rgba(255, 255, 255, 0.06);
      --modal-backdrop: rgba(0, 0, 0, 0.6);
      --panel-shadow: 0 12px 30px rgba(0, 0, 0, 0.25);
      --modal-shadow: 0 20px 40px rgba(0, 0, 0, 0.5);
      --editor-text: #a9b1d6;
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

    :root[data-app-theme="light"] {
      color-scheme: light;
      --bg-main: #f8fafc;
      --bg-sidebar: #ffffff;
      --bg-card: #ffffff;
      --bg-input: #f8fafc;
      --bg-code: #f1f5f9;
      --header-bg: rgba(255, 255, 255, 0.88);
      --panel-overlay: rgba(15, 23, 42, 0.04);
      --subtle-bg: rgba(15, 23, 42, 0.03);
      --hover-bg: rgba(15, 23, 42, 0.06);
      --inline-code-bg: rgba(15, 23, 42, 0.07);
      --modal-backdrop: rgba(15, 23, 42, 0.28);
      --panel-shadow: 0 12px 30px rgba(15, 23, 42, 0.08);
      --modal-shadow: 0 20px 40px rgba(15, 23, 42, 0.18);
      --editor-text: #1e293b;
      --border: #e2e8f0;
      --border-hover: #cbd5e1;
      --text-primary: #0f172a;
      --text-secondary: #334155;
      --text-muted: #64748b;
      --text-disabled: #94a3b8;
    }

    :root[data-app-theme="graphite"] {
      color-scheme: dark;
      --bg-main: #111113;
      --bg-sidebar: #18181a;
      --bg-card: #202024;
      --bg-input: #151518;
      --bg-code: #0c0c0e;
      --header-bg: rgba(24, 24, 26, 0.86);
      --panel-overlay: rgba(255, 255, 255, 0.04);
      --subtle-bg: rgba(255, 255, 255, 0.03);
      --hover-bg: rgba(255, 255, 255, 0.06);
      --inline-code-bg: rgba(255, 255, 255, 0.08);
      --modal-backdrop: rgba(0, 0, 0, 0.62);
      --panel-shadow: 0 12px 30px rgba(0, 0, 0, 0.28);
      --modal-shadow: 0 20px 40px rgba(0, 0, 0, 0.5);
      --editor-text: #d4d4d8;
      --border: #34343a;
      --border-hover: #52525b;
      --text-primary: #fafafa;
      --text-secondary: #d4d4d8;
      --text-muted: #a1a1aa;
      --text-disabled: #71717a;
    }

    :root[data-app-theme="contrast"] {
      color-scheme: dark;
      --bg-main: #000000;
      --bg-sidebar: #050505;
      --bg-card: #090909;
      --bg-input: #000000;
      --bg-code: #000000;
      --header-bg: rgba(0, 0, 0, 0.92);
      --panel-overlay: rgba(255, 255, 255, 0.06);
      --subtle-bg: rgba(255, 255, 255, 0.04);
      --hover-bg: rgba(255, 255, 255, 0.1);
      --inline-code-bg: rgba(255, 255, 255, 0.1);
      --modal-backdrop: rgba(0, 0, 0, 0.78);
      --panel-shadow: none;
      --modal-shadow: 0 20px 40px rgba(0, 0, 0, 0.72);
      --editor-text: #f8fafc;
      --border: #525252;
      --border-hover: #a3a3a3;
      --text-primary: #ffffff;
      --text-secondary: #f5f5f5;
      --text-muted: #d4d4d4;
      --text-disabled: #a3a3a3;
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
      background: var(--subtle-bg);
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
      background: var(--hover-bg);
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
      background: var(--header-bg);
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
      background: var(--subtle-bg);
    }

    .plan-pill strong {
      color: var(--text-secondary);
      font-weight: 600;
    }

    .view-switch {
      display: flex;
      align-items: center;
      gap: 0.25rem;
      border: 1px solid var(--border);
      border-radius: var(--radius-md);
      background: var(--subtle-bg);
      padding: 0.2rem;
    }

    .view-tab {
      border: 0;
      background: transparent;
      color: var(--text-muted);
      border-radius: var(--radius-sm);
      padding: 0.35rem 0.7rem;
      font: 700 0.72rem var(--font-ui);
      cursor: pointer;
      transition: background 0.15s ease, color 0.15s ease;
    }

    .view-tab:hover {
      color: var(--text-secondary);
      background: var(--hover-bg);
    }

    .view-tab.active {
      color: #ffffff;
      background: var(--accent);
    }

    .status-pill {
      font-size: 0.75rem;
      color: #ffffff;
      padding: 0.35rem 0.75rem;
      border: 1px solid var(--border);
      border-radius: var(--radius-sm);
      background: var(--subtle-bg);
      max-width: 250px;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
      transition: all 0.2s ease;
    }

    .status-pill.idle {
      color: #ffffff;
    }

    .status-pill.busy,
    .status-pill.thinking,
    .status-pill.uploading,
    .status-pill.listening {
      border-color: rgba(var(--accent-rgb), 0.35);
      background: rgba(var(--accent-rgb), 0.08);
      color: var(--accent-hover);
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

    .status-pill.error {
      border-color: rgba(239, 68, 68, 0.35);
      background: rgba(239, 68, 68, 0.08);
      color: #ef4444;
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

    .instance-page,
    .automations-page {
      width: 100%;
      max-width: 1400px;
      height: 100%;
    }

    .instance-page {
      display: grid;
      grid-template-columns: minmax(0, 1fr) 320px;
      gap: 1rem;
    }

    .instance-page[hidden], .workspace-content-wrap[hidden], .automations-page[hidden] {
      display: none !important;
    }

    .automations-page {
      display: block;
    }

    .automations-coming-soon {
      width: min(520px, 100%);
      border: 1px solid var(--border);
      border-radius: var(--radius-lg);
      background: var(--bg-card);
      box-shadow: var(--panel-shadow);
      padding: 3rem 2rem;
      text-align: center;
    }

    .automations-emoji {
      font-size: 4rem;
      line-height: 1;
      margin-bottom: 1rem;
    }

    .automations-coming-soon h2 {
      margin: 0;
      color: var(--text-primary);
      font-size: 1.35rem;
      letter-spacing: 0;
    }

    .automations-coming-soon p {
      margin: 0.5rem 0 0;
      color: var(--text-muted);
      font-size: 0.9rem;
    }

    .instance-stage {
      border: 4px solid #111827;
      border-radius: 0;
      overflow: hidden;
      background: #101827;
      box-shadow: 0 0 0 4px #303044, 0 18px 0 rgba(0,0,0,0.28), var(--panel-shadow);
      min-height: 0;
      display: flex;
      flex-direction: column;
    }

    .instance-titlebar {
      height: 36px;
      background: #26262b;
      border-bottom: 1px solid #16161a;
      display: flex;
      align-items: center;
      justify-content: space-between;
      padding: 0 0.85rem;
      font: 700 0.72rem var(--font-ui);
      letter-spacing: 0.08em;
      text-transform: uppercase;
      color: #d4d4d8;
      text-shadow: 2px 2px 0 #000000;
    }

    .zoom-stack {
      display: flex;
      gap: 0.35rem;
    }

    .zoom-btn {
      width: 26px;
      height: 26px;
      padding: 0;
      border: 2px solid #52526a;
      background: #232336;
      color: #f4f4f5;
      display: grid;
      place-items: center;
      font: 700 1rem var(--font-ui);
      box-shadow: inset 0 -2px 0 rgba(0,0,0,0.35);
      cursor: pointer;
      user-select: none;
    }

    .zoom-btn:hover {
      background: #303044;
      border-color: #71718a;
    }

    .zoom-btn:disabled {
      opacity: 0.45;
      cursor: not-allowed;
    }

    .pixel-map {
      flex: 1;
      min-height: 520px;
      position: relative;
      image-rendering: pixelated;
      background:
        radial-gradient(circle at 70% 18%, rgba(251, 191, 36, 0.16) 0 2px, transparent 3px 100%) 0 0 / 48px 48px,
        linear-gradient(90deg, rgba(0,0,0,0.12) 1px, transparent 1px) 0 0 / 32px 32px,
        linear-gradient(rgba(0,0,0,0.12) 1px, transparent 1px) 0 0 / 32px 32px,
        linear-gradient(90deg, #9a642d 0 52%, #202b3d 52% 100%);
      overflow: auto;
      cursor: grab;
      --instance-zoom: 1;
    }

    .instance-room-grid {
      position: absolute;
      inset: 16px;
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
      grid-auto-rows: minmax(210px, 1fr);
      gap: 14px;
      z-index: 5;
      overflow: auto;
      padding: 2px;
      transform: scale(var(--instance-zoom));
      transform-origin: top left;
      width: calc((100% - 32px) / var(--instance-zoom));
      height: calc((100% - 32px) / var(--instance-zoom));
    }

    .instance-room-grid::-webkit-scrollbar {
      width: 8px;
      height: 8px;
    }

    .instance-room-grid::-webkit-scrollbar-thumb {
      background: #303044;
      border: 2px solid #111827;
    }

    .instance-room {
      position: relative;
      min-height: 210px;
      border: 4px solid #111827;
      background:
        linear-gradient(90deg, rgba(80,44,18,0.28) 1px, transparent 1px) 0 0 / 28px 28px,
        linear-gradient(rgba(80,44,18,0.28) 1px, transparent 1px) 0 0 / 28px 28px,
        var(--room-floor, #9a642d);
      box-shadow:
        inset 0 0 0 3px rgba(255,255,255,0.08),
        0 6px 0 rgba(0,0,0,0.26);
      overflow: hidden;
    }

    .instance-room:nth-child(3n + 2) {
      --room-floor: #3e7894;
    }

    .instance-room:nth-child(3n + 3) {
      --room-floor: #eadfd8;
    }

    .instance-room.closed {
      filter: grayscale(0.55) brightness(0.78);
    }

    .instance-room.closed .status-beacon {
      background: #ef4444;
      box-shadow: 0 0 0 4px rgba(239,68,68,0.18), 0 0 18px #ef4444;
      animation: none;
    }

    .instance-room-title {
      position: absolute;
      top: 8px;
      left: 8px;
      right: 8px;
      z-index: 12;
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 8px;
      background: #111827;
      border: 2px solid #f8fafc;
      color: #f8fafc;
      padding: 3px 6px;
      font: 700 10px var(--font-code);
      text-shadow: 1px 1px 0 #000000;
      overflow: hidden;
    }

    .instance-room-title span {
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }

    .instance-room-title small {
      color: #86efac;
      font: inherit;
      flex: none;
    }

    .instance-room.closed .instance-room-title small {
      color: #fca5a5;
    }

    .task-box {
      position: absolute;
      width: 22px;
      height: 18px;
      background: #fbbf24;
      border: 3px solid #7c2d12;
      box-shadow: inset 0 5px 0 rgba(255,255,255,0.28), 0 3px 0 rgba(0,0,0,0.26);
      z-index: 20;
      animation: task-pop 1.25s ease-in-out infinite;
    }

    .task-box::after {
      content: "";
      position: absolute;
      left: 4px;
      top: 5px;
      width: 8px;
      height: 4px;
      background: #7c2d12;
      box-shadow: 8px 0 0 #7c2d12;
    }

    .task-box.two {
      animation-delay: 0.18s;
    }

    .task-box.three {
      animation-delay: 0.36s;
    }

    @keyframes task-pop {
      0%, 100% { transform: translateY(0); opacity: 0.72; }
      45% { transform: translateY(-8px); opacity: 1; }
    }

    .pixel-map::after {
      content: "";
      position: absolute;
      inset: 0;
      pointer-events: none;
      background:
        linear-gradient(rgba(255,255,255,0.035) 50%, rgba(0,0,0,0.035) 50%) 0 0 / 100% 4px,
        linear-gradient(90deg, rgba(255,255,255,0.018) 50%, rgba(0,0,0,0.018) 50%) 0 0 / 4px 100%;
      mix-blend-mode: soft-light;
      z-index: 40;
    }

    .pixel-room {
      position: absolute;
      border: 4px solid #111827;
      box-shadow: inset 0 0 0 2px rgba(255,255,255,0.08);
      overflow: hidden;
    }

    .pixel-room.code {
      left: 0;
      top: 0;
      width: 52%;
      height: 100%;
      background:
        linear-gradient(90deg, rgba(80,44,18,0.3) 1px, transparent 1px) 0 0 / 32px 32px,
        linear-gradient(rgba(80,44,18,0.3) 1px, transparent 1px) 0 0 / 32px 32px,
        #9a642d;
      border-left: 0;
      border-top: 0;
      border-bottom: 0;
    }

    .pixel-room.ops {
      right: 0;
      top: 0;
      width: 48%;
      height: 38%;
      background:
        linear-gradient(90deg, rgba(167,139,122,0.28) 1px, transparent 1px) 0 0 / 42px 42px,
        linear-gradient(rgba(167,139,122,0.28) 1px, transparent 1px) 0 0 / 42px 42px,
        #eadfd8;
      border-top: 0;
      border-right: 0;
    }

    .pixel-room.lounge {
      right: 0;
      bottom: 0;
      width: 48%;
      height: 62%;
      background:
        linear-gradient(90deg, rgba(20, 58, 77, 0.22) 1px, transparent 1px) 0 0 / 32px 32px,
        linear-gradient(rgba(20, 58, 77, 0.22) 1px, transparent 1px) 0 0 / 32px 32px,
        #3e7894;
      border-right: 0;
      border-bottom: 0;
    }

    .bookshelf, .desk, .server-rack, .coffee-table, .sofa, .plant, .agent, .status-beacon, .monitor {
      position: absolute;
      image-rendering: pixelated;
    }

    .bookshelf {
      width: 120px;
      height: 42px;
      background: #7b4a25;
      border: 3px solid #3b2418;
      box-shadow: inset 0 12px 0 #b8793d, inset 0 20px 0 #3b2418;
    }

    .bookshelf::after {
      content: "";
      position: absolute;
      left: 12px;
      top: 18px;
      width: 88px;
      height: 12px;
      background: repeating-linear-gradient(90deg, #d7e7ff 0 5px, #5c6fb1 5px 9px, #2dd4bf 9px 13px, #f8fafc 13px 18px);
    }

    .desk {
      width: 128px;
      height: 58px;
      background: #8b4f18;
      border: 4px solid #3b2418;
      box-shadow: inset 0 12px 0 #bd7a2c;
    }

    .monitor {
      width: 42px;
      height: 30px;
      background: #20243a;
      border: 4px solid #d9e2f2;
      box-shadow: inset 0 0 0 4px #5aa7e8;
    }

    .monitor::after {
      content: "";
      position: absolute;
      left: 8px;
      top: 6px;
      width: 18px;
      height: 8px;
      background: linear-gradient(90deg, #93c5fd 0 40%, #22d3ee 40% 70%, #f8fafc 70%);
      box-shadow: 0 10px 0 -2px #1e293b;
    }

    .instance-monitor {
      cursor: pointer;
      z-index: 18;
    }

    .instance-monitor:hover {
      filter: brightness(1.25) saturate(1.25);
      box-shadow: inset 0 0 0 4px #5aa7e8, 0 0 0 4px rgba(251,191,36,0.45);
    }

    .agent {
      width: 38px;
      height: 52px;
      border-radius: 0;
      background: var(--shirt, #1f2937);
      border: 4px solid #0f172a;
      box-shadow:
        inset 0 10px 0 rgba(255,255,255,0.16),
        inset 0 -8px 0 rgba(0,0,0,0.24),
        0 4px 0 rgba(0,0,0,0.28);
      cursor: grab;
      touch-action: none;
      z-index: 22;
      transition: filter 0.12s ease, transform 0.12s ease;
    }

    .agent::before {
      content: "";
      position: absolute;
      left: 3px;
      top: -24px;
      width: 24px;
      height: 24px;
      border-radius: 0;
      background:
        linear-gradient(var(--hair, #2f1c12) 0 6px, transparent 6px),
        linear-gradient(90deg, transparent 0 6px, #111827 6px 10px, transparent 10px 16px, #111827 16px 20px, transparent 20px),
        linear-gradient(var(--skin, #f4c7a1), var(--skin, #f4c7a1));
      border: 4px solid #0f172a;
      box-shadow:
        inset 0 -5px 0 rgba(0,0,0,0.12),
        4px 0 0 var(--hair, #2f1c12),
        -4px 0 0 var(--hair, #2f1c12);
    }

    .agent::after {
      content: attr(data-name);
      position: absolute;
      left: 50%;
      top: 58px;
      transform: translateX(-50%);
      background: #111827;
      border: 2px solid #f8fafc;
      color: #f8fafc;
      padding: 1px 5px;
      font: 700 9px var(--font-code);
      line-height: 1.2;
      text-shadow: 1px 1px 0 #000000;
      white-space: nowrap;
      opacity: 0;
      pointer-events: none;
    }

    .agent:hover,
    .agent.dragging {
      filter: saturate(1.3) brightness(1.08);
      transform: translateY(-2px);
      z-index: 35;
    }

    .agent:hover::after,
    .agent.dragging::after {
      opacity: 1;
    }

    .agent.dragging {
      cursor: grabbing;
      box-shadow:
        inset 0 10px 0 rgba(255,255,255,0.16),
        inset 0 -8px 0 rgba(0,0,0,0.24),
        0 0 0 4px rgba(251,191,36,0.45),
        0 8px 0 rgba(0,0,0,0.36);
    }

    .agent.red { --shirt: #a43f4f; --hair: #2f1c12; --skin: #c98a64; }
    .agent.blue { --shirt: #2b5f8f; --hair: #111827; --skin: #f2c9a5; }
    .agent.gold { --shirt: #b7791f; --hair: #d08b36; --skin: #f0b889; }
    .agent.green { --shirt: #1f8a5b; --hair: #4b2b18; --skin: #d99b75; }
    .agent.violet { --shirt: #6d4bb3; --hair: #23122f; --skin: #b9826a; }

    .agent.robot-agent {
      --shirt: #475569;
      background: #475569;
      border: 4px solid #0f172a;
      box-shadow: inset 0 10px 0 rgba(255,255,255,0.2), inset 0 -8px 0 rgba(0,0,0,0.3);
    }
    .agent.robot-agent::before {
      background: #64748b !important;
      border: 4px solid #0f172a;
      box-shadow: none !important;
      background-image: radial-gradient(circle at 4px 6px, #22c55e 2px, transparent 2px), radial-gradient(circle at 12px 6px, #22c55e 2px, transparent 2px) !important;
    }

    .server-rack {
      width: 56px;
      height: 86px;
      background: #cbd5e1;
      border: 4px solid #64748b;
      box-shadow: inset 0 14px 0 #94a3b8, inset 0 35px 0 #1f2937;
    }

    .server-rack::after {
      content: "";
      position: absolute;
      left: 10px;
      top: 43px;
      width: 34px;
      height: 20px;
      background: repeating-linear-gradient(90deg, #ef4444 0 5px, #22c55e 5px 10px, #e5e7eb 10px 15px);
    }

    .coffee-table {
      width: 112px;
      height: 64px;
      background: #b8793d;
      border: 4px solid #6b3b18;
    }

    .sofa {
      width: 96px;
      height: 62px;
      background: #be6f86;
      border: 4px solid #7f3f56;
    }

    .plant {
      width: 22px;
      height: 34px;
      background: #f8fafc;
      border: 3px solid #64748b;
    }

    .plant::before {
      content: "";
      position: absolute;
      left: -12px;
      top: -34px;
      width: 44px;
      height: 38px;
      background: repeating-linear-gradient(60deg, transparent 0 8px, #2f9e66 8px 14px, transparent 14px 22px);
    }

    .status-beacon {
      width: 14px;
      height: 14px;
      border-radius: 50%;
      background: #22c55e;
      box-shadow: 0 0 0 4px rgba(34,197,94,0.18), 0 0 18px #22c55e;
      animation: pulse 1.4s ease-in-out infinite;
    }

    @keyframes pulse {
      0%, 100% { transform: scale(1); opacity: 0.9; }
      50% { transform: scale(1.22); opacity: 1; }
    }

    .instance-panel {
      border: 1px solid var(--border);
      border-radius: var(--radius-lg);
      background: var(--bg-card);
      box-shadow: var(--panel-shadow);
      overflow: hidden;
      display: flex;
      flex-direction: column;
      min-height: 0;
    }

    .instance-panel-header {
      padding: 1rem;
      border-bottom: 1px solid var(--border);
      background: var(--panel-overlay);
    }

    .instance-panel-header h2 {
      font-size: 0.9rem;
      margin-bottom: 0.2rem;
    }

    .instance-panel-header p {
      color: var(--text-muted);
      font-size: 0.75rem;
    }

    .instance-metrics {
      padding: 1rem;
      display: grid;
      gap: 0.75rem;
      overflow-y: auto;
    }

    .metric-row {
      border: 1px solid var(--border);
      border-radius: var(--radius-md);
      padding: 0.75rem;
      background: var(--subtle-bg);
    }

    .metric-label {
      color: var(--text-muted);
      font-size: 0.68rem;
      font-weight: 700;
      text-transform: uppercase;
      letter-spacing: 0.06em;
      margin-bottom: 0.25rem;
    }

    .metric-value {
      color: var(--text-primary);
      font: 600 0.88rem var(--font-code);
      overflow-wrap: anywhere;
    }

    .metric-value.live {
      color: var(--ok);
    }

    .instance-list-header {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 0.5rem;
    }

    .instance-refresh {
      border: 1px solid var(--border);
      background: var(--bg-input);
      color: var(--text-secondary);
      border-radius: var(--radius-sm);
      padding: 0.25rem 0.5rem;
      font: 700 0.65rem var(--font-ui);
      cursor: pointer;
    }

    .instance-refresh:hover {
      border-color: var(--border-hover);
      color: var(--text-primary);
    }

    .process-list {
      display: grid;
      gap: 0.5rem;
    }

    .process-card {
      border: 1px solid var(--border);
      border-radius: var(--radius-md);
      background: var(--subtle-bg);
      padding: 0.65rem;
      display: grid;
      gap: 0.35rem;
    }

    .process-card.current {
      border-color: rgba(16,185,129,0.45);
      background: rgba(16,185,129,0.07);
    }

    .process-top {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 0.5rem;
      font: 700 0.76rem var(--font-ui);
    }

    .process-kind {
      color: var(--text-primary);
    }

    .process-pid {
      color: var(--text-muted);
      font-family: var(--font-code);
      font-size: 0.7rem;
    }

    .process-command {
      color: var(--text-muted);
      font: 0.68rem var(--font-code);
      line-height: 1.35;
      overflow-wrap: anywhere;
    }

    .process-empty {
      color: var(--text-muted);
      font-size: 0.75rem;
      text-align: center;
      padding: 0.8rem;
      border: 1px dashed var(--border);
      border-radius: var(--radius-md);
    }

    .agent-menu {
      position: fixed;
      z-index: 20000;
      min-width: 190px;
      background: #111827;
      border: 2px solid #f8fafc;
      box-shadow: 4px 4px 0 rgba(0,0,0,0.45);
      padding: 0.25rem;
    }

    .agent-menu[hidden] {
      display: none;
    }

    .agent-menu button {
      width: 100%;
      border: 0;
      background: transparent;
      color: #f8fafc;
      text-align: left;
      font: 700 0.72rem var(--font-ui);
      padding: 0.45rem 0.55rem;
      cursor: pointer;
    }

    .agent-menu button:hover {
      background: #1f2937;
    }

    .agent-menu button.danger {
      color: #fca5a5;
    }

    @media (max-width: 1100px) {
      header {
        padding: 0 0.75rem;
        gap: 0.5rem;
      }
      .plan-pill, .usage-container {
        display: none !important;
      }
      .instance-page {
        grid-template-columns: 1fr;
        overflow-y: auto;
      }
      .pixel-map {
        min-height: 420px;
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
      background: var(--bg-code);
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
      background: var(--inline-code-bg);
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
      background: var(--hover-bg);
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
      box-shadow: var(--panel-shadow);
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

    .error-action-panel {
      margin-top: 0.75rem;
      padding: 0.85rem;
      background: rgba(245, 158, 11, 0.1);
      border: 1px solid rgba(245, 158, 11, 0.22);
      border-radius: var(--radius-sm);
      display: flex;
      flex-direction: column;
      gap: 0.65rem;
    }

    .error-action-panel.error-action-critical {
      background: rgba(239, 68, 68, 0.08);
      border-color: rgba(239, 68, 68, 0.25);
    }

    .error-action-text {
      font-size: 0.85rem;
      line-height: 1.45;
      color: var(--text-secondary);
    }

    .error-action-btn {
      align-self: flex-start;
      background: var(--accent);
      color: #ffffff;
      border: none;
      padding: 0.45rem 0.8rem;
      border-radius: var(--radius-sm);
      cursor: pointer;
      font-size: 0.8rem;
      font-weight: 600;
      transition: background 0.2s;
    }

    .error-action-btn:hover {
      background: var(--accent-hover);
    }

    .chat-input-row {
      border-top: 1px solid var(--border);
      background: var(--panel-overlay);
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
      background: var(--modal-backdrop);
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
      box-shadow: var(--modal-shadow);
      overflow: hidden;
      animation: modalFadeIn 0.2s ease;
    }
    #settings-modal .modal-content {
      width: min(720px, calc(100vw - 32px));
      height: min(720px, calc(100vh - 32px));
      display: grid;
      grid-template-rows: auto auto 1fr auto;
      background: rgba(18, 18, 24, 0.75);
      backdrop-filter: blur(16px);
      -webkit-backdrop-filter: blur(16px);
      border: 1px solid rgba(255, 255, 255, 0.08);
      box-shadow: 0 20px 50px rgba(0, 0, 0, 0.4);
    }
    #instance-log-modal .modal-content {
      width: min(760px, calc(100vw - 32px));
      height: min(640px, calc(100vh - 32px));
      display: grid;
      grid-template-rows: auto 1fr;
    }
    .log-body {
      padding: 1rem;
      overflow: auto;
      background: #050507;
      font-family: var(--font-code);
      color: #d4d4d8;
    }
    .log-line {
      border-left: 3px solid #303044;
      padding: 0.45rem 0.65rem;
      margin-bottom: 0.5rem;
      background: rgba(255,255,255,0.035);
      overflow-wrap: anywhere;
    }
    .log-line strong {
      color: #86efac;
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
    #settings-modal .modal-body {
      padding: 0;
      overflow: hidden;
      display: block;
    }
    .settings-tabs {
      display: flex;
      gap: 1.5rem;
      border-bottom: 1px solid var(--border);
      padding: 0 1.5rem;
      background: rgba(255, 255, 255, 0.01);
    }
    .settings-tab-btn {
      background: transparent;
      border: none;
      color: var(--text-muted);
      padding: 1rem 0;
      font-size: 0.85rem;
      font-weight: 600;
      cursor: pointer;
      position: relative;
      transition: color 0.2s ease;
    }
    .settings-tab-btn:hover {
      color: var(--text-primary);
    }
    .settings-tab-btn.active {
      color: var(--accent);
    }
    .settings-tab-btn.active::after {
      content: '';
      position: absolute;
      bottom: 0;
      left: 0;
      width: 100%;
      height: 2px;
      background: var(--accent);
      border-radius: 2px;
    }
    .settings-tab-content {
      display: none;
      height: 100%;
      overflow-y: auto;
      padding: 1.5rem;
    }
    .settings-tab-content.active {
      display: block;
    }
    .settings-pane-grid {
      display: grid;
      grid-template-columns: repeat(2, minmax(0, 1fr));
      gap: 1rem;
      align-content: start;
    }
    .password-input-wrapper {
      position: relative;
      display: flex;
      align-items: center;
      width: 100%;
    }
    .password-input-wrapper input {
      width: 100%;
      padding-right: 2.25rem !important;
    }
    .password-toggle-btn {
      position: absolute;
      right: 0.75rem;
      background: transparent;
      border: none;
      color: var(--text-muted);
      cursor: pointer;
      display: flex;
      align-items: center;
      justify-content: center;
      padding: 0;
      transition: color 0.15s ease;
    }
    .password-toggle-btn:hover {
      color: var(--text-primary);
    }
    .password-toggle-btn svg {
      width: 16px;
      height: 16px;
    }
    .settings-connection-status {
      display: flex;
      align-items: center;
      gap: 0.5rem;
      font-size: 0.75rem;
      margin-top: 0.25rem;
    }
    .status-dot {
      width: 8px;
      height: 8px;
      border-radius: 50%;
      background: var(--text-muted);
      display: inline-block;
    }
    .status-dot.success {
      background: #10b981;
      box-shadow: 0 0 8px rgba(16, 185, 129, 0.5);
    }
    .status-dot.error {
      background: #ef4444;
      box-shadow: 0 0 8px rgba(239, 68, 68, 0.5);
    }
    .status-dot.testing {
      background: #3b82f6;
      box-shadow: 0 0 8px rgba(59, 130, 246, 0.5);
      animation: pulse-status 1s infinite alternate;
    }
    @keyframes pulse-status {
      from { opacity: 0.5; }
      to { opacity: 1; }
    }
    .settings-group {
      display: flex;
      flex-direction: column;
      gap: 0.5rem;
    }
    #settings-modal .settings-group {
      background: var(--subtle-bg);
      border: 1px solid var(--border);
      border-radius: var(--radius-md);
      padding: 1rem;
      min-width: 0;
    }
    #settings-modal .settings-group.settings-wide {
      grid-column: 1 / -1;
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
    .settings-help {
      font-size: 0.72rem;
      color: var(--text-muted);
      line-height: 1.45;
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
      background: var(--panel-overlay);
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
    .settings-btn-secondary {
      background: var(--bg-input);
      color: var(--text-primary);
      border: 1px solid var(--border);
      padding: 0.5rem 1rem;
      border-radius: var(--radius-md);
      font-weight: 600;
      font-size: 0.8rem;
      cursor: pointer;
      transition: border-color 0.15s ease, background 0.15s ease;
    }
    .settings-btn-secondary:hover {
      background: var(--hover-bg);
      border-color: var(--border-hover);
    }
    @media (max-width: 680px) {
      #settings-modal .modal-content {
        width: calc(100vw - 24px);
        height: calc(100vh - 24px);
      }
      #settings-modal .modal-body {
        grid-template-columns: 1fr;
      }
    }

    /* CURATED DESIGNER THEMES */
    :root[data-app-theme="tokyo-night"] {
      color-scheme: dark;
      --bg-main: #1a1b26;
      --bg-sidebar: #16161e;
      --bg-card: #24283b;
      --bg-input: #16161e;
      --bg-code: #16161f;
      --header-bg: rgba(26, 27, 38, 0.88);
      --panel-overlay: rgba(0, 0, 0, 0.2);
      --subtle-bg: rgba(255, 255, 255, 0.02);
      --hover-bg: rgba(255, 255, 255, 0.04);
      --inline-code-bg: rgba(255, 255, 255, 0.06);
      --modal-backdrop: rgba(0, 0, 0, 0.65);
      --panel-shadow: 0 12px 30px rgba(0, 0, 0, 0.3);
      --modal-shadow: 0 20px 40px rgba(0, 0, 0, 0.55);
      --editor-text: #a9b1d6;
      --border: #383e5a;
      --border-hover: #414868;
      --text-primary: #c0caf5;
      --text-secondary: #a9b1d6;
      --text-muted: #565f89;
      --text-disabled: #444b6a;
    }

    :root[data-app-theme="dracula"] {
      color-scheme: dark;
      --bg-main: #282a36;
      --bg-sidebar: #21222c;
      --bg-card: #2d313f;
      --bg-input: #1e1f29;
      --bg-code: #1e1f29;
      --header-bg: rgba(33, 34, 44, 0.88);
      --panel-overlay: rgba(0, 0, 0, 0.2);
      --subtle-bg: rgba(255, 255, 255, 0.02);
      --hover-bg: rgba(255, 255, 255, 0.04);
      --inline-code-bg: rgba(255, 255, 255, 0.06);
      --modal-backdrop: rgba(0, 0, 0, 0.65);
      --panel-shadow: 0 12px 30px rgba(0, 0, 0, 0.3);
      --modal-shadow: 0 20px 40px rgba(0, 0, 0, 0.55);
      --editor-text: #f8f8f2;
      --border: #44475a;
      --border-hover: #6272a4;
      --text-primary: #f8f8f2;
      --text-secondary: #e2e2dc;
      --text-muted: #6272a4;
      --text-disabled: #44475a;
    }

    :root[data-app-theme="nord"] {
      color-scheme: dark;
      --bg-main: #2e3440;
      --bg-sidebar: #242933;
      --bg-card: #3b4252;
      --bg-input: #242933;
      --bg-code: #20242c;
      --header-bg: rgba(36, 41, 51, 0.88);
      --panel-overlay: rgba(0, 0, 0, 0.2);
      --subtle-bg: rgba(255, 255, 255, 0.02);
      --hover-bg: rgba(255, 255, 255, 0.04);
      --inline-code-bg: rgba(255, 255, 255, 0.06);
      --modal-backdrop: rgba(0, 0, 0, 0.65);
      --panel-shadow: 0 12px 30px rgba(0, 0, 0, 0.3);
      --modal-shadow: 0 20px 40px rgba(0, 0, 0, 0.55);
      --editor-text: #eceff4;
      --border: #4c566a;
      --border-hover: #81a1c1;
      --text-primary: #eceff4;
      --text-secondary: #e5e9f0;
      --text-muted: #8892b0;
      --text-disabled: #4c566a;
    }

    :root[data-app-theme="synthwave"] {
      color-scheme: dark;
      --bg-main: #2b213a;
      --bg-sidebar: #241b2f;
      --bg-card: #34294f;
      --bg-input: #241b2f;
      --bg-code: #1c1424;
      --header-bg: rgba(36, 27, 47, 0.88);
      --panel-overlay: rgba(0, 0, 0, 0.2);
      --subtle-bg: rgba(255, 255, 255, 0.02);
      --hover-bg: rgba(255, 255, 255, 0.04);
      --inline-code-bg: rgba(255, 255, 255, 0.06);
      --modal-backdrop: rgba(0, 0, 0, 0.65);
      --panel-shadow: 0 12px 30px rgba(0, 0, 0, 0.3);
      --modal-shadow: 0 20px 40px rgba(0, 0, 0, 0.55);
      --editor-text: #fede5d;
      --border: #523c75;
      --border-hover: #f92aad;
      --text-primary: #f0efe2;
      --text-secondary: #b3a7d4;
      --text-muted: #8473a5;
      --text-disabled: #523c75;
    }

    /* CUSTOM DYNAMIC FONTS */
    :root[data-font-ui="inter"] {
      --font-ui: "Inter", system-ui, -apple-system, sans-serif;
    }
    :root[data-font-ui="outfit"] {
      --font-ui: "Outfit", system-ui, -apple-system, sans-serif;
    }
    :root[data-font-ui="fira-sans"] {
      --font-ui: "Fira Sans", system-ui, -apple-system, sans-serif;
    }
    :root[data-font-ui="system"] {
      --font-ui: system-ui, -apple-system, sans-serif;
    }

    :root[data-font-code="jetbrains"] {
      --font-code: "JetBrains Mono", ui-monospace, monospace;
    }
    :root[data-font-code="fira-code"] {
      --font-code: "Fira Code", ui-monospace, monospace;
    }
    :root[data-font-code="source-code"] {
      --font-code: "Source Code Pro", ui-monospace, monospace;
    }
    :root[data-font-code="monospace"] {
      --font-code: ui-monospace, monospace;
    }

    /* COMPACT DENSITY SETTINGS */
    :root[data-layout-density="compact"] {
      --radius-lg: 6px;
      --radius-md: 4px;
      --radius-sm: 3px;
    }
    :root[data-layout-density="compact"] body {
      font-size: 13px;
    }
    :root[data-layout-density="compact"] .sidebar {
      width: 220px;
    }
    :root[data-layout-density="compact"] .file {
      padding: 0.35rem 0.5rem !important;
      font-size: 0.75rem;
    }
    :root[data-layout-density="compact"] .message {
      padding: 0.5rem 0.75rem !important;
    }
    :root[data-layout-density="compact"] .message-content {
      font-size: 0.82rem;
    }
    :root[data-layout-density="compact"] header {
      padding: 0.5rem 1rem !important;
      height: 48px;
    }
    :root[data-layout-density="compact"] .editor-header {
      padding: 0.35rem 0.75rem !important;
    }

    /* SLIM SIDEBAR SETTINGS */
    :root[data-sidebar-layout="slim"] .sidebar {
      width: 68px !important;
      padding: 1.25rem 0.5rem !important;
      align-items: center;
    }
    :root[data-sidebar-layout="slim"] .sidebar-title h2,
    :root[data-sidebar-layout="slim"] #current-folder span,
    :root[data-sidebar-layout="slim"] .file span {
      display: none !important;
    }
    :root[data-sidebar-layout="slim"] #current-folder {
      justify-content: center;
      padding: 0.5rem 0 !important;
    }
    :root[data-sidebar-layout="slim"] .file {
      justify-content: center !important;
      padding: 0.5rem 0 !important;
    }
    :root[data-sidebar-layout="slim"] .file svg {
      width: 18px !important;
      height: 18px !important;
    }

    /* AMBIENT GLOW ANIMATION */
    .ambient-glow {
      position: fixed;
      width: 500px;
      height: 500px;
      border-radius: 50%;
      background: radial-gradient(circle, rgba(var(--accent-rgb), 0.08) 0%, rgba(var(--accent-rgb), 0) 70%);
      filter: blur(80px);
      z-index: -1;
      pointer-events: none;
      opacity: 0;
      transition: opacity 0.5s ease;
    }
    :root[data-ambient-glow="true"] .ambient-glow {
      opacity: 1;
    }
    #ambient-glow-1 {
      top: -150px;
      left: -150px;
      animation: float-glow-1 25s infinite alternate ease-in-out;
    }
    #ambient-glow-2 {
      bottom: -200px;
      right: -150px;
      animation: float-glow-2 30s infinite alternate ease-in-out;
    }
    #ambient-glow-3 {
      top: 40%;
      left: 50%;
      transform: translate(-50%, -50%);
      width: 700px;
      height: 700px;
      background: radial-gradient(circle, rgba(var(--accent-rgb), 0.03) 0%, rgba(var(--accent-rgb), 0) 75%);
    }
    @keyframes float-glow-1 {
      0% { transform: translate(0, 0) scale(1); }
      100% { transform: translate(150px, 100px) scale(1.15); }
    }
    @keyframes float-glow-2 {
      0% { transform: translate(0, 0) scale(1); }
      100% { transform: translate(-120px, -100px) scale(0.9); }
    }

    /* RETRO CRT TERMINAL SCANLINES & GLOW */
    :root[data-terminal-glow="true"] .log-body {
      position: relative;
      text-shadow: 0 0 4px rgba(134, 239, 172, 0.45);
    }
    :root[data-terminal-glow="true"] .log-body::after {
      content: "";
      position: absolute;
      top: 0; left: 0; bottom: 0; right: 0;
      background: linear-gradient(rgba(18, 16, 16, 0) 50%, rgba(0, 0, 0, 0.18) 50%);
      background-size: 100% 4px;
      pointer-events: none;
      z-index: 10;
    }

    /* PULSATING STATUS AURA */
    :root[data-status-pulsate="true"] #status.busy,
    :root[data-status-pulsate="true"] #status.thinking {
      animation: pulse-status-aura 1.5s infinite alternate ease-in-out;
      box-shadow: 0 0 12px rgba(var(--accent-rgb), 0.5);
    }
    @keyframes pulse-status-aura {
      0% { box-shadow: 0 0 4px rgba(var(--accent-rgb), 0.3); }
      100% { box-shadow: 0 0 16px rgba(var(--accent-rgb), 0.7); }
    }
    @keyframes float-shrimp {
      0% { transform: translateY(0) rotate(0deg); }
      50% { transform: translateY(-15px) rotate(10deg); }
      100% { transform: translateY(0) rotate(0deg); }
    }

    /* AUTOMATION FLOW BUILDER STYLE */
    .automations-page {
      display: block;
      padding: 1.5rem;
      height: calc(100vh - 64px);
      overflow: hidden;
    }
    .automation-builder-container {
      display: grid;
      grid-template-columns: 1fr 340px;
      gap: 1.5rem;
      height: 100%;
      align-items: start;
    }
    .automation-canvas {
      background: var(--bg-card);
      border: 1px solid var(--border);
      border-radius: var(--radius-lg);
      height: 100%;
      display: grid;
      grid-template-rows: auto 1fr;
      overflow: hidden;
      box-shadow: var(--panel-shadow);
    }
    .canvas-header {
      padding: 1.25rem 1.5rem;
      border-bottom: 1px solid var(--border);
      display: flex;
      justify-content: space-between;
      align-items: center;
      background: rgba(255,255,255,0.01);
    }
    .canvas-header h3 {
      font-size: 0.95rem;
      font-weight: 600;
      color: var(--text-primary);
      margin: 0;
    }
    .canvas-subtitle {
      font-size: 0.72rem;
      color: var(--text-muted);
    }
    .canvas-actions {
      display: flex;
      gap: 0.5rem;
    }
    .flow-nodes-list {
      padding: 2rem;
      overflow-y: auto;
      display: flex;
      flex-direction: column;
      align-items: center;
      gap: 0;
    }
    .flow-node {
      background: var(--bg-main);
      border: 1px solid var(--border);
      border-radius: var(--radius-md);
      width: 260px;
      box-shadow: 0 4px 20px rgba(0, 0, 0, 0.15);
      overflow: hidden;
      transition: border-color 0.2s ease, box-shadow 0.2s ease;
      animation: node-fade-in 0.25s ease-out;
      cursor: grab;
      position: absolute;
    }
    .flow-node:active {
      cursor: grabbing;
    }
    @keyframes node-fade-in {
      from { opacity: 0; transform: translateY(10px); }
      to { opacity: 1; transform: translateY(0); }
    }
    .flow-node:hover {
      border-color: var(--border-hover);
      box-shadow: 0 6px 24px rgba(0, 0, 0, 0.25);
    }
    .flow-node.trigger-node {
      border-left: 4px solid var(--accent);
    }
    .flow-node.action-node {
      border-left: 4px solid #3b82f6;
    }
    .flow-node input,
    .flow-node textarea,
    .flow-node select {
      font-size: 0.7rem;
      padding: 0.3rem 0.5rem;
    }
    .node-port {
      width: 10px;
      height: 10px;
      background: var(--border);
      border: 2px solid var(--bg-card);
      border-radius: 50%;
      position: absolute;
      z-index: 10;
      transition: background-color 0.2s, transform 0.2s;
    }
    .node-port:hover {
      background: var(--accent) !important;
      transform: translateY(-50%) scale(1.3);
    }
    .input-port {
      left: -6px;
      top: 35px; /* middle of header */
      transform: translateY(-50%);
    }
    .output-port {
      right: -6px;
      top: 35px; /* middle of header */
      transform: translateY(-50%);
    }
    @keyframes stroke-pulse {
      to {
        stroke-dashoffset: -20;
      }
    }
    .connection-path.simulating {
      stroke: #10b981 !important;
      stroke-dasharray: 6, 4;
      animation: stroke-pulse 1s infinite linear;
    }
    .drawer-field-wrap {
      display: flex;
      flex-direction: column;
      gap: 0.35rem;
      margin-bottom: 0.5rem;
    }
    .drawer-field-wrap label {
      font-size: 0.75rem;
      color: var(--text-muted);
      font-weight: 500;
    }
    .drawer-field-wrap input,
    .drawer-field-wrap textarea,
    .drawer-field-wrap select {
      background: var(--bg-main);
      border: 1px solid var(--border);
      border-radius: var(--radius-sm);
      color: var(--text-primary);
      padding: 0.5rem;
      font-size: 0.8rem;
    }
    .flow-node.action-node.ai-agent-node {
      border-left-color: #3b82f6;
    }
    .flow-node.action-node.simple-agent-node {
      border-left-color: #8b5cf6;
    }
    .flow-node.action-node.email-node {
      border-left-color: #ea580c;
    }
    .flow-node.action-node.summarize-node {
      border-left-color: #06b6d4;
    }
    .flow-node.action-node.slack-node {
      border-left-color: #ec4899;
    }
    .flow-node.action-node.bash-node {
      border-left-color: #10b981;
    }
    .flow-node.action-node.notify-node {
      border-left-color: #f59e0b;
    }
    .flow-node.action-node.http-node {
      border-left-color: #38bdf8;
    }
    .flow-node.action-node.nested-node {
      border-left-color: #fb7185;
    }
    .flow-node.action-node.routing-node {
      border-left-color: #fbbf24;
    }
    .flow-node.action-node.iterator-node {
      border-left-color: #34d399;
    }
    .node-header {
      padding: 0.75rem 1rem;
      background: rgba(255, 255, 255, 0.02);
      border-bottom: 1px solid var(--border);
      display: flex;
      align-items: center;
      gap: 0.75rem;
    }
    .node-icon {
      width: 28px;
      height: 28px;
      border-radius: var(--radius-sm);
      background: rgba(var(--accent-rgb), 0.1);
      color: var(--accent);
      display: grid;
      place-items: center;
      font-size: 0.95rem;
    }
    .node-title-wrap {
      display: flex;
      flex-direction: column;
      flex: 1;
      min-width: 0;
    }
    .node-label {
      font-size: 0.6rem;
      font-weight: 700;
      color: var(--accent);
      letter-spacing: 0.05em;
    }
    .node-name {
      font-size: 0.82rem;
      color: var(--text-primary);
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .node-remove-btn {
      background: transparent;
      border: none;
      color: var(--text-muted);
      font-size: 1.2rem;
      cursor: pointer;
      line-height: 1;
      padding: 0 0.25rem;
      transition: color 0.15s ease;
    }
    .node-remove-btn:hover {
      color: #ef4444;
    }
    .node-body {
      padding: 1rem;
      display: flex;
      flex-direction: column;
      gap: 0.5rem;
    }
    .node-body label {
      font-size: 0.68rem;
      font-weight: 600;
      color: var(--text-muted);
      text-transform: uppercase;
      letter-spacing: 0.02em;
    }
    .node-body input, .node-body textarea {
      width: 100%;
      font-size: 0.78rem;
      background: var(--bg-input);
      border: 1px solid var(--border);
      border-radius: var(--radius-sm);
      padding: 0.4rem 0.65rem;
      color: var(--text-primary);
      outline: none;
      transition: border-color 0.15s ease;
    }
    .node-body input:focus, .node-body textarea:focus {
      border-color: var(--accent);
    }
    
    /* FLOW CONNECTOR ELEMENTS */
    .flow-connector {
      display: flex;
      flex-direction: column;
      align-items: center;
      height: 64px;
      position: relative;
    }
    .connector-line {
      width: 2px;
      height: 100%;
      background: var(--border);
    }
    .add-node-btn {
      position: absolute;
      top: 50%;
      transform: translateY(-50%);
      width: 24px;
      height: 24px;
      border-radius: 50%;
      background: var(--bg-card);
      border: 1px solid var(--border);
      color: var(--text-secondary);
      display: grid;
      place-items: center;
      cursor: pointer;
      font-size: 0.95rem;
      font-weight: bold;
      transition: all 0.15s ease;
      box-shadow: 0 2px 8px rgba(0, 0, 0, 0.1);
    }
    .add-node-btn:hover {
      border-color: var(--accent);
      color: var(--accent);
      transform: translateY(-50%) scale(1.15);
    }

    /* RIGHT SIDEBAR: TOOLBOX */
    .automation-toolbox {
      background: var(--bg-card);
      border: 1px solid var(--border);
      border-radius: var(--radius-lg);
      padding: 1.5rem;
      height: 100%;
      overflow-y: auto;
      box-shadow: var(--panel-shadow);
      display: flex;
      flex-direction: column;
      gap: 1.25rem;
    }
    .automation-toolbox h3 {
      font-size: 0.95rem;
      font-weight: 600;
      color: var(--text-primary);
      margin: 0;
    }
    .toolbox-desc {
      font-size: 0.72rem;
      color: var(--text-muted);
      margin: 0;
      line-height: 1.4;
    }
    .toolbox-section h4 {
      font-size: 0.72rem;
      font-weight: 700;
      text-transform: uppercase;
      letter-spacing: 0.05em;
      color: var(--text-muted);
      margin: 0 0 0.75rem 0;
    }
    .template-presets-grid {
      display: grid;
      grid-template-columns: 1fr;
      gap: 0.5rem;
    }
    .template-chip {
      background: var(--bg-main);
      border: 1px solid var(--border);
      border-radius: var(--radius-md);
      padding: 0.65rem 0.85rem;
      text-align: left;
      cursor: pointer;
      transition: all 0.2s ease;
      display: flex;
      flex-direction: column;
      gap: 0.15rem;
      width: 100%;
    }
    .template-chip:hover {
      border-color: var(--accent);
      transform: translateX(2px);
      background: rgba(var(--accent-rgb), 0.02);
    }
    .template-chip strong {
      font-size: 0.78rem;
      color: var(--text-primary);
    }
    .template-chip span {
      font-size: 0.68rem;
      color: var(--text-muted);
    }
    .tools-grid {
      display: flex;
      flex-direction: column;
      gap: 0.5rem;
    }
    .tool-item {
      background: var(--bg-main);
      border: 1px solid var(--border);
      border-radius: var(--radius-md);
      padding: 0.65rem;
      display: flex;
      align-items: center;
      gap: 0.75rem;
      cursor: pointer;
      transition: all 0.15s ease;
    }
    .tool-item:hover {
      border-color: var(--border-hover);
      transform: scale(1.02);
    }
    .tool-icon {
      font-size: 1.15rem;
      width: 32px;
      height: 32px;
      border-radius: var(--radius-sm);
      background: rgba(255, 255, 255, 0.02);
      display: grid;
      place-items: center;
    }
    .tool-item strong {
      font-size: 0.78rem;
      color: var(--text-primary);
      display: block;
    }
    .tool-item span {
      font-size: 0.65rem;
      color: var(--text-muted);
      display: block;
    }
  </style>
</head>
<body>
  <!-- Ambient background glow elements -->
  <div class="ambient-glow" id="ambient-glow-1"></div>
  <div class="ambient-glow" id="ambient-glow-2"></div>
  <div class="ambient-glow" id="ambient-glow-3"></div>
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
        <div class="view-switch" aria-label="Workspace view">
          <button class="view-tab active" id="code-view-tab" type="button" data-view="code">Code</button>
          <button class="view-tab" id="instance-view-tab" type="button" data-view="instance">Instance</button>
          <button class="view-tab" id="automations-view-tab" type="button" data-view="automations">Automations</button>
        </div>
        <div class="usage-container" id="usage-container" style="display: none; align-items: center; gap: 0.75rem; font-size: 0.75rem; border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 0.3rem 0.6rem; background: rgba(255, 255, 255, 0.01);">
          <span style="color: var(--text-muted);">Session Usage:</span>
          <strong id="usage-text" style="color: var(--text-secondary);">0 / 12,000</strong>
          <div class="usage-bar-bg" style="width: 60px; height: 6px; background: rgba(255,255,255,0.08); border-radius: 99px; overflow: hidden; position: relative;">
            <div id="usage-bar-fill" style="width: 0%; height: 100%; background: var(--ok); border-radius: 99px; transition: width 0.3s ease, background 0.3s ease;"></div>
          </div>
          <span style="color: var(--text-muted); border-left: 1px solid var(--border); padding-left: 0.75rem;">Est. Cost:</span>
          <strong id="cost-text" style="color: var(--ok);">$0.0000</strong>
        </div>
        <div style="display: flex; align-items: center; gap: 0.75rem;">
          <div id="status" class="status-pill idle">Ready</div>
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
              <div style="display: flex; gap: 0.5rem;">
                <button id="editor-diff-btn" class="accent-btn" style="width: auto; padding: 0.25rem 0.75rem; font-size: 0.75rem; display: none; background: var(--bg-hover); color: var(--text); border: 1px solid var(--border);">Show Diff</button>
                <button id="editor-save-btn" class="accent-btn" style="width: auto; padding: 0.25rem 0.75rem; font-size: 0.75rem; display: none;">Save</button>
              </div>
            </div>
            
            <div class="editor-content-container" style="flex: 1; position: relative; display: flex; flex-direction: row; background: #050507; overflow: hidden;">
              <div id="editor-placeholder" style="position: absolute; top: 0; left: 0; width: 100%; height: 100%; display: flex; flex-direction: column; align-items: center; justify-content: center; color: var(--text-muted); gap: 0.75rem; background: var(--bg-card); z-index: 5;">
                <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z"></path><polyline points="14 2 14 8 20 8"></polyline><line x1="16" y1="13" x2="8" y2="13"></line><line x1="16" y1="17" x2="8" y2="17"></line><line x1="10" y1="9" x2="8" y2="9"></line></svg>
                <span style="font-size: 0.85rem;">Select a file from the sidebar to view and edit</span>
              </div>
              
              <div id="editor-line-numbers" style="display: none; width: 45px; padding: 1rem 0; text-align: right; color: var(--text-muted); font-family: var(--font-code); font-size: 0.85rem; line-height: 1.5; background: rgba(0,0,0,0.25); border-right: 1px solid var(--border); user-select: none; overflow-y: hidden; box-sizing: border-box; padding-right: 0.75rem;">1</div>
              <textarea id="editor-textarea" style="display: none; flex: 1; height: 100%; background: transparent; border: none; outline: none; resize: none; color: #a9b1d6; font-family: var(--font-code); font-size: 0.85rem; line-height: 1.5; padding: 1rem; box-sizing: border-box; overflow-y: auto; white-space: pre; overflow-wrap: normal;" spellcheck="false"></textarea>
              
              <div id="diff-container" style="display: none; flex: 1; height: 100%; flex-direction: row; background: #050507; overflow: hidden; width: 100%;">
                <div style="flex: 1; display: flex; flex-direction: column; height: 100%; overflow: hidden; border-right: 1px solid var(--border);">
                  <div style="background: rgba(255,255,255,0.03); padding: 0.25rem 0.5rem; font-size: 0.7rem; color: var(--text-muted); text-transform: uppercase; font-weight: bold; border-bottom: 1px solid var(--border);">Original File</div>
                  <pre id="diff-left" style="flex: 1; margin: 0; padding: 1rem; overflow: auto; font-family: var(--font-code); font-size: 0.85rem; line-height: 1.5; white-space: pre; color: #a9b1d6; box-sizing: border-box;"></pre>
                </div>
                <div style="flex: 1; display: flex; flex-direction: column; height: 100%; overflow: hidden;">
                  <div style="background: rgba(255,255,255,0.03); padding: 0.25rem 0.5rem; font-size: 0.7rem; color: var(--text-muted); text-transform: uppercase; font-weight: bold; border-bottom: 1px solid var(--border);">Improvements / Edited</div>
                  <pre id="diff-right" style="flex: 1; margin: 0; padding: 1rem; overflow: auto; font-family: var(--font-code); font-size: 0.85rem; line-height: 1.5; white-space: pre; color: #a9b1d6; box-sizing: border-box;"></pre>
                </div>
              </div>
            </div>
          </div>
        </div>
        <section class="instance-page" id="instance-page" hidden>
          <div class="instance-stage" aria-label="Running Clawie instance">
            <div class="instance-titlebar">
              <span>Pixel Agents</span>
              <div class="zoom-stack">
                <button class="zoom-btn" id="instance-zoom-in" type="button" title="Zoom in">+</button>
                <button class="zoom-btn" id="instance-zoom-out" type="button" title="Zoom out">-</button>
              </div>
            </div>
            <div class="pixel-map">
              <div class="instance-room-grid" id="instance-room-grid"></div>
            </div>
          </div>
          <aside class="instance-panel">
            <div class="instance-panel-header">
              <h2>Running Instance</h2>
              <p>Local Clawie WebUI server and workspace status.</p>
            </div>
            <div class="instance-metrics">
              <div class="metric-row">
                <div class="metric-label">Server</div>
                <div class="metric-value live">Live on 127.0.0.1</div>
              </div>
              <div class="metric-row">
                <div class="metric-label">Mode</div>
                <div class="metric-value">Local browser workspace</div>
              </div>
              <div class="metric-row">
                <div class="metric-label">Folder</div>
                <div class="metric-value" id="instance-folder">Choose a save location</div>
              </div>
              <div class="metric-row">
                <div class="metric-label">Open File</div>
                <div class="metric-value" id="instance-file">No file open</div>
              </div>
              <div class="metric-row">
                <div class="metric-label">Model</div>
                <div class="metric-value" id="instance-model">gpt-4.1</div>
              </div>
              <div class="metric-row">
                <div class="metric-label">Usage</div>
                <div class="metric-value" id="instance-usage">0 tokens · $0.0000</div>
              </div>
              <div class="metric-row">
                <div class="instance-list-header">
                  <div>
                    <div class="metric-label">Open Clawie Instances</div>
                    <div class="metric-value" id="instance-count">Scanning...</div>
                  </div>
                  <button class="instance-refresh" id="instance-refresh" type="button">Refresh</button>
                </div>
              </div>
              <div class="process-list" id="process-list">
                <div class="process-empty">Scanning this computer for open Clawie CLI sessions...</div>
              </div>
            </div>
          </aside>
        </section>
        <section class="automations-page" id="automations-page" hidden>
          <div class="automation-builder-container">
            <!-- Left Panel: Flow Canvas -->
            <div class="automation-canvas">
              <div class="canvas-header">
                <div style="display: flex; flex-direction: column; gap: 0.25rem;">
                  <h3>Automation Flow Canvas</h3>
                  <span class="canvas-subtitle">Drag blocks to position, pan/zoom canvas, and click any block to configure settings.</span>
                </div>
                <div class="canvas-actions">
                  <button class="settings-btn-secondary" id="auto-btn-clear" type="button" style="padding: 0.35rem 0.75rem; font-size: 0.75rem;">Reset Flow</button>
                  <div class="dropdown-container" style="position: relative; display: inline-block;">
                    <button class="settings-btn-save" id="auto-btn-save" type="button" style="padding: 0.35rem 0.75rem; font-size: 0.75rem; display: flex; align-items: center; gap: 0.25rem;">
                      Save Workflow <span style="font-size: 0.6rem;">▼</span>
                    </button>
                    <div class="dropdown-menu" id="workflow-save-dropdown" style="display: none; position: absolute; right: 0; top: 100%; margin-top: 0.5rem; background: var(--bg-main); border: 1px solid var(--border); border-radius: var(--radius-md); box-shadow: var(--panel-shadow); z-index: 100; min-width: 160px; overflow: hidden;">
                      <button class="dropdown-item" type="button" id="btn-save-json" style="width: 100%; text-align: left; background: none; border: none; padding: 0.6rem 1rem; color: var(--text-primary); font-size: 0.75rem; cursor: pointer; transition: background 0.2s; display: flex; align-items: center; gap: 0.5rem;">
                        📁 Save as JSON File
                      </button>
                      <button class="dropdown-item" type="button" id="btn-save-clawie" style="width: 100%; text-align: left; background: none; border: none; padding: 0.6rem 1rem; color: var(--text-primary); font-size: 0.75rem; cursor: pointer; transition: background 0.2s; display: flex; align-items: center; gap: 0.5rem; border-top: 1px solid var(--border);">
                        🧠 Save in Clawie
                      </button>
                    </div>
                  </div>
                </div>
              </div>
              
              <!-- 2D Canvas Workspace -->
              <div class="automation-workspace-container" style="position: relative; width: 100%; height: 580px; background: #070708; border: 1px solid var(--border); border-radius: var(--radius-lg); overflow: hidden; user-select: none;">
                <!-- Grid background overlay -->
                <div id="canvas-grid-overlay" style="position: absolute; top: 0; left: 0; width: 100%; height: 100%; pointer-events: auto; background-image: radial-gradient(rgba(255, 255, 255, 0.05) 1px, transparent 0); background-size: 20px 20px; z-index: 0; cursor: grab;"></div>
                
                <div class="automation-canvas-viewport" id="canvas-viewport" style="width: 100%; height: 100%; overflow: hidden; position: relative;">
                  <!-- SVG Connection Layer -->
                  <svg id="connection-svg" style="position: absolute; top: 0; left: 0; width: 3000px; height: 3000px; pointer-events: none; z-index: 1; transform-origin: 0 0; transition: transform 0.05s ease-out;">
                  </svg>
                  
                  <!-- Draggable Node Layer -->
                  <div id="nodes-layer" style="position: absolute; width: 3000px; height: 3000px; top: 0; left: 0; transform-origin: 0 0; z-index: 2; transition: transform 0.05s ease-out;">
                    <!-- Trigger Node -->
                    <div class="flow-node trigger-node" id="trigger-node-main" style="position: absolute; left: 50px; top: 120px;">
                      <div class="node-header">
                        <div class="node-icon">⚡</div>
                        <div class="node-title-wrap">
                          <span class="node-label">TRIGGER</span>
                          <strong class="node-name" id="auto-trigger-name">On File Save</strong>
                        </div>
                      </div>
                      <div class="node-body" id="auto-trigger-body">
                        <label>File Glob Path Match</label>
                        <input type="text" value="**/*.rs" placeholder="e.g. **/*.rs" id="auto-input-trigger-glob">
                      </div>
                    </div>

                    <!-- Action Node 1 -->
                    <div class="flow-node action-node ai-agent-node" id="action-node-1" style="position: absolute; left: 360px; top: 120px;">
                      <div class="node-header">
                        <div class="node-icon" style="background: rgba(59, 130, 246, 0.1); color: #3b82f6;">🤖</div>
                        <div class="node-title-wrap">
                          <span class="node-label" style="color: #3b82f6;">ACTION (AI RUNNER)</span>
                          <strong class="node-name">Clawie Code Auditor</strong>
                        </div>
                        <button class="node-remove-btn" type="button" onclick="removeActionNode('action-node-1')">&times;</button>
                      </div>
                      <div class="node-body">
                        <label>AI Prompt Goal</label>
                        <textarea rows="2" placeholder="Explain the goal for the agent..." class="auto-node-val">Audits the saved Rust file for code quality issues and ensures it complies with CLAWIE.md coding standards.</textarea>
                        
                        <!-- Expandable Settings -->
                        <details class="node-settings-details" style="margin-top: 0.5rem; font-size: 0.7rem; border-top: 1px dashed var(--border); padding-top: 0.5rem;">
                          <summary style="cursor: pointer; color: var(--text-muted); user-select: none;">⚙️ Fail-safe Guards & Settings</summary>
                          <div style="margin-top: 0.35rem; display: flex; flex-direction: column; gap: 0.25rem; padding-left: 0.5rem;">
                            <label style="display: flex; align-items: center; gap: 0.35rem; color: var(--text-muted);">
                              <input type="checkbox" class="node-opt-retry" style="margin: 0;"> Auto-retry on failure (up to 3 times)
                            </label>
                            <label style="display: flex; align-items: center; gap: 0.35rem; color: var(--text-muted);">
                              <input type="checkbox" class="node-opt-continue" style="margin: 0;"> Continue workflow on error
                            </label>
                          </div>
                        </details>

                        <!-- Node Simulation & Payload Inspector -->
                        <div class="node-simulator" style="margin-top: 0.5rem; display: flex; flex-direction: column; gap: 0.25rem; border-top: 1px dashed var(--border); padding-top: 0.5rem;">
                          <div style="display: flex; justify-content: space-between; align-items: center;">
                            <span style="font-size: 0.7rem; color: var(--text-muted);">🔬 Payload Inspector</span>
                            <button class="node-sim-btn" type="button" style="padding: 0.15rem 0.4rem; font-size: 0.65rem; background: var(--bg-main); border: 1px solid var(--border); border-radius: var(--radius-sm); cursor: pointer; color: var(--text-primary); transition: all 0.2s;" onclick="simulateNode('action-node-1')">▶️ Simulate Node</button>
                          </div>
                          <div class="node-inspector-payload" style="display: none; background: rgba(0,0,0,0.2); border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 0.4rem; font-family: monospace; font-size: 0.65rem; color: #10b981; max-height: 100px; overflow-y: auto; margin-top: 0.25rem;">
                          </div>
                        </div>
                      </div>
                    </div>
                  </div>
                </div>
                
                <!-- Zoom Controls Overlay -->
                <div style="position: absolute; bottom: 1rem; left: 1rem; z-index: 10; display: flex; gap: 0.35rem;">
                  <button class="settings-btn-secondary" onclick="adjustZoom(0.1)" type="button" style="padding: 0.4rem 0.6rem; font-size: 0.7rem; font-weight: bold; background: var(--bg-card);">➕ Zoom In</button>
                  <button class="settings-btn-secondary" onclick="adjustZoom(-0.1)" type="button" style="padding: 0.4rem 0.6rem; font-size: 0.7rem; font-weight: bold; background: var(--bg-card);">➖ Zoom Out</button>
                  <button class="settings-btn-secondary" onclick="resetZoom()" type="button" style="padding: 0.4rem 0.6rem; font-size: 0.7rem; font-weight: bold; background: var(--bg-card);">🔄 Center</button>
                </div>
              </div>
            </div>

            <!-- Right Panel: Toolbox -->
            <div class="automation-toolbox">
              <h3>Workflow Toolbox</h3>
              <p class="toolbox-desc">Click template blocks below to inject them into the active canvas.</p>
              
              <!-- Preset Templates -->
              <div class="toolbox-section">
                <h4>1. Preset Quick Flows</h4>
                <div class="template-presets-grid">
                  <button class="template-chip" type="button" id="tpl-code-guard">
                    <strong>🛡️ Code Guard</strong>
                    <span>Test and fix on save</span>
                  </button>
                  <button class="template-chip" type="button" id="tpl-auto-sync">
                    <strong>🔄 Daily Commit Sync</strong>
                    <span>Auto-commit summaries</span>
                  </button>
                  <button class="template-chip" type="button" id="tpl-email-responder">
                    <strong>📧 AI Email Support</strong>
                    <span>Auto-answer support queries</span>
                  </button>
                  <button class="template-chip" type="button" id="tpl-custom-agent">
                    <strong>🧠 AI Custom Agent</strong>
                    <span>Provision simple support agent</span>
                  </button>
                  <button class="template-chip" type="button" id="tpl-webhook-http">
                    <strong>🔌 API Webhook Sync</strong>
                    <span>Webhook triggering REST call</span>
                  </button>
                  <button class="template-chip" type="button" id="tpl-cron-runner">
                    <strong>⏱️ Cron Runner</strong>
                    <span>Timed folder tasks</span>
                  </button>
                </div>
              </div>

              <!-- Available Triggers -->
              <div class="toolbox-section" style="margin-top: 1.25rem;">
                <h4>2. Triggers</h4>
                <div class="tools-grid">
                  <div class="tool-item" id="tool-trigger-file">
                    <span class="tool-icon">📁</span>
                    <div>
                      <strong>On File Save</strong>
                      <span>Triggers on file edits</span>
                    </div>
                  </div>
                  <div class="tool-item" id="tool-trigger-cron">
                    <span class="tool-icon">⏱️</span>
                    <div>
                      <strong>Cron Schedule</strong>
                      <span>Interval-based task</span>
                    </div>
                  </div>
                  <div class="tool-item" id="tool-trigger-git">
                    <span class="tool-icon">🐙</span>
                    <div>
                      <strong>Git Pre-commit</strong>
                      <span>Guard local commits</span>
                    </div>
                  </div>
                  <div class="tool-item" id="tool-trigger-webhook">
                    <span class="tool-icon">🔌</span>
                    <div>
                      <strong>Webhook Endpoint</strong>
                      <span>Trigger via HTTP requests</span>
                    </div>
                  </div>
                </div>
              </div>

              <!-- Available Actions -->
              <div class="toolbox-section" style="margin-top: 1.25rem;">
                <h4>3. Actions</h4>
                <div class="tools-grid">
                  <div class="tool-item" id="tool-action-agent">
                    <span class="tool-icon" style="color: #3b82f6;">🤖</span>
                    <div>
                      <strong>Clawie AI Agent</strong>
                      <span>Instruct autonomous coder</span>
                    </div>
                  </div>
                  <div class="tool-item" id="tool-action-custom-agent">
                    <span class="tool-icon" style="color: #8b5cf6;">🧠</span>
                    <div>
                      <strong>Create Simple Agent</strong>
                      <span>Define custom persona & tasks</span>
                    </div>
                  </div>
                  <div class="tool-item" id="tool-action-email">
                    <span class="tool-icon" style="color: #ea580c;">📧</span>
                    <div>
                      <strong>Answer Email</strong>
                      <span>Auto-respond with AI draft</span>
                    </div>
                  </div>
                  <div class="tool-item" id="tool-action-summarize">
                    <span class="tool-icon" style="color: #06b6d4;">📝</span>
                    <div>
                      <strong>Summarize Text</strong>
                      <span>Extract highlights or items</span>
                    </div>
                  </div>
                  <div class="tool-item" id="tool-action-slack">
                    <span class="tool-icon" style="color: #ec4899;">💬</span>
                    <div>
                      <strong>Draft Slack Message</strong>
                      <span>Send alerts to Slack channels</span>
                    </div>
                  </div>
                  <div class="tool-item" id="tool-action-http">
                    <span class="tool-icon" style="color: #38bdf8;">🌐</span>
                    <div>
                      <strong>API Request</strong>
                      <span>Call external REST APIs</span>
                    </div>
                  </div>
                  <div class="tool-item" id="tool-action-nested">
                    <span class="tool-icon" style="color: #fb7185;">📦</span>
                    <div>
                      <strong>Nested Flow</strong>
                      <span>Invoke another task-flow</span>
                    </div>
                  </div>
                  <div class="tool-item" id="tool-action-routing">
                    <span class="tool-icon" style="color: #fbbf24;">🔀</span>
                    <div>
                      <strong>Routing Guard</strong>
                      <span>Conditional branch splitter</span>
                    </div>
                  </div>
                  <div class="tool-item" id="tool-action-iterator">
                    <span class="tool-icon" style="color: #34d399;">🔄</span>
                    <div>
                      <strong>Batch Iterator</strong>
                      <span>Loop over items or files</span>
                    </div>
                  </div>
                  <div class="tool-item" id="tool-action-bash">
                    <span class="tool-icon" style="color: #10b981;">💻</span>
                    <div>
                      <strong>Shell Script</strong>
                      <span>Execute a terminal command</span>
                    </div>
                  </div>
                  <div class="tool-item" id="tool-action-notify">
                    <span class="tool-icon" style="color: #f59e0b;">✉️</span>
                    <div>
                      <strong>Send Notification</strong>
                      <span>Emit system sound or alert</span>
                    </div>
                  </div>
                  <div class="tool-item" id="tool-action-sticky">
                    <span class="tool-icon" style="color: #fde047;">📌</span>
                    <div>
                      <strong>Add Sticky Note</strong>
                      <span>Annotate your flow canvas</span>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>
          <!-- Secret coming soon mode overlay removed -->
          
          <!-- Slide-out Configuration Drawer Panel -->
          <div id="node-config-drawer" style="position: absolute; top: 0; right: -400px; width: 380px; height: 100%; background: var(--bg-card); border-left: 1px solid var(--border); z-index: 1000; transition: right 0.3s cubic-bezier(0.4, 0, 0.2, 1); box-shadow: -10px 0 30px rgba(0,0,0,0.6); padding: 1.5rem; display: flex; flex-direction: column; gap: 1rem; overflow-y: auto;">
            <div style="display: flex; justify-content: space-between; align-items: center; border-bottom: 1px solid var(--border); padding-bottom: 0.75rem;">
              <h3 id="drawer-node-title" style="margin: 0; font-size: 1.1rem; color: var(--text-primary);">Configure Node</h3>
              <button onclick="closeDrawer()" type="button" style="background: none; border: none; font-size: 1.5rem; color: var(--text-muted); cursor: pointer; transition: color 0.2s;" onmouseover="this.style.color='var(--text-primary)'" onmouseout="this.style.color='var(--text-muted)'">&times;</button>
            </div>
            <div id="drawer-node-body" style="flex: 1; display: flex; flex-direction: column; gap: 1rem;">
              <!-- Config inputs go here -->
            </div>
            <div style="display: flex; gap: 0.5rem; justify-content: flex-end; border-top: 1px solid var(--border); padding-top: 0.75rem;">
              <button onclick="closeDrawer()" type="button" class="settings-btn-secondary" style="padding: 0.5rem 1rem; font-size: 0.75rem;">Cancel</button>
              <button onclick="saveDrawerSettings()" type="button" class="settings-btn-save" style="padding: 0.5rem 1rem; font-size: 0.75rem;">Apply Changes</button>
            </div>
          </div>
        </section>
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

  <div id="agent-context-menu" class="agent-menu" hidden>
    <button type="button" data-agent-action="logs">Open logs</button>
    <button type="button" data-agent-action="copy-pid">Copy PID</button>
    <button type="button" data-agent-action="refresh">Refresh instances</button>
    <button type="button" data-agent-action="terminate" class="danger">Terminate CLI</button>
  </div>

  <!-- Settings Modal Overlay -->
  <div id="settings-modal" class="modal-overlay" hidden>
    <div class="modal-content">
      <div class="modal-header" style="border-bottom: none; padding-bottom: 0.5rem;">
        <h3>Settings</h3>
        <button id="settings-close" class="close-btn">&times;</button>
      </div>
      
      <!-- Tab Buttons Header -->
      <div class="settings-tabs">
        <button class="settings-tab-btn active" type="button" data-tab="tab-appearance">Appearance</button>
        <button class="settings-tab-btn" type="button" data-tab="tab-provider">AI Provider</button>
        <button class="settings-tab-btn" type="button" data-tab="tab-limits">Lean & Limits</button>
      </div>

      <div class="modal-body">
        <!-- Appearance Tab Pane -->
        <div id="tab-appearance" class="settings-tab-content active">
          <div class="settings-pane-grid">
            <div class="settings-group">
              <label for="settings-app-theme">App Theme</label>
              <select id="settings-app-theme">
                <option value="dark">Dark</option>
                <option value="light">Light</option>
                <option value="graphite">Graphite</option>
                <option value="contrast">High Contrast</option>
                <option value="tokyo-night">Tokyo Night</option>
                <option value="dracula">Dracula</option>
                <option value="nord">Nord</option>
                <option value="synthwave">Synthwave '84</option>
              </select>
            </div>
            <div class="settings-group">
              <label>Accent Color</label>
              <div style="display: flex; flex-direction: column; gap: 0.5rem;">
                <div class="theme-options">
                  <button class="theme-opt orange active" type="button" data-color="orange" title="Orange"></button>
                  <button class="theme-opt blue" type="button" data-color="blue" title="Blue"></button>
                  <button class="theme-opt purple" type="button" data-color="purple" title="Purple"></button>
                  <button class="theme-opt green" type="button" data-color="green" title="Green"></button>
                </div>
                <div style="display: flex; align-items: center; gap: 0.5rem; margin-top: 0.25rem;">
                  <input type="checkbox" id="settings-custom-accent-toggle">
                  <label for="settings-custom-accent-toggle" style="font-size: 0.65rem; text-transform: none; font-weight: normal; margin: 0; color: var(--text-muted);">Custom Color</label>
                  <input type="color" id="settings-custom-accent-color" value="#f97316" style="width: 28px; height: 20px; padding: 0; border: none; border-radius: 3px; cursor: pointer; background: transparent;">
                </div>
              </div>
            </div>
            <div class="settings-group">
              <label for="settings-font-ui">UI Font</label>
              <select id="settings-font-ui">
                <option value="inter">Inter (Default)</option>
                <option value="outfit">Outfit</option>
                <option value="fira-sans">Fira Sans</option>
                <option value="system">System UI</option>
              </select>
            </div>
            <div class="settings-group">
              <label for="settings-font-code">Code Editor Font</label>
              <select id="settings-font-code">
                <option value="jetbrains">JetBrains Mono</option>
                <option value="fira-code">Fira Code</option>
                <option value="source-code">Source Code Pro</option>
                <option value="monospace">System Monospace</option>
              </select>
            </div>
            <div class="settings-group">
              <label for="settings-layout-density">Layout Density</label>
              <select id="settings-layout-density">
                <option value="comfortable">Comfortable</option>
                <option value="compact">Compact</option>
              </select>
            </div>
            <div class="settings-group">
              <label for="settings-sidebar-layout">Sidebar Layout</label>
              <select id="settings-sidebar-layout">
                <option value="expanded">Expanded Sidebar</option>
                <option value="slim">Slim Sidebar</option>
              </select>
            </div>
            <div class="settings-group settings-wide" style="display: flex; flex-direction: column; gap: 0.5rem;">
              <label>Effects & Animations</label>
              <div style="display: flex; flex-direction: column; gap: 0.5rem; margin-top: 0.25rem;">
                <div style="display: flex; align-items: center; gap: 0.5rem;">
                  <input type="checkbox" id="settings-ambient-glow" checked>
                  <label for="settings-ambient-glow" style="font-size: 0.72rem; text-transform: none; font-weight: normal; margin: 0; color: var(--text-secondary);">Ambient Background Glow Orbs</label>
                </div>
                <div style="display: flex; align-items: center; gap: 0.5rem;">
                  <input type="checkbox" id="settings-terminal-glow">
                  <label for="settings-terminal-glow" style="font-size: 0.72rem; text-transform: none; font-weight: normal; margin: 0; color: var(--text-secondary);">Terminal CRT Glow & Scanlines</label>
                </div>
                <div style="display: flex; align-items: center; gap: 0.5rem;">
                  <input type="checkbox" id="settings-status-pulsate" checked>
                  <label for="settings-status-pulsate" style="font-size: 0.72rem; text-transform: none; font-weight: normal; margin: 0; color: var(--text-secondary);">Pulsating Status Indicator Aura</label>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- AI Provider Tab Pane -->
        <div id="tab-provider" class="settings-tab-content">
          <div class="settings-pane-grid">
            <div class="settings-group">
              <label for="settings-provider">Active Provider</label>
              <select id="settings-provider">
                <option value="anthropic">Anthropic (Claude)</option>
                <option value="openai">OpenAI (ChatGPT)</option>
                <option value="gemini">Google (Gemini)</option>
                <option value="xai">xAI (Grok)</option>
                <option value="kimi">Moonshot AI (Kimi)</option>
              </select>
            </div>
            <div class="settings-group">
              <label for="settings-model">Active AI Model</label>
              <select id="settings-model">
                <option value="claude-3-5-sonnet">claude-3-5-sonnet</option>
                <option value="gpt-4o">gpt-4o</option>
                <option value="gemini-1.5-pro">gemini-1.5-pro</option>
                <option value="gemini-1.5-flash">gemini-1.5-flash</option>
                <option value="gemini-2.0-pro">gemini-2.0-pro</option>
                <option value="gemini-2.0-flash">gemini-2.0-flash</option>
                <option value="gemini-3.5-flash">gemini-3.5-flash</option>
                <option value="grok-3">grok-3</option>
                <option value="grok-2">grok-2</option>
                <option value="moonshot-v1-auto">moonshot-v1-auto</option>
                <option value="moonshot-v1-32k">moonshot-v1-32k</option>
                <option value="moonshot-v1-128k">moonshot-v1-128k</option>
                <option value="gpt-4.1">gpt-4.1</option>
              </select>
            </div>
            <div class="settings-group settings-wide">
              <label>Connections</label>
              <div style="display: flex; flex-direction: column; gap: 0.75rem; margin-top: 0.25rem;">
                <div style="display: flex; flex-direction: column; gap: 0.25rem;">
                  <label for="settings-openai-key" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">OpenAI API Key</label>
                  <div class="password-input-wrapper">
                    <input id="settings-openai-key" type="password" placeholder="sk-..." autocomplete="off">
                    <button class="password-toggle-btn" type="button" data-target="settings-openai-key" title="Toggle visibility">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path><circle cx="12" cy="12" r="3"></circle></svg>
                    </button>
                  </div>
                </div>
                <div style="display: flex; flex-direction: column; gap: 0.25rem;">
                  <label for="settings-anthropic-key" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">Anthropic API Key</label>
                  <div class="password-input-wrapper">
                    <input id="settings-anthropic-key" type="password" placeholder="sk-ant-..." autocomplete="off">
                    <button class="password-toggle-btn" type="button" data-target="settings-anthropic-key" title="Toggle visibility">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path><circle cx="12" cy="12" r="3"></circle></svg>
                    </button>
                  </div>
                </div>
                <div style="display: flex; flex-direction: column; gap: 0.25rem;">
                  <label for="settings-gemini-key" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">Gemini API Key</label>
                  <div class="password-input-wrapper">
                    <input id="settings-gemini-key" type="password" placeholder="AIzaSy..." autocomplete="off">
                    <button class="password-toggle-btn" type="button" data-target="settings-gemini-key" title="Toggle visibility">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path><circle cx="12" cy="12" r="3"></circle></svg>
                    </button>
                  </div>
                </div>
                <div style="display: flex; flex-direction: column; gap: 0.25rem;">
                  <label for="settings-openai-url" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">Custom OpenAI Base URL (optional)</label>
                  <input id="settings-openai-url" placeholder="https://api.openai.com/v1" autocomplete="off">
                </div>
                <div style="display: flex; flex-direction: column; gap: 0.25rem;">
                  <label for="settings-gemini-url" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">Custom Gemini Base URL (optional)</label>
                  <input id="settings-gemini-url" placeholder="https://generativelanguage.googleapis.com/v1beta/openai" autocomplete="off">
                </div>
                <div style="display: flex; flex-direction: column; gap: 0.25rem;">
                  <label for="settings-xai-key" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">xAI API Key (Grok)</label>
                  <div class="password-input-wrapper">
                    <input id="settings-xai-key" type="password" placeholder="xai-..." autocomplete="off">
                    <button class="password-toggle-btn" type="button" data-target="settings-xai-key" title="Toggle visibility">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path><circle cx="12" cy="12" r="3"></circle></svg>
                    </button>
                  </div>
                </div>
                <div style="display: flex; flex-direction: column; gap: 0.25rem;">
                  <label for="settings-xai-url" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">Custom xAI Base URL (optional)</label>
                  <input id="settings-xai-url" placeholder="https://api.x.ai/v1" autocomplete="off">
                </div>
                <div style="display: flex; flex-direction: column; gap: 0.25rem;">
                  <label for="settings-kimi-key" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">Moonshot API Key (Kimi)</label>
                  <div class="password-input-wrapper">
                    <input id="settings-kimi-key" type="password" placeholder="sk-..." autocomplete="off">
                    <button class="password-toggle-btn" type="button" data-target="settings-kimi-key" title="Toggle visibility">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path><circle cx="12" cy="12" r="3"></circle></svg>
                    </button>
                  </div>
                </div>
                <div style="display: flex; flex-direction: column; gap: 0.25rem;">
                  <label for="settings-kimi-url" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">Custom Moonshot Base URL (optional)</label>
                  <input id="settings-kimi-url" placeholder="https://api.moonshot.cn/v1" autocomplete="off">
                </div>
                <div style="display: flex; align-items: center; gap: 1rem; margin-top: 0.25rem;">
                  <button id="settings-test-conn-btn" class="settings-btn-secondary" type="button" style="padding: 0.35rem 0.75rem; font-size: 0.75rem;">Test Connection</button>
                  <div id="settings-conn-status-container" class="settings-connection-status" style="display: none;">
                    <span class="status-dot"></span>
                    <span id="settings-conn-status-text">Disconnected</span>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- Lean & Limits Tab Pane -->
        <div id="tab-limits" class="settings-tab-content">
          <div class="settings-pane-grid">
            <div class="settings-group">
              <label for="settings-lean-mode">Lean Mode Level</label>
              <select id="settings-lean-mode">
                <option value="full">Full (Enforced)</option>
                <option value="lite">Lite (Mild checks)</option>
                <option value="ultra">Ultra (Strict constraints)</option>
                <option value="off">Off (Allow exploration)</option>
              </select>
            </div>
            <div class="settings-group">
              <label for="settings-max-turns">Max Turns: <span id="max-turns-val" style="color: var(--accent); font-weight: bold;">64</span></label>
              <input type="range" id="settings-max-turns" min="1" max="150" value="64" style="margin-top: 0.25rem;">
            </div>
            <div class="settings-group">
              <label for="settings-token-budget">Token Budget Constraints</label>
              <input type="number" id="settings-token-budget" min="1" max="100000" placeholder="12000" style="width: 100%;">
            </div>
            <div class="settings-group settings-wide" style="min-height: 0;">
              <label>Web App</label>
              <button id="settings-install-app" class="settings-btn-secondary" type="button" style="align-self: flex-start;">Install Web App</button>
              <p id="settings-install-status" class="settings-help" style="margin-top: 0.25rem;">Install Clawie as a browser app for quick access from your dock or app launcher. We will launch an IDE soon.</p>
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

  <div id="instance-log-modal" class="modal-overlay" hidden>
    <div class="modal-content">
      <div class="modal-header">
        <h3 id="instance-log-title">Instance Logs</h3>
        <button id="instance-log-close" class="close-btn">&times;</button>
      </div>
      <div class="log-body" id="instance-log-body">
        <div class="log-line">Select a room PC to inspect that Clawie instance.</div>
      </div>
    </div>
  <!-- Save Path Modal -->
  <div id="save-path-modal" class="modal-overlay" hidden>
    <div class="modal-content" style="width: min(400px, 95%); padding: 0; background: var(--bg-card); border: 1px solid var(--border); border-radius: var(--radius-lg); box-shadow: var(--panel-shadow); overflow: hidden;">
      <div class="modal-header" style="padding: 1.25rem; border-bottom: 1px solid var(--border); display: flex; justify-content: space-between; align-items: center;">
        <strong style="font-size: 0.9rem; display: flex; align-items: center; gap: 0.5rem; color: var(--text-primary);">📁 Save JSON Workflow</strong>
        <button style="background: none; border: none; font-size: 1.2rem; color: var(--text-muted); cursor: pointer;" id="save-path-cancel-x">&times;</button>
      </div>
      <div class="modal-body" style="padding: 1.25rem; display: flex; flex-direction: column; gap: 0.75rem;">
        <label style="font-size: 0.75rem; color: var(--text-secondary); display: block; margin-bottom: 0.25rem;">Select destination:</label>
        <select id="save-destination-select" style="width: 100%; padding: 0.6rem 0.75rem; background: var(--bg-main); border: 1px solid var(--border); border-radius: var(--radius-sm); color: var(--text-primary); font-size: 0.75rem; outline: none; margin-bottom: 0.25rem;">
          <option value="workspace">Clawie Workspace</option>
          <option value="downloads">Browser Downloads</option>
        </select>
        
        <div id="save-workspace-options">
          <label style="font-size: 0.75rem; color: var(--text-secondary); display: block; margin-bottom: 0.25rem; margin-top: 0.5rem;">Destination path (relative to workspace root):</label>
          <input type="text" id="save-path-input" value="workflow.json" style="width: 100%; padding: 0.6rem 0.75rem; background: var(--bg-main); border: 1px solid var(--border); border-radius: var(--radius-sm); color: var(--text-primary); font-size: 0.75rem; outline: none; transition: border-color 0.2s;">
          <div style="font-size: 0.68rem; color: var(--text-muted); line-height: 1.3; margin-top: 0.5rem;">
            Workspace Root: <code id="modal-workspace-path" style="color: var(--text-secondary); word-break: break-all;"></code>
          </div>
        </div>
      </div>
      <div class="modal-footer" style="padding: 1rem 1.25rem; background: rgba(0,0,0,0.1); border-top: 1px solid var(--border); display: flex; justify-content: flex-end; gap: 0.75rem;">
        <button type="button" id="save-path-cancel" class="settings-btn-secondary" style="padding: 0.4rem 1rem; font-size: 0.75rem; cursor: pointer; transition: all 0.2s;">Cancel</button>
        <button type="button" id="save-path-confirm" class="settings-btn-save" style="padding: 0.4rem 1rem; font-size: 0.75rem; cursor: pointer; transition: all 0.2s;">Save</button>
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
    const codeViewTab = document.querySelector('#code-view-tab');
    const instanceViewTab = document.querySelector('#instance-view-tab');
    const automationsViewTab = document.querySelector('#automations-view-tab');
    const workspaceContentWrap = document.querySelector('.workspace-content-wrap');
    const instancePage = document.querySelector('#instance-page');
    const automationsPage = document.querySelector('#automations-page');
    const instanceFolder = document.querySelector('#instance-folder');
    const instanceFile = document.querySelector('#instance-file');
    const instanceModel = document.querySelector('#instance-model');
    const instanceUsage = document.querySelector('#instance-usage');
    const instanceCount = document.querySelector('#instance-count');
    const instanceRefresh = document.querySelector('#instance-refresh');
    const processList = document.querySelector('#process-list');
    const instanceRoomGrid = document.querySelector('#instance-room-grid');
    const instanceZoomIn = document.querySelector('#instance-zoom-in');
    const instanceZoomOut = document.querySelector('#instance-zoom-out');
    const instanceLogModal = document.querySelector('#instance-log-modal');
    const instanceLogClose = document.querySelector('#instance-log-close');
    const instanceLogTitle = document.querySelector('#instance-log-title');
    const instanceLogBody = document.querySelector('#instance-log-body');
    const agentContextMenu = document.querySelector('#agent-context-menu');

    const editorDiffBtn = document.querySelector('#editor-diff-btn');
    const diffContainer = document.querySelector('#diff-container');
    const diffLeft = document.querySelector('#diff-left');
    const diffRight = document.querySelector('#diff-right');

    let activeFileName = null;
    let originalCode = '';
    let improvementsCode = '';
    let logWebSocket = null;
    let selectedAgentContext = null;

    let totalInputTokens = 0;
    let totalOutputTokens = 0;
    let totalCost = 0.0;
    const maxTokensLimit = 12000;
    let knownInstanceRooms = loadKnownInstanceRooms();
    let instanceZoom = Number(localStorage.getItem('clawie-instance-zoom') || '1');
    const minInstanceZoom = 0.7;
    const maxInstanceZoom = 1.6;
    const instanceZoomStep = 0.1;

    const statusStates = new Set(['idle', 'busy', 'thinking', 'uploading', 'listening', 'saved', 'unsaved', 'error']);

    function setStatus(message, state = 'idle') {
      const nextState = statusStates.has(state) ? state : 'idle';
      status.textContent = message;
      status.className = 'status-pill ' + nextState;
      syncInstancePanel();
    }

    function syncInstancePanel() {
      const modelSetting = document.querySelector('#settings-model');
      const totalUsed = totalInputTokens + totalOutputTokens;
      if (instanceFolder) instanceFolder.textContent = locationPath.value || 'Choose a save location';
      if (instanceFile) instanceFile.textContent = activeFileName || 'No file open';
      if (instanceModel) instanceModel.textContent = modelSetting?.value || localStorage.getItem('clawie-model-setting') || 'gpt-4.1';
      if (instanceUsage) instanceUsage.textContent = `${totalUsed.toLocaleString()} tokens · $${totalCost.toFixed(4)}`;
    }

    function setWorkspaceView(view) {
      const showInstance = view === 'instance';
      const showAutomations = view === 'automations';
      workspaceContentWrap.hidden = showInstance || showAutomations;
      instancePage.hidden = !showInstance;
      automationsPage.hidden = !showAutomations;
      codeViewTab.classList.toggle('active', !showInstance && !showAutomations);
      instanceViewTab.classList.toggle('active', showInstance);
      automationsViewTab.classList.toggle('active', showAutomations);
      localStorage.setItem('clawie-workspace-view', showAutomations ? 'automations' : showInstance ? 'instance' : 'code');
      syncInstancePanel();
      if (showInstance) refreshInstances();
    }

    function clampInstanceZoom(value) {
      const numeric = Number(value);
      if (!Number.isFinite(numeric)) return 1;
      return Math.max(minInstanceZoom, Math.min(maxInstanceZoom, numeric));
    }

    function setInstanceZoom(nextZoom) {
      instanceZoom = clampInstanceZoom(nextZoom);
      if (instanceRoomGrid) {
        instanceRoomGrid.style.setProperty('--instance-zoom', instanceZoom.toFixed(2));
      }
      if (instanceZoomIn) instanceZoomIn.disabled = instanceZoom >= maxInstanceZoom;
      if (instanceZoomOut) instanceZoomOut.disabled = instanceZoom <= minInstanceZoom;
      localStorage.setItem('clawie-instance-zoom', instanceZoom.toFixed(2));
    }

    function escapeText(value) {
      return String(value ?? '')
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;');
    }

    async function refreshInstances() {
      if (!processList || !instanceCount) return;
      try {
        const response = await fetch('/instances');
        const result = await response.json();
        if (!response.ok || !result.ok) throw new Error(result.error || 'Could not scan instances');
        const instances = result.instances || [];

        // Add virtual Automation Flow instance if active in last 1 hour
        const lastActivity = Number(localStorage.getItem('clawie-last-automation-activity') || 0);
        const isAutomationRecent = (Date.now() - lastActivity) < 3600000; // 1 hour
        const isAutomationRunning = (Date.now() - lastActivity) < 300000; // 5 minutes
        
        if (isAutomationRecent) {
          const elapsedSeconds = Math.floor((Date.now() - lastActivity) / 1000);
          instances.push({
            pid: 'automation-runner',
            kind: 'Automation Flow',
            status: isAutomationRunning ? 'running' : 'closed',
            command: 'clawie --workflow-run',
            elapsed_seconds: elapsedSeconds,
            detectedAt: lastActivity,
            lastSeen: new Date(lastActivity).toLocaleTimeString(),
            active: isAutomationRunning
          });
        }

        const rooms = updateKnownInstanceRooms(instances);
        renderInstanceRooms(rooms);
        const closedCount = rooms.filter(instance => instance.status === 'closed').length;
        instanceCount.textContent = `${instances.filter(instance => instance.status === 'running').length} running · ${closedCount} closed`;
        if (rooms.length === 0) {
          processList.innerHTML = '<div class="process-empty">No Clawie CLI instances detected yet. Start a Clawie CLI session and a room will appear here.</div>';
          return;
        }
        processList.innerHTML = rooms.map(instance => `
          <div class="process-card${instance.status === 'closed' ? ' closed' : ''}">
            <div class="process-top">
              <span class="process-kind">${escapeText(instance.kind)} · ${escapeText(instance.status || 'running')}</span>
              <span class="process-pid">PID ${escapeText(instance.pid)} · <span class="live-elapsed" data-base-seconds="${escapeText(instance.elapsed_seconds || 0)}" data-started-at="${escapeText(instance.detectedAt || Date.now())}" data-status="${escapeText(instance.status || 'running')}">${escapeText(formatElapsed(instance))}</span></span>
            </div>
            <div class="process-command">${escapeText(instance.command)}</div>
          </div>
        `).join('');
      } catch (error) {
        renderInstanceRooms(knownInstanceRooms);
        instanceCount.textContent = 'Scan failed';
        processList.innerHTML = `<div class="process-empty">${escapeText(error.message)}</div>`;
      }
    }

    function loadKnownInstanceRooms() {
      try {
        const parsed = JSON.parse(localStorage.getItem('clawie-known-instances') || '[]');
        return Array.isArray(parsed) ? parsed.filter(isCliRoom) : [];
      } catch (_) {
        return [];
      }
    }

    function isCliRoom(instance) {
      if (instance && String(instance.pid) === 'automation-runner') return true;
      const kind = String(instance?.kind || '').toLowerCase();
      const command = String(instance?.command || '').toLowerCase();
      return kind !== 'webui' && !command.includes(' webui') && !command.includes(' web-ui');
    }

    function updateKnownInstanceRooms(runningInstances) {
      const now = new Date().toLocaleTimeString();
      const lastActivity = Number(localStorage.getItem('clawie-last-automation-activity') || 0);
      const isAutomationRecent = (Date.now() - lastActivity) < 3600000;

      const cleanKnownRooms = knownInstanceRooms.filter(instance => {
        if (String(instance.pid) === 'automation-runner') {
          return isAutomationRecent;
        }
        return isCliRoom(instance);
      });

      const cleanRunningInstances = runningInstances.filter(instance => {
        if (String(instance.pid) === 'automation-runner') {
          return isAutomationRecent;
        }
        return isCliRoom(instance);
      });

      const byPid = new Map(cleanKnownRooms.map(instance => [String(instance.pid), instance]));
      cleanRunningInstances.forEach(instance => {
        byPid.set(String(instance.pid), {
          ...byPid.get(String(instance.pid)),
          ...instance,
          status: instance.status || 'running',
          detectedAt: instance.detectedAt || Date.now(),
          lastSeen: now
        });
      });

      const runningPids = new Set(cleanRunningInstances.filter(instance => instance.status === 'running').map(instance => String(instance.pid)));
      knownInstanceRooms = Array.from(byPid.values()).map(instance => {
        if (String(instance.pid) === 'automation-runner') {
          return instance;
        }
        return runningPids.has(String(instance.pid))
          ? instance
          : { ...instance, status: 'closed' };
      }).filter(instance => {
        if (String(instance.pid) === 'automation-runner') {
          return isAutomationRecent;
        }
        return true;
      });

      localStorage.setItem('clawie-known-instances', JSON.stringify(knownInstanceRooms.slice(-24)));
      return knownInstanceRooms;
    }

    function renderInstanceRooms(instances) {
      if (!instanceRoomGrid) return;
      const rooms = instances;
      if (rooms.length === 0) {
        instanceRoomGrid.innerHTML = `
          <article class="instance-room closed">
            <div class="instance-room-title">
              <span>No CLI instance yet</span>
              <small>closed</small>
            </div>
            <div class="bookshelf" style="left: 18px; top: 52px; width: 98px;"></div>
            <div class="desk" style="left: 34px; bottom: 28px; width: 112px;"></div>
            <div class="monitor instance-monitor" data-status="empty" data-kind="No CLI instance" style="left: 70px; bottom: 72px;"></div>
            <div class="server-rack" style="right: 22px; top: 58px; transform: scale(0.82); transform-origin: top right;"></div>
            <div class="plant" style="right: 28px; bottom: 22px;"></div>
            <div class="status-beacon" style="right: 34px; top: 36px;"></div>
          </article>
        `;
        return;
      }
      const colors = ['red', 'gold', 'blue', 'violet', 'green'];
      instanceRoomGrid.innerHTML = rooms.map((instance, index) => {
        const color = colors[index % colors.length];
        const status = instance.status || 'running';
        const isAutomation = instance.pid === 'automation-runner';
        const showTaskBoxes = status === 'running' && (isAutomation ? true : instance.active === true);
        const roomName = isAutomation ? 'Automation Flow' : `${instance.kind || 'Clawie'} ${instance.pid || ''}`;
        const agentName = isAutomation ? 'Automation' : (status === 'closed' ? 'Closed' : (instance.kind || 'CLI').replace(/\s+/g, ''));
        const safeId = String(instance.pid ?? index).replace(/[^a-zA-Z0-9_-]/g, '-');
        return `
          <article class="instance-room ${status === 'closed' ? 'closed' : ''}">
            <div class="instance-room-title">
              <span>${escapeText(roomName)}</span>
              <small><span class="live-elapsed" data-base-seconds="${escapeText(instance.elapsed_seconds || 0)}" data-started-at="${escapeText(instance.detectedAt || Date.now())}" data-status="${escapeText(status)}">${escapeText(formatElapsed(instance))}</span></small>
            </div>
            <div class="bookshelf" style="left: 18px; top: 52px; width: 98px;"></div>
            <div class="desk" style="left: 34px; bottom: 28px; width: 112px;"></div>
            <div class="monitor instance-monitor" data-pid="${escapeText(instance.pid || '')}" data-kind="${escapeText(instance.kind || 'Clawie CLI')}" data-status="${escapeText(status)}" data-command="${escapeText(instance.command || '')}" data-last-seen="${escapeText(instance.lastSeen || '')}" style="left: 70px; bottom: 72px;"></div>
            <div class="server-rack" style="right: 22px; top: 58px; transform: scale(0.82); transform-origin: top right;"></div>
            <div class="plant" style="right: 28px; bottom: 22px;"></div>
            <div class="status-beacon" style="right: 34px; top: 36px;"></div>
            <div class="agent ${isAutomation ? 'green robot-agent' : color} draggable-agent" data-agent-id="instance-${safeId}" data-name="${escapeText(agentName)}" data-pid="${escapeText(instance.pid || '')}" data-kind="${escapeText(instance.kind || 'Clawie CLI')}" data-status="${escapeText(status)}" style="left: 88px; top: 130px;"></div>
            ${showTaskBoxes ? `
              <div class="task-box" style="left: 132px; top: 118px;"></div>
              <div class="task-box two" style="left: 158px; top: 138px;"></div>
              <div class="task-box three" style="left: 126px; top: 162px;"></div>
            ` : ''}
          </article>
        `;
      }).join('');
      initializeAgentDragging();
    }

    async function openInstanceLog(pid, kind, statusOverride = null) {
      if (logWebSocket) {
        logWebSocket.close();
        logWebSocket = null;
      }
      instanceLogTitle.textContent = `${kind || 'Clawie Instance'} Logs`;
      instanceLogModal.hidden = false;

      if (pid === 'automation-runner') {
        instanceLogBody.innerHTML = '';
        const logs = JSON.parse(localStorage.getItem('clawie-automation-logs') || '[]');
        if (logs.length === 0) {
          instanceLogBody.innerHTML = '<div class="log-line">No recent automation activity logged in the last 1 hour.</div>';
        } else {
          logs.forEach(log => {
            const timeString = new Date(log.timestamp).toLocaleTimeString();
            const logLineDiv = document.createElement('div');
            logLineDiv.className = 'log-line';
            logLineDiv.innerHTML = `<span style="color: var(--text-muted); font-size: 0.65rem;">[${timeString}]</span> <strong style="color: #10b981;">*</strong> ${escapeText(log.message)}`;
            instanceLogBody.appendChild(logLineDiv);
          });
        }
        instanceLogBody.scrollTop = instanceLogBody.scrollHeight;
        return;
      }

      const monitor = document.querySelector(`.instance-monitor[data-pid="${CSS.escape(String(pid || ''))}"]`);
      const monitorStatus = statusOverride || monitor?.dataset.status;
      if (monitorStatus === 'empty') {
        instanceLogBody.innerHTML = `
          <div class="log-line"><strong>No CLI instance:</strong> WebUI is not counted as an instance.</div>
          <div class="log-line"><strong>Action:</strong> start a Clawie CLI session and a room will appear here.</div>
        `;
        return;
      }
      if (monitorStatus === 'closed') {
        instanceLogBody.innerHTML = `
          <div class="log-line"><strong>Closed:</strong> this Clawie CLI process is no longer running.</div>
          <div class="log-line"><strong>Last seen:</strong> ${escapeText(monitor?.dataset.lastSeen || 'unknown')}</div>
          <div class="log-line"><strong>Last command:</strong> ${escapeText(monitor?.dataset.command || 'unknown')}</div>
        `;
        return;
      }
      if (!pid) {
        instanceLogBody.innerHTML = '<div class="log-line"><strong>Missing PID:</strong> this room is not attached to a process.</div>';
        return;
      }

      instanceLogBody.innerHTML = '<div class="log-line">Connecting to live log stream...</div>';
      try {
        const wsProtocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
        const wsUrl = `${wsProtocol}//${window.location.host}/ws-log?pid=${encodeURIComponent(pid)}`;
        logWebSocket = new WebSocket(wsUrl);

        logWebSocket.onopen = () => {
          instanceLogBody.innerHTML = '<div class="log-line" style="color: var(--ok);"><strong>Connected:</strong> Log stream active.</div>';
        };

        logWebSocket.onmessage = (event) => {
          const line = event.data;
          const logLineDiv = document.createElement('div');
          logLineDiv.className = 'log-line';
          logLineDiv.innerHTML = `<strong>*</strong> ${escapeText(line)}`;
          instanceLogBody.appendChild(logLineDiv);
          instanceLogBody.scrollTop = instanceLogBody.scrollHeight;
        };

        logWebSocket.onerror = (error) => {
          console.error("WebSocket error:", error);
          instanceLogBody.innerHTML = '<div class="log-line" style="color: var(--error);"><strong>Error:</strong> WebSocket connection error.</div>';
        };

        logWebSocket.onclose = () => {
          const closeDiv = document.createElement('div');
          closeDiv.className = 'log-line';
          closeDiv.innerHTML = '<strong>Disconnected:</strong> Log stream closed.';
          instanceLogBody.appendChild(closeDiv);
        };
      } catch (error) {
        instanceLogBody.innerHTML = `<div class="log-line"><strong>Error:</strong> ${escapeText(error.message)}</div>`;
      }
    }

    function formatSeconds(totalSeconds) {
      const safeSeconds = Math.max(0, Math.floor(Number(totalSeconds) || 0));
      const days = Math.floor(safeSeconds / 86400);
      const hours = Math.floor((safeSeconds % 86400) / 3600);
      const minutes = Math.floor((safeSeconds % 3600) / 60);
      const seconds = safeSeconds % 60;
      const two = value => String(value).padStart(2, '0');
      if (days > 0) return `${days}d ${two(hours)}:${two(minutes)}:${two(seconds)}`;
      if (hours > 0) return `${two(hours)}:${two(minutes)}:${two(seconds)}`;
      return `${two(minutes)}:${two(seconds)}`;
    }

    function formatElapsed(instance) {
      if ((instance.status || 'running') === 'closed') return 'closed';
      return formatSeconds(Number(instance.elapsed_seconds || 0));
    }

    function currentElapsedSeconds(element) {
      const baseSeconds = Number(element.dataset.baseSeconds || 0);
      const startedAt = Number(element.dataset.startedAt || Date.now());
      const status = element.dataset.status || 'running';
      if (status === 'closed') return baseSeconds;
      return baseSeconds + Math.floor((Date.now() - startedAt) / 1000);
    }

    function tickElapsedTimers() {
      document.querySelectorAll('.live-elapsed').forEach(element => {
        if (element.dataset.status === 'closed') {
          element.textContent = 'closed';
          return;
        }
        element.textContent = formatSeconds(currentElapsedSeconds(element));
      });
    }

    function openAgentContextMenu(agent, event) {
      event.preventDefault();
      selectedAgentContext = {
        pid: agent.dataset.pid,
        kind: agent.dataset.kind,
        status: agent.dataset.status
      };
      agentContextMenu.hidden = false;
      const menuWidth = 200;
      const menuHeight = 152;
      agentContextMenu.style.left = Math.min(event.clientX, window.innerWidth - menuWidth - 8) + 'px';
      agentContextMenu.style.top = Math.min(event.clientY, window.innerHeight - menuHeight - 8) + 'px';
    }

    function closeAgentContextMenu() {
      agentContextMenu.hidden = true;
      selectedAgentContext = null;
    }

    async function terminateSelectedInstance() {
      if (!selectedAgentContext?.pid || selectedAgentContext.status === 'closed') {
        setStatus('No running CLI instance selected', 'error');
        return;
      }
      const confirmed = window.confirm(`Terminate Clawie CLI PID ${selectedAgentContext.pid}?`);
      if (!confirmed) return;
      try {
        setStatus('Terminating CLI PID ' + selectedAgentContext.pid + '...', 'busy');
        const response = await fetch('/instance-action', {
          method: 'POST',
          headers: {'Content-Type': 'application/json'},
          body: JSON.stringify({ pid: Number(selectedAgentContext.pid), action: 'terminate' })
        });
        const result = await response.json();
        if (!response.ok || !result.ok) throw new Error(result.error || 'Terminate failed');
        setStatus('Terminate signal sent to PID ' + selectedAgentContext.pid, 'saved');
        await refreshInstances();
      } catch (error) {
        setStatus(error.message, 'error');
      }
    }

    function initializeAgentDragging() {
      const pixelMap = document.querySelector('.pixel-map');
      const agents = Array.from(document.querySelectorAll('.draggable-agent'));
      if (!pixelMap || agents.length === 0) return;

      agents.forEach(agent => {
        if (agent.dataset.dragReady === 'true') return;
        agent.dataset.dragReady = 'true';
        agent.addEventListener('contextmenu', event => openAgentContextMenu(agent, event));
        const saved = localStorage.getItem('clawie-agent-pos-' + agent.dataset.agentId);
        if (saved) {
          try {
            const position = JSON.parse(saved);
            if (Number.isFinite(position.left) && Number.isFinite(position.top)) {
              agent.style.left = position.left + 'px';
              agent.style.top = position.top + 'px';
              agent.style.right = 'auto';
              agent.style.bottom = 'auto';
            }
          } catch (_) {}
        }

        agent.addEventListener('pointerdown', event => {
          event.preventDefault();
          agent.setPointerCapture(event.pointerId);
          const dragBounds = agent.closest('.instance-room') || pixelMap;
          const boundsRect = dragBounds.getBoundingClientRect();
          const agentRect = agent.getBoundingClientRect();
          const offsetX = event.clientX - agentRect.left;
          const offsetY = event.clientY - agentRect.top;
          agent.classList.add('dragging');
          agent.style.right = 'auto';
          agent.style.bottom = 'auto';

          const moveAgent = moveEvent => {
            const zoomScale = boundsRect.width > 0 && dragBounds.offsetWidth > 0
              ? boundsRect.width / dragBounds.offsetWidth
              : 1;
            const maxLeft = dragBounds.clientWidth - agent.offsetWidth - 4;
            const maxTop = dragBounds.clientHeight - agent.offsetHeight - 4;
            const nextLeft = Math.max(4, Math.min(maxLeft, (moveEvent.clientX - boundsRect.left - offsetX) / zoomScale));
            const nextTop = Math.max(36, Math.min(maxTop, (moveEvent.clientY - boundsRect.top - offsetY) / zoomScale));
            const snappedLeft = Math.round(nextLeft / 4) * 4;
            const snappedTop = Math.round(nextTop / 4) * 4;
            agent.style.left = snappedLeft + 'px';
            agent.style.top = snappedTop + 'px';
          };

          const stopDrag = () => {
            agent.classList.remove('dragging');
            localStorage.setItem('clawie-agent-pos-' + agent.dataset.agentId, JSON.stringify({
              left: parseInt(agent.style.left, 10) || 0,
              top: parseInt(agent.style.top, 10) || 0
            }));
            agent.removeEventListener('pointermove', moveAgent);
            agent.removeEventListener('pointerup', stopDrag);
            agent.removeEventListener('pointercancel', stopDrag);
          };

          agent.addEventListener('pointermove', moveAgent);
          agent.addEventListener('pointerup', stopDrag);
          agent.addEventListener('pointercancel', stopDrag);
        });
      });
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
      syncInstancePanel();
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
      setStatus('Clawie is thinking...', 'thinking');
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
            gemini_api_key: localStorage.getItem('clawie-gemini-key') || '',
            xai_api_key: localStorage.getItem('clawie-xai-key') || '',
            kimi_api_key: localStorage.getItem('clawie-kimi-key') || '',
            openai_base_url: localStorage.getItem('clawie-openai-url') || '',
            gemini_base_url: localStorage.getItem('clawie-gemini-url') || '',
            xai_base_url: localStorage.getItem('clawie-xai-url') || '',
            kimi_base_url: localStorage.getItem('clawie-kimi-url') || '',
            lean_mode: selectedLeanMode,
            max_turns: selectedMaxTurns,
            token_budget: selectedTokenBudget
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
        const isFetchFailure = error instanceof TypeError && /fetch/i.test(error.message);
        const displayError = isFetchFailure
          ? 'Could not reach the local Clawie server. Reload this Web UI from the latest URL printed in the terminal.'
          : error.message;
        content.innerHTML = formatMarkdown(displayError);
        const action = classifyChatError(displayError, isFetchFailure);
        if (action) {
          content.appendChild(createErrorActionPanel(action));
        }
        
        pending.append(content);
        setStatus(displayError, 'error');
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
        setStatus('Copy failed: ' + e.message, 'error');
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
        setStatus('Open in editor failed: ' + e.message, 'error');
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
        syncInstancePanel();
        fileList.replaceChildren();
        result.files.forEach(name => {
          const item = document.createElement('button');
          item.className = 'file';
          item.title = name;
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

    editorDiffBtn.addEventListener('click', () => {
      if (diffContainer.style.display === 'none') {
        textarea.style.display = 'none';
        lineNumbers.style.display = 'none';
        diffContainer.style.display = 'flex';
        editorDiffBtn.textContent = 'Show Editor';
        
        const currentCode = textarea.value;
        const targetCode = (improvementsCode && improvementsCode !== originalCode) ? improvementsCode : currentCode;
        const diffResult = computeLineDiff(originalCode, targetCode);
        diffLeft.innerHTML = diffResult.left;
        diffRight.innerHTML = diffResult.right;
      } else {
        diffContainer.style.display = 'none';
        textarea.style.display = 'block';
        lineNumbers.style.display = 'block';
        editorDiffBtn.textContent = 'Show Diff';
      }
    });

    function computeLineDiff(oldText, newText) {
      const oldLines = oldText.split('\n');
      const newLines = newText.split('\n');
      
      if (oldLines.length * newLines.length > 1000000) {
        const leftHtml = [];
        const rightHtml = [];
        const maxLen = Math.max(oldLines.length, newLines.length);
        for (let i = 0; i < maxLen; i++) {
          const oldLine = i < oldLines.length ? oldLines[i] : null;
          const newLine = i < newLines.length ? newLines[i] : null;
          if (oldLine === newLine) {
            const line = escapeText(oldLine);
            leftHtml.push(`<div>  ${line}</div>`);
            rightHtml.push(`<div>  ${line}</div>`);
          } else {
            if (oldLine !== null) {
              leftHtml.push(`<div style="background-color: rgba(247, 118, 142, 0.2); font-weight: 500;">- ${escapeText(oldLine)}</div>`);
            }
            if (newLine !== null) {
              rightHtml.push(`<div style="background-color: rgba(78, 172, 109, 0.2); font-weight: 500;">+ ${escapeText(newLine)}</div>`);
            }
          }
        }
        return { left: leftHtml.join(''), right: rightHtml.join('') };
      }
      
      const dp = Array(oldLines.length + 1).fill(null).map(() => Array(newLines.length + 1).fill(0));
      for (let i = 1; i <= oldLines.length; i++) {
        for (let j = 1; j <= newLines.length; j++) {
          if (oldLines[i - 1] === newLines[j - 1]) {
            dp[i][j] = dp[i - 1][j - 1] + 1;
          } else {
            dp[i][j] = Math.max(dp[i - 1][j], dp[i][j - 1]);
          }
        }
      }
      
      const leftHtml = [];
      const rightHtml = [];
      let i = oldLines.length;
      let j = newLines.length;
      
      while (i > 0 || j > 0) {
        if (i > 0 && j > 0 && oldLines[i - 1] === newLines[j - 1]) {
          const line = escapeText(oldLines[i - 1]);
          leftHtml.unshift(`<div>  ${line}</div>`);
          rightHtml.unshift(`<div>  ${line}</div>`);
          i--;
          j--;
        } else if (j > 0 && (i === 0 || dp[i][j - 1] >= dp[i - 1][j])) {
          const line = escapeText(newLines[j - 1]);
          leftHtml.unshift(`<div style="background-color: rgba(0, 0, 0, 0); min-height: 1.5em;">&nbsp;</div>`);
          rightHtml.unshift(`<div style="background-color: rgba(78, 172, 109, 0.2); font-weight: 500;">+ ${line}</div>`);
          j--;
        } else {
          const line = escapeText(oldLines[i - 1]);
          leftHtml.unshift(`<div style="background-color: rgba(247, 118, 142, 0.2); font-weight: 500;">- ${line}</div>`);
          rightHtml.unshift(`<div style="background-color: rgba(0, 0, 0, 0); min-height: 1.5em;">&nbsp;</div>`);
          i--;
        }
      }
      return { left: leftHtml.join(''), right: rightHtml.join('') };
    }

    async function loadFile(name, item) {
      setStatus('Opening ' + name + '...', 'busy');
      try {
        const response = await fetch('/load', { method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify({directory: locationPath.value, filename: name}) });
        const result = await response.json();
        if (!response.ok || !result.ok) throw new Error(result.error || 'Open failed');
        
        activeFileName = name;
        originalCode = result.code;
        improvementsCode = result.improvements || '';
        
        document.querySelector('#editor-filename').innerHTML = `
          <span class="brand-dot" style="width: 6px; height: 6px; border-radius: 50%; background: var(--ok); box-shadow: 0 0 6px var(--ok);"></span>
          ${name}
        `;
        document.querySelector('#editor-placeholder').style.display = 'none';
        
        textarea.value = result.code;
        textarea.style.display = 'block';
        lineNumbers.style.display = 'block';
        diffContainer.style.display = 'none';
        editorDiffBtn.style.display = 'block';
        editorDiffBtn.textContent = 'Show Diff';
        document.querySelector('#editor-save-btn').style.display = 'block';

        document.querySelectorAll('.file').forEach(f => f.classList.remove('active'));
        item.classList.add('active');
        updateLineNumbers();
        syncInstancePanel();
        setStatus('Opened ' + name, 'saved');
      } catch (error) { setStatus(error.message, 'error'); }
    }

    async function saveCurrentFile() {
      if (!activeFileName) return;
      const saveBtn = document.querySelector('#editor-save-btn');
      saveBtn.disabled = true;
      setStatus('Saving ' + activeFileName + '...', 'busy');
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
        setStatus(error.message, 'error');
      } finally {
        saveBtn.disabled = false;
      }
    }

    chatSend.addEventListener('click', sendChatMessage);
    document.querySelector('#editor-save-btn').addEventListener('click', saveCurrentFile);
    locationPreset.addEventListener('change', () => { if (locationPreset.value) locationPath.value = locationPreset.value; });
    locationPreset.addEventListener('dblclick', () => { if (locationPreset.value) { locationPath.value = locationPreset.value; refreshFiles(); } });
    locationPath.addEventListener('change', refreshFiles);
    codeViewTab.addEventListener('click', () => setWorkspaceView('code'));
    instanceViewTab.addEventListener('click', () => setWorkspaceView('instance'));
    automationsViewTab.addEventListener('click', () => setWorkspaceView('automations'));
    instanceZoomIn.addEventListener('click', () => setInstanceZoom(instanceZoom + instanceZoomStep));
    instanceZoomOut.addEventListener('click', () => setInstanceZoom(instanceZoom - instanceZoomStep));
    instanceRefresh.addEventListener('click', refreshInstances);
    instanceRoomGrid.addEventListener('click', event => {
      const monitor = event.target.closest('.instance-monitor');
      const agent = event.target.closest('.agent');
      if (monitor) {
        openInstanceLog(monitor.dataset.pid, monitor.dataset.kind, monitor.dataset.status);
      } else if (agent) {
        openInstanceLog(agent.dataset.pid, agent.dataset.kind, agent.dataset.status);
      }
    });
    agentContextMenu.addEventListener('click', async event => {
      const action = event.target.closest('button')?.dataset.agentAction;
      if (!action || !selectedAgentContext) return;
      const context = selectedAgentContext;
      agentContextMenu.hidden = true;
      if (action === 'logs') {
        openInstanceLog(context.pid, context.kind, context.status);
      } else if (action === 'copy-pid') {
        await navigator.clipboard.writeText(context.pid || '');
        setStatus('Copied PID ' + context.pid, 'saved');
      } else if (action === 'refresh') {
        await refreshInstances();
      } else if (action === 'terminate') {
        selectedAgentContext = context;
        await terminateSelectedInstance();
      }
    });
    instanceLogClose.addEventListener('click', () => {
      instanceLogModal.hidden = true;
      if (logWebSocket) {
        logWebSocket.close();
        logWebSocket = null;
      }
    });
    window.addEventListener('click', event => {
      if (!agentContextMenu.hidden && !event.target.closest('#agent-context-menu')) {
        closeAgentContextMenu();
      }
      if (event.target === instanceLogModal) {
        instanceLogModal.hidden = true;
        if (logWebSocket) {
          logWebSocket.close();
          logWebSocket = null;
        }
      }
    });

    const chooseFolderBtn = document.querySelector('#choose-folder-btn');
    chooseFolderBtn.addEventListener('click', async () => {
      setStatus('Choosing folder...', 'busy');
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
        setStatus('Failed to choose folder. Please enter the absolute path directly in the field below.', 'error');
      }
    });

    const settingsToggle = document.querySelector('#settings-toggle');
    const settingsModal = document.querySelector('#settings-modal');
    const settingsClose = document.querySelector('#settings-close');
    const settingsSaveBtn = document.querySelector('#settings-save-btn');
    const settingsAppTheme = document.querySelector('#settings-app-theme');
    const settingsModel = document.querySelector('#settings-model');
    const settingsOpenAiKey = document.querySelector('#settings-openai-key');
    const settingsAnthropicKey = document.querySelector('#settings-anthropic-key');
    const settingsGeminiKey = document.querySelector('#settings-gemini-key');
    const settingsOpenAiUrl = document.querySelector('#settings-openai-url');
    const settingsGeminiUrl = document.querySelector('#settings-gemini-url');
    const settingsXaiKey = document.querySelector('#settings-xai-key');
    const settingsXaiUrl = document.querySelector('#settings-xai-url');
    const settingsKimiKey = document.querySelector('#settings-kimi-key');
    const settingsKimiUrl = document.querySelector('#settings-kimi-url');
    const settingsInstallApp = document.querySelector('#settings-install-app');
    const settingsInstallStatus = document.querySelector('#settings-install-status');

    const settingsProvider = document.querySelector('#settings-provider');
    const settingsLeanMode = document.querySelector('#settings-lean-mode');
    const settingsMaxTurns = document.querySelector('#settings-max-turns');
    const maxTurnsVal = document.querySelector('#max-turns-val');
    const settingsTokenBudget = document.querySelector('#settings-token-budget');
    const settingsTestConnBtn = document.querySelector('#settings-test-conn-btn');
    const settingsConnStatusContainer = document.querySelector('#settings-conn-status-container');
    const settingsConnStatusText = document.querySelector('#settings-conn-status-text');

    const settingsFontUi = document.querySelector('#settings-font-ui');
    const settingsFontCode = document.querySelector('#settings-font-code');
    const settingsLayoutDensity = document.querySelector('#settings-layout-density');
    const settingsSidebarLayout = document.querySelector('#settings-sidebar-layout');
    const settingsCustomAccentToggle = document.querySelector('#settings-custom-accent-toggle');
    const settingsCustomAccentColor = document.querySelector('#settings-custom-accent-color');
    const settingsAmbientGlow = document.querySelector('#settings-ambient-glow');
    const settingsTerminalGlow = document.querySelector('#settings-terminal-glow');
    const settingsStatusPulsate = document.querySelector('#settings-status-pulsate');

    const themes = {
      orange: { rgb: '249, 115, 22', hover: '#ea580c' },
      blue: { rgb: '37, 99, 235', hover: '#1d4ed8' },
      purple: { rgb: '139, 92, 246', hover: '#7c3aed' },
      green: { rgb: '16, 185, 129', hover: '#059669' }
    };

    let selectedTheme = localStorage.getItem('clawie-theme') || 'orange';
    let selectedAppTheme = localStorage.getItem('clawie-app-theme') || 'dark';
    let selectedModel = localStorage.getItem('clawie-model-setting') || 'gemini-1.5-pro';
    let selectedProvider = localStorage.getItem('clawie-provider') || 'gemini';
    let selectedLeanMode = localStorage.getItem('clawie-lean-mode') || 'full';
    let selectedMaxTurns = parseInt(localStorage.getItem('clawie-max-turns') || '64', 10);
    let selectedTokenBudget = parseInt(localStorage.getItem('clawie-token-budget') || '12000', 10);

    let selectedFontUi = localStorage.getItem('clawie-font-ui') || 'inter';
    let selectedFontCode = localStorage.getItem('clawie-font-code') || 'jetbrains';
    let selectedLayoutDensity = localStorage.getItem('clawie-layout-density') || 'comfortable';
    let selectedSidebarLayout = localStorage.getItem('clawie-sidebar-layout') || 'expanded';
    let selectedCustomAccentToggle = localStorage.getItem('clawie-custom-accent-toggle') === "true";
    let selectedCustomAccentColor = localStorage.getItem('clawie-custom-accent-color') || '#f97316';
    let selectedAmbientGlow = localStorage.getItem('clawie-ambient-glow') !== "false";
    let selectedTerminalGlow = localStorage.getItem('clawie-terminal-glow') === "true";
    let selectedStatusPulsate = localStorage.getItem('clawie-status-pulsate') !== "false";

    let deferredInstallPrompt = null;

    // Appearance helpers
    function applyFontUi(font) {
      document.documentElement.dataset.fontUi = font;
      localStorage.setItem('clawie-font-ui', font);
      selectedFontUi = font;
      if (settingsFontUi) settingsFontUi.value = font;
    }

    function applyFontCode(font) {
      document.documentElement.dataset.fontCode = font;
      localStorage.setItem('clawie-font-code', font);
      selectedFontCode = font;
      if (settingsFontCode) settingsFontCode.value = font;
    }

    function applyLayoutDensity(density) {
      document.documentElement.dataset.layoutDensity = density;
      localStorage.setItem('clawie-layout-density', density);
      selectedLayoutDensity = density;
      if (settingsLayoutDensity) settingsLayoutDensity.value = density;
    }

    function applySidebarLayout(layout) {
      document.documentElement.dataset.sidebarLayout = layout;
      localStorage.setItem('clawie-sidebar-layout', layout);
      selectedSidebarLayout = layout;
      if (settingsSidebarLayout) settingsSidebarLayout.value = layout;
    }

    function applyAmbientGlow(enabled) {
      document.documentElement.dataset.ambientGlow = enabled ? "true" : "false";
      localStorage.setItem('clawie-ambient-glow', enabled ? "true" : "false");
      selectedAmbientGlow = enabled;
      if (settingsAmbientGlow) settingsAmbientGlow.checked = enabled;
    }

    function applyTerminalGlow(enabled) {
      document.documentElement.dataset.terminalGlow = enabled ? "true" : "false";
      localStorage.setItem('clawie-terminal-glow', enabled ? "true" : "false");
      selectedTerminalGlow = enabled;
      if (settingsTerminalGlow) settingsTerminalGlow.checked = enabled;
    }

    function applyStatusPulsate(enabled) {
      document.documentElement.dataset.statusPulsate = enabled ? "true" : "false";
      localStorage.setItem('clawie-status-pulsate', enabled ? "true" : "false");
      selectedStatusPulsate = enabled;
      if (settingsStatusPulsate) settingsStatusPulsate.checked = enabled;
    }

    function applyCustomAccent(enabled, hexColor) {
      selectedCustomAccentToggle = enabled;
      selectedCustomAccentColor = hexColor;
      localStorage.setItem('clawie-custom-accent-toggle', enabled ? "true" : "false");
      localStorage.setItem('clawie-custom-accent-color', hexColor);

      if (settingsCustomAccentToggle) settingsCustomAccentToggle.checked = enabled;
      if (settingsCustomAccentColor) settingsCustomAccentColor.value = hexColor;

      if (enabled && hexColor) {
        const r = parseInt(hexColor.slice(1, 3), 16);
        const g = parseInt(hexColor.slice(3, 5), 16);
        const b = parseInt(hexColor.slice(5, 7), 16);
        
        document.documentElement.style.setProperty('--accent-rgb', `${r}, ${g}, ${b}`);
        document.documentElement.style.setProperty('--accent-hover', adjustColorBrightness(hexColor, -15));
        
        document.querySelectorAll('.theme-opt').forEach(opt => opt.classList.remove('active'));
      } else {
        applyTheme(selectedTheme);
      }
    }

    function adjustColorBrightness(hex, percent) {
      let r = parseInt(hex.slice(1, 3), 16);
      let g = parseInt(hex.slice(3, 5), 16);
      let b = parseInt(hex.slice(5, 7), 16);

      r = Math.min(255, Math.max(0, r + (r * percent / 100)));
      g = Math.min(255, Math.max(0, g + (g * percent / 100)));
      b = Math.min(255, Math.max(0, b + (b * percent / 100)));

      const rHex = Math.round(r).toString(16).padStart(2, '0');
      const gHex = Math.round(g).toString(16).padStart(2, '0');
      const bHex = Math.round(b).toString(16).padStart(2, '0');
      
      return `#${rHex}${gHex}${bHex}`;
    }

    // INITIAL APPLICATION OF THEMES & APPEARANCE
    applyAppTheme(selectedAppTheme);
    applyTheme(selectedTheme);
    applyFontUi(selectedFontUi);
    applyFontCode(selectedFontCode);
    applyLayoutDensity(selectedLayoutDensity);
    applySidebarLayout(selectedSidebarLayout);
    applyAmbientGlow(selectedAmbientGlow);
    applyTerminalGlow(selectedTerminalGlow);
    applyStatusPulsate(selectedStatusPulsate);
    applyCustomAccent(selectedCustomAccentToggle, selectedCustomAccentColor);

    async function openSettingsPanel(focusTarget = 'openai') {
      try {
        const response = await fetch('/api/settings');
        const res = await response.json();
        if (response.ok && res.ok && res.settings) {
          const s = res.settings;
          if (s.provider) selectedProvider = s.provider;
          if (s.model) selectedModel = s.model;
          if (s.OPENAI_API_KEY) localStorage.setItem('clawie-openai-key', s.OPENAI_API_KEY);
          if (s.ANTHROPIC_API_KEY) localStorage.setItem('clawie-anthropic-key', s.ANTHROPIC_API_KEY);
          if (s.GEMINI_API_KEY) localStorage.setItem('clawie-gemini-key', s.GEMINI_API_KEY);
          if (s.OPENAI_BASE_URL) localStorage.setItem('clawie-openai-url', s.OPENAI_BASE_URL);
          if (s.GEMINI_BASE_URL) localStorage.setItem('clawie-gemini-url', s.GEMINI_BASE_URL);
          if (s.XAI_API_KEY) localStorage.setItem('clawie-xai-key', s.XAI_API_KEY);
          if (s.XAI_BASE_URL) localStorage.setItem('clawie-xai-url', s.XAI_BASE_URL);
          if (s.MOONSHOT_API_KEY) localStorage.setItem('clawie-kimi-key', s.MOONSHOT_API_KEY);
          if (s.MOONSHOT_BASE_URL) localStorage.setItem('clawie-kimi-url', s.MOONSHOT_BASE_URL);
          if (s.CLAWIE_LEAN_MODE) selectedLeanMode = s.CLAWIE_LEAN_MODE;
          if (s.CLAWIE_MAX_TURNS) selectedMaxTurns = parseInt(s.CLAWIE_MAX_TURNS, 10);
          if (s.CLAWIE_MAX_BUDGET_TOKENS) selectedTokenBudget = parseInt(s.CLAWIE_MAX_BUDGET_TOKENS, 10);
        }
      } catch (e) {
        console.warn('Failed to load settings from backend, using local defaults', e);
      }

      settingsProvider.value = selectedProvider;
      settingsAppTheme.value = selectedAppTheme;
      settingsModel.value = selectedModel;
      settingsLeanMode.value = selectedLeanMode;
      settingsMaxTurns.value = selectedMaxTurns;
      maxTurnsVal.textContent = selectedMaxTurns;
      settingsTokenBudget.value = selectedTokenBudget || '';

      settingsOpenAiKey.value = localStorage.getItem('clawie-openai-key') || '';
      settingsAnthropicKey.value = localStorage.getItem('clawie-anthropic-key') || '';
      settingsGeminiKey.value = localStorage.getItem('clawie-gemini-key') || '';
      settingsOpenAiUrl.value = localStorage.getItem('clawie-openai-url') || '';
      settingsGeminiUrl.value = localStorage.getItem('clawie-gemini-url') || '';
      settingsXaiKey.value = localStorage.getItem('clawie-xai-key') || '';
      settingsXaiUrl.value = localStorage.getItem('clawie-xai-url') || '';
      settingsKimiKey.value = localStorage.getItem('clawie-kimi-key') || '';
      settingsKimiUrl.value = localStorage.getItem('clawie-kimi-url') || '';
      
      applyAppTheme(selectedAppTheme);
      applyTheme(selectedTheme);
      applyFontUi(selectedFontUi);
      applyFontCode(selectedFontCode);
      applyLayoutDensity(selectedLayoutDensity);
      applySidebarLayout(selectedSidebarLayout);
      applyAmbientGlow(selectedAmbientGlow);
      applyTerminalGlow(selectedTerminalGlow);
      applyStatusPulsate(selectedStatusPulsate);
      applyCustomAccent(selectedCustomAccentToggle, selectedCustomAccentColor);

      settingsConnStatusContainer.style.display = 'none';
      settingsModal.hidden = false;

      // Select active tab button and content
      document.querySelectorAll('.settings-tab-btn').forEach(b => b.classList.toggle('active', b.dataset.tab === 'tab-appearance'));
      document.querySelectorAll('.settings-tab-content').forEach(c => c.classList.toggle('active', c.id === 'tab-appearance'));

      const target = focusTarget === 'anthropic'
        ? settingsAnthropicKey
        : focusTarget === 'base-url'
          ? settingsOpenAiUrl
          : settingsOpenAiKey;
      requestAnimationFrame(() => {
        target.focus();
        target.scrollIntoView({ block: 'center', behavior: 'smooth' });
      });
    }

    function classifyChatError(message, isFetchFailure) {
      if (isFetchFailure) {
        return {
          severity: 'critical',
          text: 'The local Web UI server is not reachable. Open the newest URL printed by `./clawie webui` and keep that terminal window running.',
          button: 'Use latest Web UI URL'
        };
      }
      if (/anthropic|claude/i.test(message) && /401|unauthorized|api key|auth|credential|token/i.test(message)) {
        return {
          text: 'Anthropic authentication failed. Add a valid Anthropic API key or restart `./clawie webui` from a terminal that has `ANTHROPIC_API_KEY` exported.',
          button: 'Open Anthropic API Settings',
          focus: 'anthropic'
        };
      }
      if (/base url|OPENAI_BASE_URL|invalid url|connection refused|name resolution|dns/i.test(message)) {
        return {
          text: 'The custom OpenAI base URL looks unreachable or invalid. Check the base URL, or clear it to use the default OpenAI endpoint.',
          button: 'Open Base URL Settings',
          focus: 'base-url'
        };
      }
      if (/401|unauthorized|api key|incorrect api key|invalid_request_error|missing openai credentials|dummy|credential/i.test(message)) {
        return {
          text: 'OpenAI authentication failed. Add a valid OpenAI API key here, or restart `./clawie webui` from a terminal that has `OPENAI_API_KEY` exported.',
          button: 'Open OpenAI API Settings',
          focus: 'openai'
        };
      }
      if (/gemini|google/i.test(message) && /401|unauthorized|api key|auth|credential|token|GEMINI_API_KEY|GOOGLE_API_KEY/i.test(message)) {
        return {
          text: 'Gemini authentication failed. Add a valid Gemini API key or restart `./clawie webui` from a terminal that has `GEMINI_API_KEY` exported.',
          button: 'Open Gemini API Settings',
          focus: 'gemini'
        };
      }
      if (/rate limit|429|quota|billing|insufficient_quota/i.test(message)) {
        return {
          text: 'The provider rejected the request because of rate limits, quota, or billing. Check your provider dashboard, then retry.',
          button: 'Open API Settings',
          focus: 'openai'
        };
      }
      return null;
    }

    function createErrorActionPanel(action) {
      const panel = document.createElement('div');
      panel.className = 'error-action-panel' + (action.severity === 'critical' ? ' error-action-critical' : '');
      const text = document.createElement('div');
      text.className = 'error-action-text';
      text.textContent = action.text;
      panel.appendChild(text);
      if (action.focus) {
        const button = document.createElement('button');
        button.type = 'button';
        button.className = 'error-action-btn';
        button.textContent = action.button || 'Open Settings';
        button.addEventListener('click', () => openSettingsPanel(action.focus));
        panel.appendChild(button);
      }
      return panel;
    }

    function applyAppTheme(themeName) {
      const supportedThemes = new Set([
        'dark', 'light', 'graphite', 'contrast',
        'tokyo-night', 'dracula', 'nord', 'synthwave'
      ]);
      const nextTheme = supportedThemes.has(themeName) ? themeName : 'dark';
      document.documentElement.dataset.appTheme = nextTheme;
      localStorage.setItem('clawie-app-theme', nextTheme);
      selectedAppTheme = nextTheme;
      if (settingsAppTheme) {
        settingsAppTheme.value = nextTheme;
      }
      const bgColors = {
        light: '#f8fafc',
        graphite: '#111113',
        'tokyo-night': '#1a1b26',
        dracula: '#282a36',
        nord: '#2e3440',
        synthwave: '#2b213a',
        contrast: '#000000',
        dark: '#09090b'
      };
      const themeColor = bgColors[nextTheme] || '#09090b';
      const themeColorMeta = document.querySelector('meta[name="theme-color"]');
      if (themeColorMeta) {
        themeColorMeta.setAttribute('content', themeColor);
      }
    }

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

    // Tab switching event listener
    document.querySelectorAll('.settings-tabs .settings-tab-btn').forEach(btn => {
      btn.addEventListener('click', () => {
        document.querySelectorAll('.settings-tabs .settings-tab-btn').forEach(b => b.classList.remove('active'));
        document.querySelectorAll('#settings-modal .settings-tab-content').forEach(c => c.classList.remove('active'));
        btn.classList.add('active');
        document.querySelector('#' + btn.dataset.tab).classList.add('active');
      });
    });

    // Password fields show/hide toggles
    document.querySelectorAll('.password-toggle-btn').forEach(btn => {
      btn.addEventListener('click', () => {
        const input = document.getElementById(btn.dataset.target);
        if (input.type === 'password') {
          input.type = 'text';
          btn.innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"></path><line x1="1" y1="1" x2="23" y2="23"></line></svg>';
        } else {
          input.type = 'password';
          btn.innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path><circle cx="12" cy="12" r="3"></circle></svg>';
        }
      });
    });

    // Update max turns slider span value on slide
    settingsMaxTurns.addEventListener('input', () => {
      maxTurnsVal.textContent = settingsMaxTurns.value;
    });

    // Test Connection listener
    settingsTestConnBtn.addEventListener('click', async () => {
      const provider = settingsProvider.value;
      let key;
      let baseUrl;
      if (provider === 'anthropic') {
        key = settingsAnthropicKey.value.trim();
        baseUrl = '';
      } else if (provider === 'gemini') {
        key = settingsGeminiKey.value.trim();
        baseUrl = settingsGeminiUrl.value.trim();
      } else {
        key = settingsOpenAiKey.value.trim();
        baseUrl = settingsOpenAiUrl.value.trim();
      }
      const model = settingsModel.value;

      if (!key) {
        settingsConnStatusContainer.style.display = 'flex';
        const dot = settingsConnStatusContainer.querySelector('.status-dot');
        dot.className = 'status-dot error';
        settingsConnStatusText.textContent = 'API Key is empty';
        return;
      }

      settingsConnStatusContainer.style.display = 'flex';
      const dot = settingsConnStatusContainer.querySelector('.status-dot');
      dot.className = 'status-dot testing';
      settingsConnStatusText.textContent = 'Testing connection...';

      try {
        const response = await fetch('/test-connection', {
          method: 'POST',
          headers: {'Content-Type': 'application/json'},
          body: JSON.stringify({
            provider: provider,
            api_key: key,
            base_url: baseUrl || null,
            model: model || null
          })
        });
        const result = await response.json();
        if (response.ok && result.ok) {
          dot.className = 'status-dot success';
          settingsConnStatusText.textContent = 'Connected successfully';
        } else {
          dot.className = 'status-dot error';
          settingsConnStatusText.textContent = result.error || 'Connection failed';
        }
      } catch (e) {
        dot.className = 'status-dot error';
        settingsConnStatusText.textContent = e.message || 'Connection failed';
      }
    });

    // Custom Accent Color listeners
    settingsCustomAccentToggle.addEventListener('change', () => {
      applyCustomAccent(settingsCustomAccentToggle.checked, settingsCustomAccentColor.value);
    });

    settingsCustomAccentColor.addEventListener('input', () => {
      applyCustomAccent(settingsCustomAccentToggle.checked, settingsCustomAccentColor.value);
    });

    settingsToggle.addEventListener('click', () => openSettingsPanel());

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
        applyCustomAccent(false, selectedCustomAccentColor);
      });
    });

    settingsAppTheme.addEventListener('change', () => {
      applyAppTheme(settingsAppTheme.value);
    });

    settingsSaveBtn.addEventListener('click', async () => {
      applyAppTheme(settingsAppTheme.value);
      selectedModel = settingsModel.value;
      selectedProvider = settingsProvider.value;
      selectedLeanMode = settingsLeanMode.value;
      selectedMaxTurns = parseInt(settingsMaxTurns.value, 10) || 64;
      selectedTokenBudget = parseInt(settingsTokenBudget.value, 10) || 12000;

      localStorage.setItem('clawie-model-setting', selectedModel);
      localStorage.setItem('clawie-provider', selectedProvider);
      localStorage.setItem('clawie-openai-key', settingsOpenAiKey.value.trim());
      localStorage.setItem('clawie-anthropic-key', settingsAnthropicKey.value.trim());
      localStorage.setItem('clawie-gemini-key', settingsGeminiKey.value.trim());
      localStorage.setItem('clawie-openai-url', settingsOpenAiUrl.value.trim());
      localStorage.setItem('clawie-gemini-url', settingsGeminiUrl.value.trim());
      localStorage.setItem('clawie-xai-key', settingsXaiKey.value.trim());
      localStorage.setItem('clawie-xai-url', settingsXaiUrl.value.trim());
      localStorage.setItem('clawie-kimi-key', settingsKimiKey.value.trim());
      localStorage.setItem('clawie-kimi-url', settingsKimiUrl.value.trim());
      localStorage.setItem('clawie-lean-mode', selectedLeanMode);
      localStorage.setItem('clawie-max-turns', selectedMaxTurns.toString());
      localStorage.setItem('clawie-token-budget', selectedTokenBudget.toString());

      // Save appearance fields
      applyFontUi(settingsFontUi.value);
      applyFontCode(settingsFontCode.value);
      applyLayoutDensity(settingsLayoutDensity.value);
      applySidebarLayout(settingsSidebarLayout.value);
      applyAmbientGlow(settingsAmbientGlow.checked);
      applyTerminalGlow(settingsTerminalGlow.checked);
      applyStatusPulsate(settingsStatusPulsate.checked);
      applyCustomAccent(settingsCustomAccentToggle.checked, settingsCustomAccentColor.value);

      try {
        const payload = {
          provider: selectedProvider,
          model: selectedModel,
          OPENAI_API_KEY: settingsOpenAiKey.value.trim() || null,
          ANTHROPIC_API_KEY: settingsAnthropicKey.value.trim() || null,
          GEMINI_API_KEY: settingsGeminiKey.value.trim() || null,
          OPENAI_BASE_URL: settingsOpenAiUrl.value.trim() || null,
          GEMINI_BASE_URL: settingsGeminiUrl.value.trim() || null,
          CLAWIE_LEAN_MODE: selectedLeanMode,
          CLAWIE_MAX_TURNS: selectedMaxTurns,
          CLAWIE_MAX_BUDGET_TOKENS: selectedTokenBudget
        };
        await fetch('/api/settings', {
          method: 'POST',
          headers: {'Content-Type': 'application/json'},
          body: JSON.stringify(payload)
        });
      } catch (e) {
        console.error('Failed to persist settings to backend disk', e);
      }

      settingsModal.hidden = true;
      syncInstancePanel();
      setStatus('Settings applied successfully', 'saved');
    });

    window.addEventListener('beforeinstallprompt', event => {
      event.preventDefault();
      deferredInstallPrompt = event;
      settingsInstallStatus.textContent = 'Clawie is ready to install as a browser app. We will launch an IDE soon.';
    });

    window.addEventListener('appinstalled', () => {
      deferredInstallPrompt = null;
      settingsInstallStatus.textContent = 'Clawie was installed successfully.';
      setStatus('Clawie app installed', 'saved');
    });

    settingsInstallApp.addEventListener('click', async () => {
      if (window.matchMedia('(display-mode: standalone)').matches || window.navigator.standalone) {
        settingsInstallStatus.textContent = 'Clawie is already running as an installed app.';
        return;
      }
      if (deferredInstallPrompt) {
        deferredInstallPrompt.prompt();
        const choice = await deferredInstallPrompt.userChoice;
        deferredInstallPrompt = null;
        settingsInstallStatus.textContent = choice.outcome === 'accepted'
          ? 'Install accepted. Clawie will appear in your app launcher. We will launch an IDE soon.'
          : 'Install dismissed. You can try again from your browser install menu. We will launch an IDE soon.';
        return;
      }
      settingsInstallStatus.textContent = 'Use your browser menu to install this page as an app. In Safari, choose File > Add to Dock. We will launch an IDE soon.';
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
        setStatus('Creating ' + filename + '...', 'busy');
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
        setStatus(error.message, 'error');
      }
    });

    if ('serviceWorker' in navigator) {
      navigator.serviceWorker.getRegistrations()
        .then(registrations => Promise.all(registrations.map(registration => registration.unregister())))
        .catch(() => {});
    }
    if ('caches' in window) {
      caches.keys()
        .then(keys => Promise.all(keys.filter(key => key.startsWith('clawie-webui-')).map(key => caches.delete(key))))
        .catch(() => {});
    }

    applyAppTheme(selectedAppTheme);
    applyTheme(selectedTheme);
    updateUsageDisplay(0, 0);
    renderInstanceRooms([]);
    setInstanceZoom(instanceZoom);
    const savedWorkspaceView = localStorage.getItem('clawie-workspace-view');
    setWorkspaceView(['code', 'instance', 'automations'].includes(savedWorkspaceView) ? savedWorkspaceView : 'code');
    initializeAgentDragging();
    setInterval(tickElapsedTimers, 1000);
    setInterval(() => {
      if (!instancePage.hidden) refreshInstances();
    }, 7000);

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
      } catch (error) { setStatus(error.message, 'error'); }
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
      
      const files = await collectDroppedFiles(e.dataTransfer);
      if (!files || files.length === 0) {
        setStatus('No readable files found in the dropped item', 'error');
        return;
      }
      
      setStatus(`Uploading ${files.length} file(s)...`, 'uploading');
      
      let uploadedCount = 0;
      for (const item of files) {
        try {
          const file = item.file;
          const content = await readFileAsText(file);
          const response = await fetch('/upload', {
            method: 'POST',
            headers: {'Content-Type': 'application/json'},
            body: JSON.stringify({
              directory: locationPath.value,
              filename: item.path,
              content: content
            })
          });
          const result = await response.json();
          if (!response.ok || !result.ok) throw new Error(result.error || 'Upload failed');
          uploadedCount += 1;
        } catch (error) {
          appendChatMessage('clawie', `Failed to upload **${item.path}**: ${error.message}`);
        }
      }
      
      await refreshFiles();
      appendChatMessage('clawie', `Added **${uploadedCount}** file(s) to workspace.`);
      setStatus(`Uploaded ${uploadedCount} file(s)`, uploadedCount === files.length ? 'saved' : 'unsaved');
    });

    async function collectDroppedFiles(dataTransfer) {
      const items = Array.from(dataTransfer.items || []);
      if (items.length > 0 && items.some(item => typeof item.webkitGetAsEntry === 'function')) {
        const entries = items
          .filter(item => item.kind === 'file')
          .map(item => item.webkitGetAsEntry())
          .filter(Boolean);
        const nested = [];
        for (const entry of entries) {
          nested.push(...await readEntryFiles(entry, ''));
        }
        return nested;
      }
      return Array.from(dataTransfer.files || []).map(file => ({
        file,
        path: file.webkitRelativePath || file.name
      }));
    }

    async function readEntryFiles(entry, parentPath) {
      const entryPath = parentPath ? `${parentPath}/${entry.name}` : entry.name;
      if (entry.isFile) {
        const file = await entryFile(entry);
        return [{ file, path: entryPath }];
      }
      if (!entry.isDirectory) {
        return [];
      }
      const reader = entry.createReader();
      const children = [];
      let batch = [];
      do {
        batch = await readDirectoryBatch(reader);
        children.push(...batch);
      } while (batch.length > 0);

      const files = [];
      for (const child of children) {
        files.push(...await readEntryFiles(child, entryPath));
      }
      return files;
    }

    function entryFile(entry) {
      return new Promise((resolve, reject) => entry.file(resolve, reject));
    }

    function readDirectoryBatch(reader) {
      return new Promise((resolve, reject) => reader.readEntries(resolve, reject));
    }

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
          setStatus('Listening for speech...', 'listening');
        } catch (e) {
          setStatus('Failed to start speech recognition: ' + e.message, 'error');
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
        setStatus('Speech error: ' + event.error, 'error');
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



    // AUTOMATIONS FLOW BUILDER INTERACTIVE CONTROLLER
    const flowNodesList = document.querySelector('#nodes-layer');
    const autoTriggerName = document.querySelector('#auto-trigger-name');
    const autoTriggerBody = document.querySelector('#auto-trigger-body');
    const autoBtnClear = document.querySelector('#auto-btn-clear');
    const autoBtnSave = document.querySelector('#auto-btn-save');

    let actionNodeCounter = 1;
    let currentZoom = 1.0;
    let panX = 0;
    let panY = 0;

    // Helper: Append Action Node
    window.appendActionNode = function(type, value = '', customTitle = '', value2 = '') {
      actionNodeCounter++;
      const currentId = `action-node-${actionNodeCounter}`;
      
      // Node HTML
      const node = document.createElement('div');
      node.className = `flow-node action-node ${type}-node`;
      node.id = currentId;

      // Layout cascade (auto-calculate coordinate based on node count to lay out horizontally)
      const existingNodes = flowNodesList.querySelectorAll('.flow-node');
      const count = Array.from(existingNodes).filter(n => !n.classList.contains('sticky-note')).length;
      const left = 50 + count * 310;
      const top = 120;
      node.style.left = `${left}px`;
      node.style.top = `${top}px`;
      node.style.position = 'absolute';

      let icon = '🤖';
      let title = customTitle || 'Clawie AI Agent';
      let label = 'ACTION (AI RUNNER)';
      let labelColor = '#3b82f6';
      let bodyHtml = '';

      if (type === 'ai-agent') {
        icon = '🤖';
        title = customTitle || 'Clawie AI Agent';
        label = 'ACTION (AI RUNNER)';
        labelColor = '#3b82f6';
        bodyHtml = `
          <label>AI Prompt Goal</label>
          <textarea rows="2" placeholder="Explain the goal for the agent..." class="auto-node-val">${value || 'Audits code changes and fixes quality warnings.'}</textarea>
        `;
      } else if (type === 'simple-agent') {
        icon = '🧠';
        title = customTitle || 'Create Simple Agent';
        label = 'ACTION (CUSTOM AGENT)';
        labelColor = '#8b5cf6';
        bodyHtml = `
          <label>Agent Name / Persona</label>
          <input type="text" placeholder="e.g. Support Helper Agent" class="auto-node-val" value="${value || 'Support Helper Agent'}">
          <label>System Instructions</label>
          <textarea rows="2" placeholder="Instructions for the agent..." class="auto-node-val-2">${value2 || 'You are a helpful customer support agent for Clawie. Be polite and concise.'}</textarea>
        `;
      } else if (type === 'email') {
        icon = '📧';
        title = customTitle || 'Answer Email';
        label = 'ACTION (EMAIL AUTO-RESPONDER)';
        labelColor = '#ea580c';
        bodyHtml = `
          <label>Email Filter (e.g. subject:support)</label>
          <input type="text" placeholder="e.g. subject:support" class="auto-node-val" value="${value || 'subject:support'}">
          <label>Response Template & AI Instructions</label>
          <textarea rows="2" placeholder="Draft rules for responding..." class="auto-node-val-2">${value2 || 'Draft a polite response answering the customer\'s technical query.'}</textarea>
        `;
      } else if (type === 'summarize') {
        icon = '📝';
        title = customTitle || 'Summarize Text';
        label = 'ACTION (TEXT SUMMARIZATION)';
        labelColor = '#06b6d4';
        bodyHtml = `
          <label>Summary Length & Focus</label>
          <input type="text" placeholder="e.g. 3 bullet points" class="auto-node-val" value="${value || '3 concise bullet points outlining key requests'}">
          <label>Input Source</label>
          <select class="auto-node-val-select" style="width: 100%; padding: 0.4rem; background: var(--bg-main); border: 1px solid var(--border); border-radius: var(--radius-sm); color: var(--text-primary); margin-top: 0.25rem;">
            <option value="trigger-output" ${value2 === 'trigger-output' ? 'selected' : ''}>Output from previous node</option>
            <option value="email-body" ${value2 === 'email-body' ? 'selected' : ''}>Raw Email Body</option>
            <option value="file-content" ${value2 === 'file-content' ? 'selected' : ''}>File Contents</option>
          </select>
        `;
      } else if (type === 'slack') {
        icon = '💬';
        title = customTitle || 'Draft Slack Message';
        label = 'ACTION (SLACK INTEG)';
        labelColor = '#ec4899';
        bodyHtml = `
          <label>Target Channel</label>
          <input type="text" placeholder="e.g. #support-alerts" class="auto-node-val" value="${value || '#support-alerts'}">
          <label>Slack Message Template</label>
          <textarea rows="2" placeholder="Draft slack message..." class="auto-node-val-2">${value2 || '📢 *New Support Alert!* Info: {summarize.output}'}</textarea>
        `;
      } else if (type === 'http') {
        icon = '🌐';
        title = customTitle || 'API Request';
        label = 'ACTION (REST API)';
        labelColor = '#38bdf8';
        bodyHtml = `
          <label>Request Method & URL</label>
          <div style="display: flex; gap: 0.5rem; margin-bottom: 0.25rem;">
            <select class="auto-node-val-select" style="width: auto; padding: 0.4rem; background: var(--bg-main); border: 1px solid var(--border); border-radius: var(--radius-sm); color: var(--text-primary);">
              <option>POST</option>
              <option>GET</option>
              <option>PUT</option>
            </select>
            <input type="text" placeholder="https://api.example.com/v1/alert" class="auto-node-val" style="flex: 1;" value="${value || 'https://api.example.com/v1/alert'}">
          </div>
          <label>Headers (JSON)</label>
          <input type="text" placeholder='{"Content-Type": "application/json"}' class="auto-node-val-2" value='{"Content-Type": "application/json"}'>
        `;
      } else if (type === 'nested') {
        icon = '📦';
        title = customTitle || 'Nested Flow';
        label = 'ACTION (SUB-WORKFLOW)';
        labelColor = '#fb7185';
        bodyHtml = `
          <label>Select Target Flow</label>
          <select class="auto-node-val-select" style="width: 100%; padding: 0.4rem; background: var(--bg-main); border: 1px solid var(--border); border-radius: var(--radius-sm); color: var(--text-primary);">
            <option value="commit-sync">Daily Commit Sync</option>
            <option value="code-fix">Code Guard Auto-Fix</option>
            <option value="notifications">Broadcaster System</option>
          </select>
          <label>Pass Input Data</label>
          <input type="text" class="auto-node-val" value="${value || '{trigger.data}'}">
        `;
      } else if (type === 'routing') {
        icon = '🔀';
        title = customTitle || 'Routing Guard';
        label = 'ACTION (CONDITIONAL)';
        labelColor = '#fbbf24';
        bodyHtml = `
          <label>Check Condition</label>
          <div style="display: flex; gap: 0.5rem; margin-bottom: 0.25rem;">
            <input type="text" placeholder="Variable" class="auto-node-val" style="width: 40%;" value="${value || '{agent.exit_code}'}">
            <select class="auto-node-val-select" style="width: auto; padding: 0.4rem; background: var(--bg-main); border: 1px solid var(--border); border-radius: var(--radius-sm); color: var(--text-primary);">
              <option value="eq">equals</option>
              <option value="ne">not equals</option>
              <option value="contains">contains</option>
            </select>
            <input type="text" placeholder="Value" class="auto-node-val-2" style="width: 40%;" value="${value2 || '0'}">
          </div>
        `;
      } else if (type === 'iterator') {
        icon = '🔄';
        title = customTitle || 'Batch Iterator';
        label = 'ACTION (LOOP)';
        labelColor = '#34d399';
        bodyHtml = `
          <label>Input Items Array</label>
          <input type="text" placeholder="e.g. {trigger.changed_files}" class="auto-node-val" value="${value || '{trigger.changed_files}'}">
          <label>Batch Size (Parallel execution)</label>
          <input type="number" class="auto-node-val-2" style="width: 80px;" value="${value2 || '1'}">
        `;
      } else if (type === 'bash') {
        icon = '💻';
        title = customTitle || 'Execute Shell Script';
        label = 'ACTION (SHELL COMMAND)';
        labelColor = '#10b981';
        bodyHtml = `
          <label>Bash Command</label>
          <input type="text" placeholder="e.g. cargo test, npm test" class="auto-node-val" value="${value || 'cargo test'}">
        `;
      } else if (type === 'notify') {
        icon = '✉️';
        title = customTitle || 'Send Notification';
        label = 'ACTION (NOTIFICATION)';
        labelColor = '#f59e0b';
        bodyHtml = `
          <label>Notification Message</label>
          <input type="text" placeholder="e.g. Workflow complete!" class="auto-node-val" value="${value || 'Automation workflow executed successfully!'}">
        `;
      }

      node.innerHTML = `
        <div class="node-header">
          <div class="node-icon" style="background: rgba(255,255,255,0.03); color: ${labelColor};">${icon}</div>
          <div class="node-title-wrap">
            <span class="node-label" style="color: ${labelColor};">${label}</span>
            <strong class="node-name">${title}</strong>
          </div>
          <button class="node-remove-btn" type="button" onclick="removeActionNode('${currentId}')">&times;</button>
        </div>
        <div class="node-body">
          ${bodyHtml}
          
          <!-- Expandable Fail-safe & Settings -->
          <details class="node-settings-details" style="margin-top: 0.5rem; font-size: 0.7rem; border-top: 1px dashed var(--border); padding-top: 0.5rem;">
            <summary style="cursor: pointer; color: var(--text-muted); user-select: none;">⚙️ Fail-safe Guards & Settings</summary>
            <div style="margin-top: 0.35rem; display: flex; flex-direction: column; gap: 0.25rem; padding-left: 0.5rem;">
              <label style="display: flex; align-items: center; gap: 0.35rem; color: var(--text-muted);">
                <input type="checkbox" class="node-opt-retry" style="margin: 0;"> Auto-retry on failure (up to 3 times)
              </label>
              <label style="display: flex; align-items: center; gap: 0.35rem; color: var(--text-muted);">
                <input type="checkbox" class="node-opt-continue" style="margin: 0;"> Continue workflow on error
              </label>
            </div>
          </details>

          <!-- Node Simulation & Payload Inspector -->
          <div class="node-simulator" style="margin-top: 0.5rem; display: flex; flex-direction: column; gap: 0.25rem; border-top: 1px dashed var(--border); padding-top: 0.5rem;">
            <div style="display: flex; justify-content: space-between; align-items: center;">
              <span style="font-size: 0.7rem; color: var(--text-muted);">🔬 Payload Inspector</span>
              <button class="node-sim-btn" type="button" style="padding: 0.15rem 0.4rem; font-size: 0.65rem; background: var(--bg-main); border: 1px solid var(--border); border-radius: var(--radius-sm); cursor: pointer; color: var(--text-primary); transition: all 0.2s;" onclick="simulateNode('${currentId}')">▶️ Simulate Node</button>
            </div>
            <div class="node-inspector-payload" style="display: none; background: rgba(0,0,0,0.2); border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 0.4rem; font-family: monospace; font-size: 0.65rem; color: #10b981; max-height: 100px; overflow-y: auto; margin-top: 0.25rem;">
            </div>
          </div>
        </div>
      `;

      flowNodesList.appendChild(node);
      
      initDraggable(node);
      addPorts(node, true, true);
      setupNodeClick(node);
      
      redrawConnections();
    };

    // Helper: Remove action node
    window.removeActionNode = function(nodeId) {
      const node = document.getElementById(nodeId);
      if (node) node.remove();
      redrawConnections();
    };

    // Helper: Add connection ports (bullets) to a node
    window.addPorts = function(node, hasInput, hasOutput) {
      if (hasInput && !node.querySelector('.input-port')) {
        const inputPort = document.createElement('div');
        inputPort.className = 'node-port input-port';
        node.appendChild(inputPort);
      }
      if (hasOutput && !node.querySelector('.output-port')) {
        const outputPort = document.createElement('div');
        outputPort.className = 'node-port output-port';
        node.appendChild(outputPort);
      }
    };

    // Helper: Setup click to open configuration drawer
    window.setupNodeClick = function(node) {
      node.addEventListener('click', (e) => {
        if (e.target.closest('.node-sim-btn') || e.target.closest('.node-remove-btn') || e.target.closest('.node-settings-details') || e.target.closest('input') || e.target.closest('textarea') || e.target.closest('select')) {
          return;
        }
        openNodeDrawer(node.id);
      });
    };

    // Helper: Initialize dragging for a node
    let activeDragNode = null;
    let dragStartX = 0;
    let dragStartY = 0;
    let nodeStartX = 0;
    let nodeStartY = 0;

    window.initDraggable = function(node) {
      const header = node.querySelector('.node-header');
      if (!header) return;
      header.style.cursor = 'move';
      header.addEventListener('mousedown', (e) => {
        if (e.target.closest('.node-remove-btn')) return;
        activeDragNode = node;
        dragStartX = e.clientX;
        dragStartY = e.clientY;
        nodeStartX = parseInt(node.style.left) || 0;
        nodeStartY = parseInt(node.style.top) || 0;
        node.style.zIndex = 100;
        e.preventDefault();
      });
    };

    // Mouse Move listener for node drag & canvas pan
    let isPanning = false;
    let panStartX = 0;
    let panStartY = 0;

    document.addEventListener('mousemove', (e) => {
      if (activeDragNode) {
        const dx = (e.clientX - dragStartX) / currentZoom;
        const dy = (e.clientY - dragStartY) / currentZoom;
        activeDragNode.style.left = `${nodeStartX + dx}px`;
        activeDragNode.style.top = `${nodeStartY + dy}px`;
        redrawConnections();
      } else if (isPanning) {
        panX = e.clientX - panStartX;
        panY = e.clientY - panStartY;
        updateCanvasTransform();
      }
    });

    document.addEventListener('mouseup', () => {
      if (activeDragNode) {
        activeDragNode.style.zIndex = 2;
        activeDragNode = null;
      }
      if (isPanning) {
        isPanning = false;
        document.getElementById('canvas-viewport').style.cursor = 'grab';
      }
    });

    // Panning bindings on viewport background
    setTimeout(() => {
      const gridOverlay = document.getElementById('canvas-grid-overlay');
      if (gridOverlay) {
        gridOverlay.addEventListener('mousedown', (e) => {
          isPanning = true;
          panStartX = e.clientX - panX;
          panStartY = e.clientY - panY;
          document.getElementById('canvas-viewport').style.cursor = 'grabbing';
        });
      }
      
      const viewport = document.getElementById('canvas-viewport');
      if (viewport) {
        viewport.addEventListener('wheel', (e) => {
          e.preventDefault();
          const zoomFactor = 0.05;
          const amount = e.deltaY < 0 ? zoomFactor : -zoomFactor;
          adjustZoom(amount);
        }, { passive: false });
      }
    }, 500);

    // Zoom handlers
    window.adjustZoom = function(amount) {
      currentZoom = Math.min(Math.max(currentZoom + amount, 0.4), 1.8);
      updateCanvasTransform();
    };

    window.resetZoom = function() {
      currentZoom = 1.0;
      panX = 0;
      panY = 0;
      updateCanvasTransform();
    };

    window.updateCanvasTransform = function() {
      const nodesLayer = document.getElementById('nodes-layer');
      const connectionSvg = document.getElementById('connection-svg');
      if (nodesLayer) nodesLayer.style.transform = `translate(${panX}px, ${panY}px) scale(${currentZoom})`;
      if (connectionSvg) connectionSvg.style.transform = `translate(${panX}px, ${panY}px) scale(${currentZoom})`;
    };

    // SVG S-Curve Path Calculator
    window.drawCurve = function(x1, y1, x2, y2) {
      const ctrlX1 = x1 + 100;
      const ctrlY1 = y1;
      const ctrlX2 = x2 - 100;
      const ctrlY2 = y2;
      return `M ${x1} ${y1} C ${ctrlX1} ${ctrlY1}, ${ctrlX2} ${ctrlY2}, ${x2} ${y2}`;
    };

    // Redraw SVG connections between adjacent nodes in flowNodesList
    window.redrawConnections = function() {
      const svg = document.getElementById('connection-svg');
      if (!svg) return;
      svg.innerHTML = '';
      
      const nodes = Array.from(flowNodesList.querySelectorAll('.flow-node'));
      if (nodes.length < 2) return;
      
      for (let i = 0; i < nodes.length - 1; i++) {
        const nodeA = nodes[i];
        const nodeB = nodes[i+1];
        
        // Skip sticky notes in paths
        if (nodeA.classList.contains('sticky-note') || nodeB.classList.contains('sticky-note')) {
          continue;
        }
        
        const x1 = parseInt(nodeA.style.left) + 260; // Node Width is 260px
        const y1 = parseInt(nodeA.style.top) + 35;  // Middle of header
        
        const x2 = parseInt(nodeB.style.left);
        const y2 = parseInt(nodeB.style.top) + 35;
        
        const path = document.createElementNS('http://www.w3.org/2000/svg', 'path');
        const d = drawCurve(x1, y1, x2, y2);
        path.setAttribute('d', d);
        path.setAttribute('stroke', 'var(--border)');
        path.setAttribute('stroke-width', '3');
        path.setAttribute('fill', 'none');
        path.setAttribute('class', 'connection-path');
        svg.appendChild(path);
      }
    };

    // Configuration drawer controls
    let activeConfigNodeId = null;

    window.openNodeDrawer = function(nodeId) {
      activeConfigNodeId = nodeId;
      const node = document.getElementById(nodeId);
      const title = node.querySelector('.node-name').textContent;
      const body = node.querySelector('.node-body');
      
      document.getElementById('drawer-node-title').innerHTML = `⚙️ Configure ${title}`;
      
      const drawerBody = document.getElementById('drawer-node-body');
      drawerBody.innerHTML = '';
      
      const inputs = body.querySelectorAll('input, textarea, select');
      inputs.forEach(input => {
        if (input.closest('.node-simulator') || input.closest('.node-settings-details')) return;
        
        const labelText = input.previousElementSibling ? input.previousElementSibling.textContent : '';
        const wrapper = document.createElement('div');
        wrapper.className = 'drawer-field-wrap';
        wrapper.innerHTML = `<label>${labelText}</label>`;
        
        const clonedInput = input.cloneNode(true);
        clonedInput.value = input.value;
        clonedInput.dataset.targetClass = Array.from(input.classList).join('.');
        
        wrapper.appendChild(clonedInput);
        drawerBody.appendChild(wrapper);
      });
      
      document.getElementById('node-config-drawer').style.right = '0';
    };

    window.closeDrawer = function() {
      document.getElementById('node-config-drawer').style.right = '-400px';
      activeConfigNodeId = null;
    };

    window.saveDrawerSettings = function() {
      if (!activeConfigNodeId) return;
      const node = document.getElementById(activeConfigNodeId);
      const body = node.querySelector('.node-body');
      const drawerBody = document.getElementById('drawer-node-body');
      
      const drawerInputs = drawerBody.querySelectorAll('input, textarea, select');
      drawerInputs.forEach(dInput => {
        const targetClass = dInput.dataset.targetClass;
        const targetInput = body.querySelector(`.${targetClass}`);
        if (targetInput) {
          targetInput.value = dInput.value;
        }
      });
      
      setStatus('Node configurations saved', 'saved');
      if (window.logAutomationEvent) {
        const title = node.querySelector('.node-name')?.textContent || 'Node';
        window.logAutomationEvent(`Configured settings for node: ${title}`);
      }
      closeDrawer();
    };

    // Helper: Append Sticky Note
    window.appendStickyNote = function() {
      actionNodeCounter++;
      const currentId = `sticky-note-${actionNodeCounter}`;
      
      const note = document.createElement('div');
      note.className = 'flow-node sticky-note';
      note.id = currentId;
      note.style.position = 'absolute';
      note.style.left = '150px';
      note.style.top = '280px';
      note.style.width = '200px';
      note.style.background = '#fef08a';
      note.style.color = '#1e293b';
      note.style.padding = '0.75rem';
      note.style.borderRadius = 'var(--radius-sm)';
      note.style.boxShadow = 'var(--panel-shadow)';
      note.style.border = '1px solid #fde047';
      note.style.zIndex = 10;
      
      note.innerHTML = `
        <div style="display: flex; justify-content: space-between; align-items: center; border-bottom: 1px dashed rgba(0,0,0,0.1); padding-bottom: 0.25rem; margin-bottom: 0.5rem; font-size: 0.65rem; color: #64748b;">
          <strong>📌 NOTE</strong>
          <button style="background: none; border: none; font-size: 1rem; color: #64748b; cursor: pointer;" onclick="document.getElementById('${currentId}').remove()">&times;</button>
        </div>
        <textarea style="width: 100%; border: none; background: transparent; resize: none; font-size: 0.75rem; color: #1e293b; outline: none;" rows="3" placeholder="Type notes here..."></textarea>
      `;
      
      flowNodesList.appendChild(note);
      initDraggable(note);
    };

    // Set Trigger Type
    window.setTriggerType = function(type) {
      if (type === 'file') {
        autoTriggerName.textContent = 'On File Save';
        autoTriggerBody.innerHTML = `
          <label>File Glob Path Match</label>
          <input type="text" value="**/*.rs" placeholder="e.g. **/*.rs, **/*.py" id="auto-input-trigger-glob">
        `;
      } else if (type === 'cron') {
        autoTriggerName.textContent = 'Cron Schedule';
        autoTriggerBody.innerHTML = `
          <label>Cron Expression</label>
          <input type="text" value="*/30 * * * *" placeholder="e.g. */15 * * * *" id="auto-input-trigger-cron">
        `;
      } else if (type === 'git') {
        autoTriggerName.textContent = 'Git Pre-commit';
        autoTriggerBody.innerHTML = `
          <label>Git Hook Stage</label>
          <input type="text" value="pre-commit" readonly id="auto-input-trigger-git">
        `;
      } else if (type === 'webhook') {
        autoTriggerName.textContent = 'Webhook Listener';
        autoTriggerBody.innerHTML = `
          <label>Webhook URL (Endpoint path)</label>
          <input type="text" value="/webhooks/clawie-receive" readonly id="auto-input-trigger-webhook">
          <div style="font-size: 0.68rem; color: var(--text-muted); margin-top: 0.25rem;">Triggers when POST request is sent to http://127.0.0.1:port/webhooks/clawie-receive</div>
        `;
      }
    };

    // Bind Tools Trigger clicks
    document.querySelector('#tool-trigger-file').addEventListener('click', () => setTriggerType('file'));
    document.querySelector('#tool-trigger-cron').addEventListener('click', () => setTriggerType('cron'));
    document.querySelector('#tool-trigger-git').addEventListener('click', () => setTriggerType('git'));
    document.querySelector('#tool-trigger-webhook').addEventListener('click', () => setTriggerType('webhook'));

    // Bind Tools Action clicks
    document.querySelector('#tool-action-agent').addEventListener('click', () => appendActionNode('ai-agent'));
    document.querySelector('#tool-action-custom-agent').addEventListener('click', () => appendActionNode('simple-agent'));
    document.querySelector('#tool-action-email').addEventListener('click', () => appendActionNode('email'));
    document.querySelector('#tool-action-summarize').addEventListener('click', () => appendActionNode('summarize'));
    document.querySelector('#tool-action-slack').addEventListener('click', () => appendActionNode('slack'));
    document.querySelector('#tool-action-http').addEventListener('click', () => appendActionNode('http'));
    document.querySelector('#tool-action-nested').addEventListener('click', () => appendActionNode('nested'));
    document.querySelector('#tool-action-routing').addEventListener('click', () => appendActionNode('routing'));
    document.querySelector('#tool-action-iterator').addEventListener('click', () => appendActionNode('iterator'));
    document.querySelector('#tool-action-bash').addEventListener('click', () => appendActionNode('bash'));
    document.querySelector('#tool-action-notify').addEventListener('click', () => appendActionNode('notify'));
    document.querySelector('#tool-action-sticky').addEventListener('click', () => appendStickyNote());

    // Bind Clear button
    autoBtnClear.addEventListener('click', () => {
      const trigger = document.getElementById('trigger-node-main');
      const nodes = Array.from(flowNodesList.children);
      nodes.forEach(node => {
        if (node !== trigger) {
          node.remove();
        }
      });
      
      setTriggerType('file');
      resetZoom();
      redrawConnections();
      setStatus('Automation canvas reset', 'saved');
    });

    // Preset Templates Loader
    document.querySelector('#tpl-code-guard').addEventListener('click', () => {
      autoBtnClear.click();
      setTriggerType('file');
      document.querySelector('#auto-input-trigger-glob').value = '**/*.rs';
      appendActionNode('bash', 'cargo test', 'Run Tests');
      appendActionNode('ai-agent', 'Fix all compile errors and failing cargo test warnings found in the repository.', 'Clawie Auto-Fixer');
      setStatus('Loaded "Code Guard" template', 'saved');
    });

    document.querySelector('#tpl-auto-sync').addEventListener('click', () => {
      autoBtnClear.click();
      setTriggerType('git');
      appendActionNode('ai-agent', 'Analyze all staged files in git and generate a detailed commit description using standard conventional commits guidelines.', 'Clawie Commit Summarizer');
      appendActionNode('notify', 'Commit description generated and staged.', 'Emit Alert Notification');
      setStatus('Loaded "Daily Commit Sync" template', 'saved');
    });

    document.querySelector('#tpl-email-responder').addEventListener('click', () => {
      autoBtnClear.click();
      setTriggerType('cron');
      document.querySelector('#auto-input-trigger-cron').value = '*/15 * * * *';
      appendActionNode('simple-agent', 'Support Helper Agent', 'You are a helpful customer support agent for Clawie. Be polite and concise.');
      appendActionNode('summarize', '3 concise bullet points outlining key requests', 'trigger-output');
      appendActionNode('email', 'subject:support', 'Draft a polite response answering the customer\'s technical query.');
      appendActionNode('slack', '#support-alerts', '📢 *New Support Alert!* Email summarized and responded.');
      setStatus('Loaded "AI Email Support" template', 'saved');
    });

    document.querySelector('#tpl-custom-agent').addEventListener('click', () => {
      autoBtnClear.click();
      setTriggerType('file');
      document.querySelector('#auto-input-trigger-glob').value = '**/*.rs';
      appendActionNode('simple-agent', 'Rust Expert Auditor', 'You are a Senior Rust compiler specialist. Review the code for safety and performance.');
      appendActionNode('ai-agent', 'Run the custom auditor agent to perform linting.', 'Clawie AI Agent');
      appendActionNode('notify', 'Custom agent execution complete.', 'Emit Alert Notification');
      setStatus('Loaded "AI Custom Agent" template', 'saved');
    });

    document.querySelector('#tpl-webhook-http').addEventListener('click', () => {
      autoBtnClear.click();
      setTriggerType('webhook');
      appendActionNode('routing', '{agent.exit_code}', 'Conditional Branch', '0');
      appendActionNode('iterator', '{trigger.changed_files}', 'Loop Over Files', '1');
      appendActionNode('http', 'https://api.example.com/v1/alert', 'Call API Endpoint');
      appendActionNode('slack', '#support-alerts', 'Notify Team on Slack');
      setStatus('Loaded "API Webhook Sync" template', 'saved');
    });

    document.querySelector('#tpl-cron-runner').addEventListener('click', () => {
      autoBtnClear.click();
      setTriggerType('cron');
      document.querySelector('#auto-input-trigger-cron').value = '0 9 * * *';
      appendActionNode('bash', './scripts/daily-backup.sh', 'Execute Backup Script');
      appendActionNode('notify', 'Daily file sync completed successfully.', 'Emit Alert Notification');
      setStatus('Loaded "Cron Runner" template', 'saved');
    });

    // Save Button Dropdown Toggle
    const saveDropdown = document.querySelector('#workflow-save-dropdown');
    autoBtnSave.addEventListener('click', (e) => {
      e.stopPropagation();
      const isVisible = saveDropdown.style.display === 'block';
      saveDropdown.style.display = isVisible ? 'none' : 'block';
    });

    // Close dropdown on click outside
    document.addEventListener('click', () => {
      if (saveDropdown) saveDropdown.style.display = 'none';
    });

    // Extract workflow JSON representation
    window.getWorkflowJson = function() {
      const triggerType = autoTriggerName.textContent;
      let triggerVal = '';
      const globInput = document.getElementById('auto-input-trigger-glob');
      const cronInput = document.getElementById('auto-input-trigger-cron');
      const gitInput = document.getElementById('auto-input-trigger-git');
      const webhookInput = document.getElementById('auto-input-trigger-webhook');
      if (globInput) triggerVal = globInput.value;
      else if (cronInput) triggerVal = cronInput.value;
      else if (gitInput) triggerVal = gitInput.value;
      else if (webhookInput) triggerVal = webhookInput.value;

      const nodes = Array.from(flowNodesList.querySelectorAll('.flow-node'));
      const actions = [];
      const stickyNotes = [];

      nodes.forEach(n => {
        if (n.id === 'trigger-node-main') return;
        
        if (n.classList.contains('sticky-note')) {
          const textarea = n.querySelector('textarea');
          stickyNotes.push({
            id: n.id,
            text: textarea ? textarea.value : '',
            left: n.style.left,
            top: n.style.top
          });
          return;
        }

        const title = n.querySelector('.node-name').textContent;
        const type = Array.from(n.classList).find(c => c.endsWith('-node'))?.replace('-node', '') || 'unknown';
        
        // Find input values
        const inputs = Array.from(n.querySelectorAll('.auto-node-val, .auto-node-val-2, .auto-node-val-select'));
        const values = inputs.map(inp => inp.value);
        
        const optRetry = n.querySelector('.node-opt-retry')?.checked || false;
        const optContinue = n.querySelector('.node-opt-continue')?.checked || false;

        actions.push({
          id: n.id,
          type: type,
          title: title,
          values: values,
          settings: {
            retryOnFailure: optRetry,
            continueOnError: optContinue
          },
          left: n.style.left,
          top: n.style.top
        });
      });

      return JSON.stringify({
        trigger: {
          type: triggerType,
          value: triggerVal
        },
        actions: actions,
        stickyNotes: stickyNotes,
        zoom: currentZoom,
        panX: panX,
        panY: panY
      }, null, 2);
    };

    // Save in Clawie Action
    document.querySelector('#btn-save-clawie').addEventListener('click', () => {
      const workflowData = getWorkflowJson();
      localStorage.setItem('clawie-saved-workflow', workflowData);
      setStatus('Workflow successfully saved in Clawie config!', 'saved');
      if (window.logAutomationEvent) window.logAutomationEvent('Workflow successfully saved in Clawie config');
    });

    // Save as JSON Action (Open custom modal)
    const savePathModal = document.querySelector('#save-path-modal');
    const savePathInput = document.querySelector('#save-path-input');

    document.querySelector('#btn-save-json').addEventListener('click', () => {
      document.querySelector('#modal-workspace-path').textContent = locationPath.value || '/Users/horatiubudai';
      savePathModal.hidden = false;
      savePathInput.focus();
    });

    // Modal Cancel Handlers
    const closeSaveModal = () => {
      savePathModal.hidden = true;
    };
    document.querySelector('#save-path-cancel').addEventListener('click', closeSaveModal);
    document.querySelector('#save-path-cancel-x').addEventListener('click', closeSaveModal);

    // Modal Save Confirm Handler
    document.querySelector('#save-destination-select').addEventListener('change', (e) => {
      document.querySelector('#save-workspace-options').style.display = e.target.value === 'workspace' ? 'block' : 'none';
    });

    document.querySelector('#save-path-confirm').addEventListener('click', async () => {
      const dest = document.querySelector('#save-destination-select').value;
      const workflowData = getWorkflowJson();

      if (dest === 'downloads') {
        const blob = new Blob([workflowData], {type: 'application/json'});
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = 'workflow.json';
        a.click();
        URL.revokeObjectURL(url);
        
        setStatus('Workflow successfully downloaded!', 'saved');
        if (window.logAutomationEvent) window.logAutomationEvent('Workflow successfully saved as JSON file to Downloads');
        closeSaveModal();
        return;
      }
      
      const filename = savePathInput.value.trim() || 'workflow.json';
      
      try {
        setStatus('Saving workflow to JSON file...', 'uploading');
        const response = await fetch('/upload', {
          method: 'POST',
          headers: {'Content-Type': 'application/json'},
          body: JSON.stringify({
            directory: locationPath.value,
            filename: filename,
            content: workflowData
          })
        });
        const result = await response.json();
        if (!response.ok || !result.ok) throw new Error(result.error || 'Upload failed');
        
        setStatus(`Workflow successfully saved to ${filename}!`, 'saved');
        if (window.logAutomationEvent) window.logAutomationEvent(`Workflow successfully saved as JSON file: ${filename}`);
        closeSaveModal();
      } catch (error) {
        setStatus(`Failed to save JSON: ${error.message}`, 'error');
      }
    });

    // Node Simulator Logic
    // Log automation execution event helper
    window.logAutomationEvent = function(message) {
      const now = Date.now();
      localStorage.setItem('clawie-last-automation-activity', String(now));
      
      const logs = JSON.parse(localStorage.getItem('clawie-automation-logs') || '[]');
      const oneHourAgo = now - 3600000;
      const cleanLogs = logs.filter(item => item.timestamp > oneHourAgo);
      
      cleanLogs.push({
        timestamp: now,
        message: message
      });
      
      localStorage.setItem('clawie-automation-logs', JSON.stringify(cleanLogs));
      
      if (typeof refreshInstances === 'function' && !document.querySelector('#instance-page').hidden) {
        refreshInstances();
      }
    };

    window.simulateNode = function(nodeId) {
      const node = document.getElementById(nodeId);
      if (!node) return;
      const simBtn = node.querySelector('.node-sim-btn');
      const inspector = node.querySelector('.node-inspector-payload');
      
      const originalText = simBtn.innerHTML;
      simBtn.innerHTML = '⏳ Simulating...';
      simBtn.disabled = true;
      
      const nodeNameElement = node.querySelector('.node-name');
      const nodeName = nodeNameElement ? nodeNameElement.textContent : 'Node';
      if (window.logAutomationEvent) window.logAutomationEvent(`Simulation started for node: ${nodeName}`);

      const paths = document.querySelectorAll('.connection-path');
      paths.forEach(p => p.classList.add('simulating'));
      
      setTimeout(() => {
        simBtn.innerHTML = '✅ Done';
        inspector.style.display = 'block';
        
        let mockOutput = {};
        
        if (node.classList.contains('ai-agent-node')) {
          mockOutput = {
            "status": "success",
            "findings": 0,
            "actions_taken": "Audited local files. No security compliance issues found.",
            "output_tokens": 348
          };
        } else if (node.classList.contains('simple-agent-node')) {
          mockOutput = {
            "agent_id": "agent_" + Math.floor(Math.random() * 1000),
            "status": "active",
            "persona": "Customer Support Helper",
            "instructions_applied": true
          };
        } else if (node.classList.contains('email-node')) {
          mockOutput = {
            "email_id": "msg_938127",
            "status": "draft_created",
            "to": "customer@example.com",
            "draft_body": "Hello! Thank you for reaching out. We have received your request..."
          };
        } else if (node.classList.contains('summarize-node')) {
          mockOutput = {
            "source_type": "trigger-output",
            "bullet_points": [
              "User is requesting technical documentation access",
              "Action: Email responder agent initiated",
              "Severity: Normal"
            ]
          };
        } else if (node.classList.contains('slack-node')) {
          mockOutput = {
            "channel": "#support-alerts",
            "delivered": true,
            "timestamp": new Date().toISOString()
          };
        } else if (node.classList.contains('http-node')) {
          mockOutput = {
            "response_code": 200,
            "response_body": {
              "ok": true,
              "message": "API call completed successfully."
            }
          };
        } else if (node.classList.contains('nested-node')) {
          mockOutput = {
            "sub_workflow_id": "wf_daily_sync",
            "status": "triggered_async",
            "execution_id": "exec_847192"
          };
        } else if (node.classList.contains('routing-node')) {
          mockOutput = {
            "evaluated_condition": "{agent.exit_code} == 0",
            "result": true,
            "routed_branch": "success"
          };
        } else if (node.classList.contains('iterator-node')) {
          mockOutput = {
            "loop_count": 3,
            "active_item": "src/main.rs",
            "status": "iterating"
          };
        } else {
          mockOutput = {
            "exit_code": 0,
            "stdout": "Command completed successfully."
          };
        }
        
        inspector.innerHTML = `<strong>Input:</strong> { ... }<br><strong>Output:</strong> ${JSON.stringify(mockOutput, null, 2)}`;
        if (window.logAutomationEvent) window.logAutomationEvent(`Node ${nodeName} simulation completed. Output: ${JSON.stringify(mockOutput)}`);

        setTimeout(() => {
          simBtn.innerHTML = originalText;
          simBtn.disabled = false;
        }, 1500);
      }, 800);
    };

    // Easter egg mode disabled - builder remains fully active
  </script>
</body>
</html>"##;

const WEB_APP_MANIFEST: &str = r##"{
  "name": "Clawie Workspace",
  "short_name": "Clawie",
  "description": "A local browser workspace for Clawie coding sessions.",
  "start_url": "/",
  "scope": "/",
  "display": "standalone",
  "background_color": "#09090b",
  "theme_color": "#f97316",
  "icons": [
    {
      "src": "/icon.svg",
      "sizes": "any",
      "type": "image/svg+xml",
      "purpose": "any maskable"
    }
  ]
}"##;

const WEB_APP_ICON_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512">
  <rect width="512" height="512" rx="112" fill="#09090b"/>
  <circle cx="256" cy="256" r="186" fill="#f97316" opacity=".18"/>
  <text x="256" y="314" text-anchor="middle" font-size="260" font-family="Apple Color Emoji, Segoe UI Emoji, Noto Color Emoji, sans-serif">🦐</text>
</svg>"##;

const SERVICE_WORKER_JS: &str = r##"self.addEventListener('install', event => {
  self.skipWaiting();
});

self.addEventListener('activate', event => {
  event.waitUntil(
    caches.keys()
      .then(keys => Promise.all(keys.filter(key => key.startsWith('clawie-webui-')).map(key => caches.delete(key))))
      .then(() => self.registration.unregister())
      .then(() => self.clients.matchAll())
      .then(clients => Promise.all(clients.map(client => client.navigate(client.url))))
  );
});
"##;

#[cfg(test)]
mod tests {
    use super::{
        clean_api_key, list_code_files, load_workspace_files, resolve_output_directory,
        safe_filename, safe_relative_path, save_workspace_files, SaveRequest,
    };
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn rejects_paths_outside_the_documents_workspace() {
        assert!(safe_filename("../secret.txt").is_err());
        assert!(safe_filename("folder/code.rs").is_err());
        assert!(safe_filename("code.rs").is_ok());
    }

    #[test]
    fn accepts_nested_upload_paths_without_escape_segments() {
        assert_eq!(
            safe_relative_path("src/main.rs").expect("nested path"),
            Path::new("src").join("main.rs")
        );
        assert!(safe_relative_path("../secret.txt").is_err());
        assert!(safe_relative_path("src/../secret.txt").is_err());
        assert!(safe_relative_path("/tmp/secret.txt").is_err());
        assert!(safe_relative_path("").is_err());
    }

    #[test]
    fn ignores_placeholder_api_keys_from_webui_settings() {
        assert_eq!(clean_api_key(Some("dummy")), None);
        assert_eq!(clean_api_key(Some("test-dummy-key")), None);
        assert_eq!(clean_api_key(Some("  ")), None);
        assert_eq!(clean_api_key(Some("sk-real")), Some("sk-real"));
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
