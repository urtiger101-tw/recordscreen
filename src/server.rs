use crate::windows_capture::WindowTarget;
use crate::{
    current_status, screenshot_command, screenshot_result, start_recording, windows_capture, Status,
};
use axum::{
    body::Body,
    extract::State,
    http::{header, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use serde::Deserialize;
use serde_json::{json, value::RawValue, Map, Value};
use std::{
    io::{BufRead, Write},
    path::PathBuf,
    process::Child,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{io::AsyncReadExt, net::TcpListener, sync::watch};

const LISTEN_ADDRESS: &str = "127.0.0.1:17321";
const API_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const API_RUNTIME_TIMEOUT: Duration = Duration::from_secs(1);

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
    deny_unknown_fields,
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

pub struct ApiServer {
    shutdown: watch::Sender<bool>,
    thread: JoinHandle<()>,
}

impl ApiServer {
    pub fn shutdown(self) -> thread::Result<()> {
        self.shutdown.send_replace(true);
        self.thread.join()
    }
}

pub fn spawn_api(state: AppState) -> std::io::Result<ApiServer> {
    let (shutdown, receiver) = watch::channel(false);
    let thread = thread::Builder::new()
        .name("agent-control-api".into())
        .spawn(move || {
            match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => {
                    runtime.block_on(serve_at(state, LISTEN_ADDRESS, receiver));
                    runtime.shutdown_timeout(API_RUNTIME_TIMEOUT);
                }
                Err(error) => state.set_api_status(ApiStatus::Failed(format!(
                    "Cannot start Agent API runtime: {error}"
                ))),
            }
        })?;
    Ok(ApiServer { shutdown, thread })
}

async fn serve_at(state: AppState, address: &str, shutdown: watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    let listener = match TcpListener::bind(address).await {
        Ok(listener) => listener,
        Err(error) => {
            state.set_api_status(ApiStatus::Failed(format!(
                "Cannot listen on {address}: {error}"
            )));
            return;
        }
    };
    serve_listener(state, listener, shutdown).await;
}

async fn serve_listener(state: AppState, listener: TcpListener, shutdown: watch::Receiver<bool>) {
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
        .layer(middleware::from_fn(check_local_request))
        .layer(Extension(shutdown.clone()));

    state.set_api_status(ApiStatus::Listening);
    let mut graceful = shutdown.clone();
    let mut deadline = shutdown;
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        let _ = graceful.wait_for(|requested| *requested).await;
    });
    tokio::select! {
        result = server => {
            if let Err(error) = result {
                state.set_api_status(ApiStatus::Failed(format!("Agent API stopped: {error}")));
            }
        }
        () = async move {
            let _ = deadline.wait_for(|requested| *requested).await;
            tokio::time::sleep(API_DRAIN_TIMEOUT).await;
        } => {}
    }
}

async fn check_local_request(
    Extension(shutdown): Extension<watch::Receiver<bool>>,
    req: Request<Body>,
    next: Next,
) -> Response {
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
    if *shutdown.borrow() {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "RecordScreen is shutting down",
        );
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

async fn screenshot(
    State(state): State<AppState>,
    Extension(shutdown): Extension<watch::Receiver<bool>>,
) -> Response {
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
    if *shutdown.borrow() {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "RecordScreen is shutting down",
        );
    }
    let mut child = match tokio::process::Command::from(screenshot_command(&ffmpeg, &path, target))
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("啟動 FFmpeg 失敗：{error}"),
            )
        }
    };
    let result = match finish_screenshot(&mut child, shutdown).await {
        Ok(Some((status, stderr))) => screenshot_result(&path, status, &stderr),
        Ok(None) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "RecordScreen is shutting down",
            )
        }
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => Json(FileResponse {
            ok: true,
            file: path.display().to_string(),
        })
        .into_response(),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

async fn finish_screenshot(
    child: &mut tokio::process::Child,
    mut shutdown: watch::Receiver<bool>,
) -> Result<Option<(std::process::ExitStatus, Vec<u8>)>, String> {
    let mut stderr = child.stderr.take();
    let mut diagnostic = Vec::new();
    let result = tokio::select! {
        biased;
        _ = shutdown.wait_for(|requested| *requested) => None,
        result = async {
            let collect_stderr = async {
                if let Some(stderr) = stderr.as_mut() {
                    stderr.read_to_end(&mut diagnostic).await?;
                }
                Ok(())
            };
            tokio::try_join!(child.wait(), collect_stderr)
        } => Some(result),
    };
    match result {
        Some(Ok((status, ()))) => Ok(Some((status, diagnostic))),
        Some(Err(error)) => {
            child.kill().await.map_err(|kill_error| format!("Could not read FFmpeg result: {error}. Could not terminate FFmpeg: {kill_error}"))?;
            Err(format!("Could not read FFmpeg result: {error}"))
        }
        None => {
            child
                .kill()
                .await
                .map_err(|error| format!("Could not terminate screenshot FFmpeg: {error}"))?;
            Ok(None)
        }
    }
}

async fn start(
    State(state): State<AppState>,
    Extension(shutdown): Extension<watch::Receiver<bool>>,
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
    // Requests already parsing a body can outlast listener shutdown.
    // Check under the same lock used to finish any recording on exit.
    if *shutdown.borrow() {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "RecordScreen is shutting down",
        );
    }
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
    finish_recording(state)?.ok_or_else(|| "recording is not active".to_owned())
}

pub fn finish_recording(state: &AppState) -> Result<Option<PathBuf>, String> {
    let mut recording = state
        .recording
        .lock()
        .map_err(|_| "recording state unavailable".to_owned())?;
    let Some(active) = recording.as_mut() else {
        return Ok(None);
    };
    if let Some(mut stdin) = active.child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(b"q\n");
    }
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let failure = match active.child.try_wait() {
            Ok(Some(status)) => {
                let path = active.path.clone();
                *recording = None;
                return if status.success() && path.exists() {
                    Ok(Some(path))
                } else {
                    Err(format!("FFmpeg ended with {status}"))
                };
            }
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(100));
                continue;
            }
            Ok(None) => "FFmpeg did not stop cleanly within 8 seconds".into(),
            Err(error) => format!("could not check whether FFmpeg stopped: {error}"),
        };
        if let Err(error) = active.child.kill() {
            if !matches!(active.child.try_wait(), Ok(Some(_))) {
                return Err(format!("{failure}. Could not terminate FFmpeg: {error}"));
            }
        }
        active
            .child
            .wait()
            .map_err(|error| format!("{failure}. Could not wait for FFmpeg: {error}"))?;
        *recording = None;
        return Err(failure);
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
        if !line.trim_start().starts_with('{') {
            continue;
        }
        let Ok(message) = serde_json::from_str::<McpMessage<'_>>(&line) else {
            continue;
        };
        let Some(id) = message.id.filter(|id| id.is_string() || id.is_number()) else {
            continue;
        };
        let method = message.method.as_str();
        if message.jsonrpc != "2.0" || method.starts_with("notifications/") {
            continue;
        }
        let params = message.params.map_or("null", RawValue::get);
        let mut response = match method {
            "initialize" => {
                let params = serde_json::from_str::<Value>(params).unwrap_or(Value::Null);
                let requested_version = params
                    .get("protocolVersion")
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
            "tools/call" => match mcp_tool_request(client, params) {
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
        response["id"] = id;
        if serde_json::to_writer(&mut output, &response).is_err()
            || output.write_all(b"\n").is_err()
            || output.flush().is_err()
        {
            break;
        }
    }
}

#[derive(Deserialize)]
struct McpMessage<'a> {
    jsonrpc: String,
    method: String,
    id: Option<Value>,
    #[serde(borrow)]
    params: Option<&'a RawValue>,
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

#[derive(Deserialize)]
struct McpCallParams<'a> {
    name: String,
    #[serde(borrow, default = "empty_arguments")]
    arguments: &'a RawValue,
}

fn empty_arguments() -> &'static RawValue {
    serde_json::from_str("{}").expect("empty JSON object")
}

fn mcp_tool_request(
    client: &reqwest::blocking::Client,
    params: &str,
) -> Result<reqwest::blocking::RequestBuilder, McpCallError> {
    use McpCallError::{InvalidArguments, InvalidRequest};

    if !params.starts_with('{') {
        return Err(InvalidRequest(
            "tool call parameters must be an object".into(),
        ));
    }
    let params: McpCallParams = serde_json::from_str(params)
        .map_err(|error| InvalidRequest(format!("Invalid tool call parameters: {error}")))?;
    let arguments = params.arguments.get();
    if !arguments.starts_with('{') {
        return Err(InvalidRequest("arguments must be an object".into()));
    }
    let name = params.name.as_str();
    let (method, path, body) = match name {
        "get_status" => ("GET", "/api/v1/status", None),
        "list_windows" => ("GET", "/api/v1/windows", None),
        "select_window" => {
            let SelectTargetRequest::Object { hwnd } =
                serde_json::from_str(arguments).map_err(|_| {
                    InvalidArguments("hwnd is required exactly once and must be a nonnegative integer returned by list_windows, or explicit null for the entire desktop".into())
                })?;
            ("POST", "/api/v1/target", Some(json!({ "hwnd": hwnd })))
        }
        "take_screenshot" => ("POST", "/api/v1/screenshot", None),
        "start_recording" => {
            let StartRequest::Object { fps } = serde_json::from_str(arguments).map_err(|_| {
                InvalidArguments("Pass fps at most once as an integer between 10 and 60, with no other arguments".into())
            })?;
            if !(10..=60).contains(&fps) {
                return Err(InvalidArguments(
                    "fps must be an integer between 10 and 60".into(),
                ));
            }
            (
                "POST",
                "/api/v1/recording/start",
                Some(json!({ "fps": fps })),
            )
        }
        "stop_recording" => ("POST", "/api/v1/recording/stop", None),
        _ => return Err(InvalidRequest(format!("unknown tool: {name}"))),
    };
    if body.is_none()
        && !serde_json::from_str::<Map<String, Value>>(arguments)
            .is_ok_and(|arguments| arguments.is_empty())
    {
        return Err(InvalidArguments(format!("{name} accepts no arguments")));
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

    fn test_state(output_dir: PathBuf) -> AppState {
        AppState {
            ffmpeg: None,
            output_dir: Arc::new(Mutex::new(output_dir)),
            recording: Arc::new(Mutex::new(None)),
            finished_recordings: Arc::new(Mutex::new(Vec::new())),
            selected_target: Arc::new(Mutex::new(None)),
            token: Arc::new("test-only-token".into()),
            api_status: Arc::new(Mutex::new(ApiStatus::Starting)),
        }
    }

    #[tokio::test]
    async fn in_flight_start_is_rejected_after_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state(dir.path().to_owned());
        state.ffmpeg = Some(dir.path().join("unavailable-ffmpeg"));
        let (shutdown, receiver) = watch::channel(false);
        shutdown.send_replace(true);
        let response = start(
            State(state.clone()),
            Extension(receiver),
            Json(StartRequest::Object { fps: 30 }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(state.recording.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn listener_stops_with_a_slow_request_body() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let state = test_state(PathBuf::new());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown, receiver) = watch::channel(false);
        let server = tokio::spawn(serve_listener(state, listener, receiver));
        let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
        client
            .write_all(b"POST /api/v1/recording/start HTTP/1.1\r\nHost: 127.0.0.1:17321\r\nAuthorization: Bearer test-only-token\r\nContent-Type: application/json\r\nContent-Length: 10\r\nExpect: 100-continue\r\n\r\n")
            .await
            .unwrap();
        let mut response = [0; 25];
        tokio::time::timeout(Duration::from_secs(2), client.read_exact(&mut response))
            .await
            .expect("server must begin reading the request body")
            .unwrap();
        assert_eq!(&response, b"HTTP/1.1 100 Continue\r\n\r\n");
        shutdown.send_replace(true);
        tokio::time::timeout(API_DRAIN_TIMEOUT + Duration::from_secs(2), server)
            .await
            .expect("slow request must not indefinitely prolong shutdown")
            .unwrap();
        assert!(tokio::net::TcpStream::connect(address).await.is_err());
    }

    #[test]
    fn finish_recording_sends_q_and_reaps_child() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fake-recording.mp4");
        let state = test_state(dir.path().to_owned());
        *state.recording.lock().unwrap() = Some(RecordingProcess {
            child: spawn_recording_child(&path, "finalize"),
            path: path.clone(),
            started: Instant::now(),
        });
        assert_eq!(finish_recording(&state).unwrap(), Some(path.clone()));
        assert_eq!(std::fs::read(path).unwrap(), b"finalized");
        assert_eq!(finish_recording(&state).unwrap(), None);
        assert_eq!(stop_capture(&state).unwrap_err(), "recording is not active");
    }

    #[test]
    fn finish_recording_terminates_unresponsive_child_after_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fake-recording.mp4");
        let state = test_state(dir.path().to_owned());
        *state.recording.lock().unwrap() = Some(RecordingProcess {
            child: spawn_recording_child(&path, "hang"),
            path,
            started: Instant::now(),
        });

        assert_eq!(
            finish_recording(&state).unwrap_err(),
            "FFmpeg did not stop cleanly within 8 seconds"
        );
        assert_eq!(finish_recording(&state).unwrap(), None);
    }

    fn spawn_recording_child(path: &std::path::Path, behavior: &str) -> Child {
        use std::process::{Command, Stdio};

        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "server::tests::fake_recording_child",
                "--nocapture",
            ])
            .env("RECORDSCREEN_TEST_CHILD_OUTPUT", path)
            .env("RECORDSCREEN_TEST_CHILD_BEHAVIOR", behavior)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    #[test]
    fn fake_recording_child() {
        let Some(path) = std::env::var_os("RECORDSCREEN_TEST_CHILD_OUTPUT") else {
            return;
        };
        match std::env::var("RECORDSCREEN_TEST_CHILD_BEHAVIOR").as_deref() {
            Ok("screenshot-complete") => {
                std::fs::write(path, b"fake screenshot").unwrap();
                eprintln!("fake screenshot diagnostic");
                return;
            }
            Ok("screenshot-wait") => {
                std::fs::write(path, b"fake screenshot started").unwrap();
                loop {
                    thread::park();
                }
            }
            _ => {}
        }
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        assert_eq!(line, "q\n");
        if std::env::var("RECORDSCREEN_TEST_CHILD_BEHAVIOR").as_deref() == Ok("hang") {
            loop {
                thread::park();
            }
        }
        std::fs::write(path, b"finalized").unwrap();
    }

    #[tokio::test]
    async fn screenshot_child_is_reaped_after_completion_or_api_shutdown() {
        use std::process::Stdio;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fake-screenshot.png");
        let spawn = |behavior| {
            tokio::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "server::tests::fake_recording_child",
                    "--nocapture",
                ])
                .env("RECORDSCREEN_TEST_CHILD_OUTPUT", &path)
                .env("RECORDSCREEN_TEST_CHILD_BEHAVIOR", behavior)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap()
        };
        let (shutdown, receiver) = watch::channel(false);
        let mut child = spawn("screenshot-complete");
        let (status, diagnostic) = tokio::time::timeout(
            Duration::from_secs(10),
            finish_screenshot(&mut child, receiver.clone()),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert!(status.success());
        assert_eq!(child.try_wait().unwrap(), Some(status));
        assert!(String::from_utf8_lossy(&diagnostic).contains("fake screenshot diagnostic"));
        assert_eq!(std::fs::read(&path).unwrap(), b"fake screenshot");
        std::fs::remove_file(&path).unwrap();

        let mut child = spawn("screenshot-wait");
        let completed = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(finish_screenshot(&mut child, receiver), async {
                while !path.exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                shutdown.send_replace(true);
            })
            .0
        })
        .await
        .unwrap()
        .unwrap();
        assert!(completed.is_none());
        assert!(!child.try_wait().unwrap().unwrap().success());
    }

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
            assert!(
                mcp_tool_request(&client, &invalid.to_string()).is_err(),
                "{invalid}"
            );
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
            let request = mcp_tool_request(&client, &valid.to_string())
                .unwrap()
                .build()
                .unwrap();
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
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"select_window","arguments":{"hwnd":42,"hwnd":null}}}
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"start_recording","arguments":{"fps":30,"fps":60}}}
{"jsonrpc":"2.0","id":6,"method":"tools/call","params":["start_recording"]}
{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"start_recording","arguments":null}}
"#;
        let mut output = Vec::new();
        // An invalid call must never reach the user's local RecordScreen API.
        let client = reqwest::blocking::Client::builder()
            .proxy(reqwest::Proxy::all("http://127.0.0.1:0").unwrap())
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap();
        serve_mcp_stdio(&client, "test-only-token", &input[..], &mut output);
        let responses: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(responses.len(), 8);
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
        for (response, field) in [(&responses[4], "hwnd"), (&responses[5], "fps")] {
            assert_eq!(response["result"]["isError"], true);
            assert!(
                response["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains(field),
                "{response}"
            );
        }
        assert_eq!(responses[6]["error"]["code"], -32602);
        assert_eq!(responses[7]["error"]["code"], -32602);
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
        let state = test_state(PathBuf::new());
        let (_shutdown, receiver) = watch::channel(false);
        serve_at(state.clone(), &address, receiver).await;
        let result = state.api_status.lock().unwrap().clone();
        assert!(matches!(result, ApiStatus::Failed(message) if message.contains(&address)));
    }
}
