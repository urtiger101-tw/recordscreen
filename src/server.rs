use crate::windows_capture::WindowTarget;
use crate::{capture_screenshot, current_status, start_recording, windows_capture, Status};
use axum::{
    body::Body,
    extract::State,
    http::{header, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    io::{BufRead, Write},
    path::PathBuf,
    process::Child,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct AppState {
    pub ffmpeg: Option<PathBuf>,
    pub output_dir: Arc<Mutex<PathBuf>>,
    pub recording: Arc<Mutex<Option<RecordingProcess>>>,
    pub selected_target: Arc<Mutex<Option<WindowTarget>>>,
    pub token: Arc<String>,
}

pub struct RecordingProcess {
    pub child: Child,
    pub path: PathBuf,
    pub started: Instant,
}

#[derive(Deserialize)]
pub struct StartRequest {
    #[serde(default = "default_fps")]
    fps: u32,
}

fn default_fps() -> u32 {
    30
}

#[derive(serde::Serialize)]
struct FileResponse {
    ok: bool,
    file: String,
}

#[derive(serde::Serialize)]
struct ErrorResponse {
    ok: bool,
    error: String,
}

#[derive(Deserialize)]
struct SelectTargetRequest {
    hwnd: Option<usize>,
}

#[derive(serde::Serialize)]
struct WindowsResponse {
    windows: Vec<WindowTarget>,
}

pub async fn serve(state: AppState) {
    let protected = Router::new()
        .route("/api/v1/status", get(status))
        .route("/api/v1/windows", get(list_windows))
        .route("/api/v1/target", post(select_target))
        .route("/api/v1/screenshot", post(screenshot))
        .route("/api/v1/recording/start", post(start))
        .route("/api/v1/recording/stop", post(stop))
        .route_layer(middleware::from_fn_with_state(state.clone(), authorize))
        .with_state(state.clone());
    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .merge(protected)
        .layer(middleware::from_fn(check_local_request));

    if let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:17321").await {
        let _ = axum::serve(listener, app).await;
    }
}

async fn check_local_request(req: Request<Body>, next: Next) -> Response {
    let host_ok = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|host| host == "127.0.0.1:17321" || host == "localhost:17321")
        .unwrap_or(false);
    let origin_ok = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(|origin| origin == "http://127.0.0.1:17321" || origin == "http://localhost:17321")
        .unwrap_or(true);
    if !host_ok || !origin_ok {
        return (StatusCode::FORBIDDEN, "loopback requests only").into_response();
    }
    next.run(req).await
}

async fn authorize(State(state): State<AppState>, req: Request<Body>, next: Next) -> Response {
    let supplied = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if supplied != Some(state.token.as_str()) {
        return (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response();
    }
    next.run(req).await
}

async fn status(State(state): State<AppState>) -> Json<Status> {
    clear_finished_recording(&state);
    Json(current_status(&state))
}

async fn list_windows() -> Json<WindowsResponse> {
    Json(WindowsResponse {
        windows: windows_capture::enumerate_windows(),
    })
}

async fn select_target(
    State(state): State<AppState>,
    Json(request): Json<SelectTargetRequest>,
) -> Response {
    let recording = match state.recording.lock() {
        Ok(recording) => recording,
        Err(_) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "recording state unavailable",
            )
        }
    };
    if recording.is_some() {
        return api_error(
            StatusCode::CONFLICT,
            "stop recording before changing the capture target",
        );
    }
    let target = match request.hwnd {
        Some(hwnd) => match windows_capture::find_window(hwnd) {
            Some(target) => Some(target),
            None => return api_error(StatusCode::NOT_FOUND, "window handle is not available"),
        },
        None => None,
    };
    let mut selected = match state.selected_target.lock() {
        Ok(selected) => selected,
        Err(_) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "capture target unavailable",
            )
        }
    };
    *selected = target.clone();
    Json(serde_json::json!({ "ok": true, "capture_target": target })).into_response()
}

async fn screenshot(State(state): State<AppState>) -> Response {
    let Some(ffmpeg) = state.ffmpeg.clone() else {
        return api_error(StatusCode::SERVICE_UNAVAILABLE, "FFmpeg not found");
    };
    let dir = match output_dir(&state) {
        Ok(dir) => dir,
        Err(error) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    let target = match capture_target(&state) {
        Ok(target) => target,
        Err(error) => return api_error(StatusCode::CONFLICT, &error),
    };
    let path = dir.join(format!("screenshot-{}.png", timestamp()));
    match capture_screenshot(&ffmpeg, &path, target) {
        Ok(()) => Json(FileResponse {
            ok: true,
            file: path.display().to_string(),
        })
        .into_response(),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

async fn start(State(state): State<AppState>, Json(request): Json<StartRequest>) -> Response {
    if !(10..=60).contains(&request.fps) {
        return api_error(StatusCode::BAD_REQUEST, "fps must be between 10 and 60");
    }
    let Some(ffmpeg) = state.ffmpeg.clone() else {
        return api_error(StatusCode::SERVICE_UNAVAILABLE, "FFmpeg not found");
    };
    let dir = match output_dir(&state) {
        Ok(dir) => dir,
        Err(error) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    let mut guard = match state.recording.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "recording state unavailable",
            )
        }
    };
    if guard.is_some() {
        return api_error(StatusCode::CONFLICT, "recording is already active");
    }
    let target = match capture_target(&state) {
        Ok(target) => target,
        Err(error) => return api_error(StatusCode::CONFLICT, &error),
    };
    let path = dir.join(format!("recording-{}.mp4", timestamp()));
    match start_recording(&ffmpeg, &path, request.fps, target) {
        Ok(child) => {
            *guard = Some(RecordingProcess {
                child,
                path: path.clone(),
                started: Instant::now(),
            });
            Json(FileResponse {
                ok: true,
                file: path.display().to_string(),
            })
            .into_response()
        }
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

fn capture_target(state: &AppState) -> Result<Option<WindowTarget>, String> {
    let selected = state
        .selected_target
        .lock()
        .map_err(|_| "capture target unavailable".to_owned())?
        .clone();
    match selected {
        Some(target) => windows_capture::find_window(target.hwnd)
            .filter(|active| active.process_id == target.process_id && active.title == target.title)
            .map(Some)
            .ok_or_else(|| {
                "selected app window is no longer available; refresh and select it again".into()
            }),
        None => Ok(None),
    }
}

async fn stop(State(state): State<AppState>) -> Response {
    match stop_capture(&state) {
        Ok(path) => Json(FileResponse {
            ok: true,
            file: path.display().to_string(),
        })
        .into_response(),
        Err(error) => api_error(StatusCode::CONFLICT, &error),
    }
}

pub fn stop_capture(state: &AppState) -> Result<PathBuf, String> {
    let mut recording = state
        .recording
        .lock()
        .map_err(|_| "recording state unavailable".to_owned())?;
    let mut active = recording
        .take()
        .ok_or_else(|| "recording is not active".to_owned())?;
    if let Some(mut stdin) = active.child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(b"q\n");
    }
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match active.child.try_wait() {
            Ok(Some(status)) if status.success() && active.path.exists() => return Ok(active.path),
            Ok(Some(status)) => return Err(format!("FFmpeg ended with {status}")),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                let _ = active.child.kill();
                let _ = active.child.wait();
                return Err("FFmpeg did not stop cleanly within 8 seconds".into());
            }
            Err(error) => return Err(format!("could not stop FFmpeg: {error}")),
        }
    }
}

fn clear_finished_recording(state: &AppState) {
    if let Ok(mut guard) = state.recording.lock() {
        if guard
            .as_mut()
            .and_then(|r| r.child.try_wait().ok().flatten())
            .is_some()
        {
            guard.take();
        }
    }
}

fn output_dir(state: &AppState) -> Result<PathBuf, String> {
    let dir = state
        .output_dir
        .lock()
        .map_err(|_| "output directory unavailable".to_owned())?
        .clone();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn api_error(status: StatusCode, error: &str) -> Response {
    (
        status,
        Json(ErrorResponse {
            ok: false,
            error: error.to_owned(),
        }),
    )
        .into_response()
}

fn timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}

pub fn run_mcp_stdio() {
    let token = match crate::load_or_create_token() {
        Ok(token) => token,
        Err(error) => {
            eprintln!("RecordScreen MCP: cannot load token: {error}");
            return;
        }
    };
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            eprintln!("RecordScreen MCP: HTTP client error: {error}");
            return;
        }
    };
    let stdin = std::io::stdin();
    let mut stdout = std::io::BufWriter::new(std::io::stdout().lock());
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            continue;
        };
        let id = message.get("id").cloned();
        if method.starts_with("notifications/") {
            continue;
        }
        let result = match method {
            "initialize" => {
                let requested_version = message
                    .pointer("/params/protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("2024-11-05");
                let version = match requested_version {
                    "2024-11-05" | "2025-03-26" | "2025-06-18" | "2025-11-25" => requested_version,
                    _ => "2024-11-05",
                };
                Some(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "recordscreen", "version": env!("CARGO_PKG_VERSION") }
                }))
            }
            "ping" => Some(json!({})),
            "tools/list" => Some(json!({ "tools": mcp_tools() })),
            "tools/call" => Some(call_mcp_tool(&client, &token, &message["params"])),
            _ => {
                if let Some(id) = id.clone() {
                    let response = json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": { "code": -32601, "message": format!("Unknown method: {method}") }
                    });
                    let _ = serde_json::to_writer(&mut stdout, &response);
                    let _ = stdout.write_all(b"\n");
                    let _ = stdout.flush();
                }
                None
            }
        };
        if let (Some(id), Some(result)) = (id, result) {
            let response = json!({ "jsonrpc": "2.0", "id": id, "result": result });
            if serde_json::to_writer(&mut stdout, &response).is_ok() {
                let _ = stdout.write_all(b"\n");
                let _ = stdout.flush();
            }
        }
    }
}

fn mcp_tools() -> Value {
    json!([
        { "name": "get_status", "description": "Read the local screen recorder status and output directory.", "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false } },
        { "name": "list_windows", "description": "List visible app windows that can be selected for capture.", "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false } },
        { "name": "select_window", "description": "Lock capture to a window handle returned by list_windows; pass null to capture the whole desktop.", "inputSchema": { "type": "object", "properties": { "hwnd": { "type": ["integer", "null"] } }, "required": ["hwnd"], "additionalProperties": false } },
        { "name": "take_screenshot", "description": "Capture the selected app window, or the whole Windows desktop, to a PNG file.", "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false } },
        { "name": "start_recording", "description": "Start recording the selected app window, or whole desktop, to an MP4 file.", "inputSchema": { "type": "object", "properties": { "fps": { "type": "integer", "minimum": 10, "maximum": 60, "default": 30 } }, "additionalProperties": false } },
        { "name": "stop_recording", "description": "Stop desktop recording and finalize the MP4 file.", "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false } }
    ])
}

fn call_mcp_tool(client: &reqwest::blocking::Client, token: &str, params: &Value) -> Value {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return mcp_error("tool name is required");
    };
    let (method, path, body) = match name {
        "get_status" => ("GET", "/api/v1/status", None),
        "list_windows" => ("GET", "/api/v1/windows", None),
        "select_window" => {
            let hwnd = params
                .pointer("/arguments/hwnd")
                .cloned()
                .unwrap_or(Value::Null);
            if !hwnd.is_null() && !hwnd.is_u64() {
                return mcp_error("hwnd must be an integer returned by list_windows, or null");
            }
            ("POST", "/api/v1/target", Some(json!({ "hwnd": hwnd })))
        }
        "take_screenshot" => ("POST", "/api/v1/screenshot", None),
        "start_recording" => {
            let fps = params
                .pointer("/arguments/fps")
                .and_then(Value::as_u64)
                .unwrap_or(30);
            if !(10..=60).contains(&fps) {
                return mcp_error("fps must be between 10 and 60");
            }
            (
                "POST",
                "/api/v1/recording/start",
                Some(json!({ "fps": fps })),
            )
        }
        "stop_recording" => ("POST", "/api/v1/recording/stop", None),
        _ => return mcp_error(&format!("unknown tool: {name}")),
    };
    let url = format!("http://127.0.0.1:17321{path}");
    let request = match method {
        "GET" => client.get(url),
        _ => client.post(url),
    }
    .bearer_auth(token);
    let request = if let Some(body) = body {
        request.json(&body)
    } else {
        request
    };
    match request.send() {
        Ok(response) => {
            let is_error = !response.status().is_success();
            let body = response
                .text()
                .unwrap_or_else(|e| format!("HTTP read error: {e}"));
            json!({ "content": [{ "type": "text", "text": body }], "isError": is_error })
        }
        Err(error) => json!({
            "content": [{ "type": "text", "text": format!("RecordScreen is unavailable at 127.0.0.1:17321. Start the desktop app first. ({error})") }],
            "isError": true
        }),
    }
}

fn mcp_error(error: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": error }], "isError": true })
}
