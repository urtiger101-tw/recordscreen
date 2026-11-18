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
use serde_json::{json, Map, Value};
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
    pub finished_recordings: Arc<Mutex<Vec<(PathBuf, bool)>>>,
    pub selected_target: Arc<Mutex<Option<WindowTarget>>>,
    pub token: Arc<String>,
    pub api_status: Arc<Mutex<ApiStatus>>,
}

#[derive(Clone, Debug)]
pub enum ApiStatus {
    Starting,
    Listening,
    Failed(String),
}

impl AppState {
    pub fn set_api_status(&self, status: ApiStatus) {
        if let Ok(mut current) = self.api_status.lock() {
            *current = status;
        }
    }
}

pub struct RecordingProcess {
    pub child: Child,
    pub path: PathBuf,
    pub started: Instant,
}

#[derive(Deserialize)]
// An untagged struct variant accepts JSON objects and preserves field validation.
#[serde(
    untagged,
    deny_unknown_fields,
    expecting = "a JSON object with an optional integer fps"
)]
enum StartRequest {
    Object {
        #[serde(default = "default_fps")]
        fps: u32,
    },
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
#[serde(
    untagged,
    expecting = "a JSON object with one hwnd field containing a nonnegative integer or null"
)]
enum SelectTargetRequest {
    Object {
        #[serde(deserialize_with = "Deserialize::deserialize")]
        hwnd: Option<usize>,
    },
}

#[derive(serde::Serialize)]
struct WindowsResponse {
    windows: Vec<WindowTarget>,
}

pub async fn serve(state: AppState) {
    serve_at(state, "127.0.0.1:17321").await;
}

async fn serve_at(state: AppState, address: &str) {
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

    let listener = match tokio::net::TcpListener::bind(address).await {
        Ok(listener) => listener,
        Err(error) => {
            state.set_api_status(ApiStatus::Failed(format!(
                "Cannot listen on {address}: {error}"
            )));
            return;
        }
    };
    state.set_api_status(ApiStatus::Listening);
    if let Err(error) = axum::serve(listener, app).await {
        state.set_api_status(ApiStatus::Failed(format!("Agent API stopped: {error}")));
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
    Json(SelectTargetRequest::Object { hwnd }): Json<SelectTargetRequest>,
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
    let target = match hwnd {
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

async fn start(
    State(state): State<AppState>,
    Json(StartRequest::Object { fps }): Json<StartRequest>,
) -> Response {
    if !(10..=60).contains(&fps) {
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
    match start_recording(&ffmpeg, &path, fps, target) {
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
            .filter(|active| target.is_same_window(active))
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

pub fn clear_finished_recording(state: &AppState) {
    let Ok(mut guard) = state.recording.lock() else {
        return;
    };
    let Some(status) = guard
        .as_mut()
        .and_then(|r| r.child.try_wait().ok().flatten())
    else {
        return;
    };
    if let Some(recording) = guard.take() {
        if let Ok(mut finished) = state.finished_recordings.lock() {
            finished.push((recording.path, status.success()));
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
    let stdout = std::io::BufWriter::new(std::io::stdout().lock());
    serve_mcp_stdio(&client, &token, stdin.lock(), stdout);
}

fn serve_mcp_stdio(
    client: &reqwest::blocking::Client,
    token: &str,
    input: impl BufRead,
    mut output: impl Write,
) {
    let mut structured_results = false;
    for line in input.lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            continue;
        };
        let Some(id) = message
            .get("id")
            .filter(|id| id.is_string() || id.is_number())
        else {
            continue;
        };
        if message["jsonrpc"] != "2.0" || method.starts_with("notifications/") {
            continue;
        }
        let mut response = match method {
            "initialize" => {
                let requested_version = message
                    .pointer("/params/protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("2024-11-05");
                let version = match requested_version {
                    "2024-11-05" | "2025-03-26" | "2025-06-18" | "2025-11-25" => requested_version,
                    _ => "2024-11-05",
                };
                structured_results = matches!(version, "2025-06-18" | "2025-11-25");
                json!({ "result": {
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "recordscreen", "version": env!("CARGO_PKG_VERSION") }
                } })
            }
            "ping" => json!({ "result": {} }),
            "tools/list" => json!({ "result": { "tools": mcp_tools() } }),
            "tools/call" => match mcp_tool_request(client, &message["params"]) {
                Ok(request) => {
                    json!({ "result": call_mcp_tool(request, token, structured_results) })
                }
                Err(McpCallError::InvalidRequest(error)) => {
                    json!({ "error": { "code": -32602, "message": error } })
                }
                Err(McpCallError::InvalidArguments(error)) => {
                    json!({ "result": mcp_error(&error) })
                }
            },
            _ => {
                json!({ "error": { "code": -32601, "message": format!("Unknown method: {method}") } })
            }
        };
        response["jsonrpc"] = json!("2.0");
        response["id"] = id.clone();
        if serde_json::to_writer(&mut output, &response).is_err()
            || output.write_all(b"\n").is_err()
            || output.flush().is_err()
        {
            break;
        }
    }
}

fn mcp_tools() -> Value {
    json!([
        { "name": "get_status", "description": "Read the local screen recorder status and output directory.", "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false } },
        { "name": "list_windows", "description": "List visible app windows that can be selected for capture.", "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false } },
        { "name": "select_window", "description": "Lock capture to a window handle returned by list_windows; pass null to capture the whole desktop.", "inputSchema": { "type": "object", "properties": { "hwnd": { "type": ["integer", "null"], "minimum": 0 } }, "required": ["hwnd"], "additionalProperties": false } },
        { "name": "take_screenshot", "description": "Capture the selected app window, or the whole Windows desktop, to a PNG file.", "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false } },
        { "name": "start_recording", "description": "Start recording the selected app window, or whole desktop, to an MP4 file.", "inputSchema": { "type": "object", "properties": { "fps": { "type": "integer", "minimum": 10, "maximum": 60, "default": 30 } }, "additionalProperties": false } },
        { "name": "stop_recording", "description": "Stop desktop recording and finalize the MP4 file.", "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false } }
    ])
}

#[derive(Debug)]
enum McpCallError {
    InvalidRequest(String),
    InvalidArguments(String),
}

fn mcp_tool_request(
    client: &reqwest::blocking::Client,
    params: &Value,
) -> Result<reqwest::blocking::RequestBuilder, McpCallError> {
    use McpCallError::{InvalidArguments, InvalidRequest};

    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Err(InvalidRequest("tool name is required".into()));
    };
    let empty = Map::new();
    let arguments = match params.get("arguments") {
        None => &empty,
        Some(Value::Object(arguments)) => arguments,
        Some(_) => return Err(InvalidRequest("arguments must be an object".into())),
    };
    let (method, path, body, allowed_argument) = match name {
        "get_status" => ("GET", "/api/v1/status", None, None),
        "list_windows" => ("GET", "/api/v1/windows", None, None),
        "select_window" => {
            let SelectTargetRequest::Object { hwnd } =
                serde_json::from_value(Value::Object(arguments.clone())).map_err(|_| {
                    InvalidArguments("hwnd is required and must be a nonnegative integer returned by list_windows, or explicit null for the entire desktop".into())
                })?;
            (
                "POST",
                "/api/v1/target",
                Some(json!({ "hwnd": hwnd })),
                Some("hwnd"),
            )
        }
        "take_screenshot" => ("POST", "/api/v1/screenshot", None, None),
        "start_recording" => {
            let StartRequest::Object { fps } = serde_json::from_value(Value::Object(arguments.clone()))
                .map_err(|error| InvalidArguments(format!("Invalid start_recording arguments. fps must be an integer between 10 and 60: {error}")))?;
            if !(10..=60).contains(&fps) {
                return Err(InvalidArguments(
                    "fps must be an integer between 10 and 60".into(),
                ));
            }
            (
                "POST",
                "/api/v1/recording/start",
                Some(json!({ "fps": fps })),
                Some("fps"),
            )
        }
        "stop_recording" => ("POST", "/api/v1/recording/stop", None, None),
        _ => return Err(InvalidRequest(format!("unknown tool: {name}"))),
    };
    if let Some(key) = arguments
        .keys()
        .find(|key| Some(key.as_str()) != allowed_argument)
    {
        return Err(InvalidArguments(format!(
            "unexpected argument for {name}: {key}"
        )));
    }
    let url = format!("http://127.0.0.1:17321{path}");
    let request = match method {
        "GET" => client.get(url),
        _ => client.post(url),
    };
    Ok(if let Some(body) = body {
        request.json(&body)
    } else {
        request
    })
}

fn call_mcp_tool(
    request: reqwest::blocking::RequestBuilder,
    token: &str,
    structured_results: bool,
) -> Value {
    let response = match request.bearer_auth(token).send() {
        Ok(response) => response,
        Err(error) => {
            return mcp_error(&format!(
                "RecordScreen request failed. Verify the desktop app and check its status before retrying. ({error})"
            ))
        }
    };
    let status = response.status();
    match response.text() {
        Ok(body) => mcp_http_result(status, &body, structured_results),
        Err(error) => mcp_error(&format!(
            "Could not read the RecordScreen HTTP response: {error}"
        )),
    }
}

fn mcp_http_result(status: StatusCode, body: &str, structured_results: bool) -> Value {
    if !status.is_success() {
        return mcp_error(body);
    }
    let Ok(data @ Value::Object(_)) = serde_json::from_str::<Value>(body) else {
        return mcp_error("RecordScreen returned a successful HTTP status without a JSON object.");
    };
    let mut result = json!({ "content": [{ "type": "text", "text": body }], "isError": false });
    if structured_results {
        result["structuredContent"] = data;
    }
    result
}

fn mcp_error(error: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": error }], "isError": true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_capture_arguments_require_objects_with_unambiguous_fields() {
        for invalid in [
            "{}",
            "[null]",
            r#"{"hwnd":-1}"#,
            r#"{"hwnd":"42"}"#,
            r#"{"hwnd":42,"hwnd":null}"#,
        ] {
            assert!(serde_json::from_str::<SelectTargetRequest>(invalid).is_err());
        }
        for (valid, expected) in [(r#"{"hwnd":null}"#, None), (r#"{"hwnd":42}"#, Some(42))] {
            let SelectTargetRequest::Object { hwnd } = serde_json::from_str(valid).unwrap();
            assert_eq!(hwnd, expected);
        }

        for invalid in [
            "[]",
            "[30]",
            r#"{"fps":null}"#,
            r#"{"fps":30,"fps":60}"#,
            r#"{"fpx":30}"#,
        ] {
            assert!(serde_json::from_str::<StartRequest>(invalid).is_err());
        }
        for (valid, expected) in [("{}", 30), (r#"{"fps":60}"#, 60)] {
            let StartRequest::Object { fps } = serde_json::from_str(valid).unwrap();
            assert_eq!(fps, expected);
        }
    }

    #[test]
    fn mcp_only_builds_requests_for_schema_valid_arguments() {
        let client = reqwest::blocking::Client::new();
        for invalid in [
            json!({ "name": "select_window" }),
            json!({ "name": "select_window", "arguments": {} }),
            json!({ "name": "select_window", "arguments": { "hwnd": -1 } }),
            json!({ "name": "select_window", "arguments": { "hwnd": null, "hwd": 42 } }),
            json!({ "name": "start_recording", "arguments": null }),
            json!({ "name": "start_recording", "arguments": { "fps": null } }),
            json!({ "name": "start_recording", "arguments": { "fps": "30" } }),
            json!({ "name": "start_recording", "arguments": { "fps": -5 } }),
            json!({ "name": "start_recording", "arguments": { "fps": 29.5 } }),
            json!({ "name": "start_recording", "arguments": { "fps": 61 } }),
            json!({ "name": "start_recording", "arguments": { "fpx": 30 } }),
            json!({ "name": "take_screenshot", "arguments": { "hwnd": 42 } }),
        ] {
            assert!(mcp_tool_request(&client, &invalid).is_err(), "{invalid}");
        }
        for (valid, path, expected) in [
            (
                json!({ "name": "select_window", "arguments": { "hwnd": null } }),
                "/api/v1/target",
                json!({ "hwnd": null }),
            ),
            (
                json!({ "name": "select_window", "arguments": { "hwnd": 42 } }),
                "/api/v1/target",
                json!({ "hwnd": 42 }),
            ),
            (
                json!({ "name": "start_recording" }),
                "/api/v1/recording/start",
                json!({ "fps": 30 }),
            ),
            (
                json!({ "name": "start_recording", "arguments": { "fps": 60 } }),
                "/api/v1/recording/start",
                json!({ "fps": 60 }),
            ),
        ] {
            let request = mcp_tool_request(&client, &valid).unwrap().build().unwrap();
            assert_eq!(request.url().path(), path);
            assert_eq!(request.method(), "POST");
            let body = request.body().unwrap().as_bytes().unwrap();
            assert_eq!(serde_json::from_slice::<Value>(body).unwrap(), expected);
        }
    }

    #[test]
    fn mcp_stdio_lists_tools_and_reports_invalid_calls() {
        let input = br#"{"jsonrpc":"2.0","id":"init","method":"initialize","params":{"protocolVersion":"2025-06-18"}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","method":"ping"}
{"jsonrpc":"2.0","id":1,"method":"tools/list"}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"select_window","arguments":{}}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"invalid_tool"}}
"#;
        let mut output = Vec::new();
        serve_mcp_stdio(
            &reqwest::blocking::Client::new(),
            "test-only-token",
            &input[..],
            &mut output,
        );
        let responses: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(responses.len(), 4);
        assert_eq!(responses[0]["id"], "init");
        assert_eq!(responses[0]["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(responses[1]["result"]["tools"].as_array().unwrap().len(), 6);
        assert_eq!(responses[2]["id"], 2);
        assert_eq!(responses[2]["result"]["isError"], true);
        assert!(responses[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("hwnd is required"));
        assert_eq!(responses[3]["error"]["code"], -32602);
        assert!(responses[3]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown tool"));
    }

    #[test]
    fn mcp_response_keeps_json_text_and_adds_structured_results_when_negotiated() {
        let body = r#"{"recording":false,"capture_target":null}"#;
        let structured = mcp_http_result(StatusCode::OK, body, true);
        let legacy = mcp_http_result(StatusCode::OK, body, false);
        for result in [&structured, &legacy] {
            assert_eq!(result["content"][0]["text"], body);
            assert_eq!(result["isError"], false);
        }
        assert_eq!(
            structured["structuredContent"],
            serde_json::from_str::<Value>(body).unwrap()
        );
        assert!(legacy.get("structuredContent").is_none());
        for (status, body) in [
            (StatusCode::OK, "not json"),
            (StatusCode::UNAUTHORIZED, "missing or invalid bearer token"),
        ] {
            let result = mcp_http_result(status, body, true);
            assert_eq!(result["isError"], true);
            assert!(result.get("structuredContent").is_none());
        }
    }

    #[tokio::test]
    async fn occupied_port_is_reported_in_api_status() {
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = occupied.local_addr().unwrap().to_string();
        let state = AppState {
            ffmpeg: None,
            output_dir: Arc::new(Mutex::new(PathBuf::new())),
            recording: Arc::new(Mutex::new(None)),
            finished_recordings: Arc::new(Mutex::new(Vec::new())),
            selected_target: Arc::new(Mutex::new(None)),
            token: Arc::new("test-only-token".into()),
            api_status: Arc::new(Mutex::new(ApiStatus::Starting)),
        };
        serve_at(state.clone(), &address).await;
        let result = state.api_status.lock().unwrap().clone();
        assert!(matches!(result, ApiStatus::Failed(message) if message.contains(&address)));
    }
}
