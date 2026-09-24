#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod server;
mod settings;
mod windows_capture;

use eframe::egui;
use serde::Serialize;
use server::{AppState, RecordingProcess};
use settings::Settings;
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_capture::WindowTarget;

#[derive(Clone, Serialize)]
struct Status {
    recording: bool,
    recording_file: Option<String>,
    output_dir: String,
    ffmpeg_available: bool,
    ffmpeg_path: String,
    capture_target: Option<WindowTarget>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Language {
    English,
    TraditionalChinese,
}

impl Language {
    fn from_code(code: &str) -> Self {
        match code {
            "zh-TW" => Self::TraditionalChinese,
            _ => Self::English,
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::English => "en-US",
            Self::TraditionalChinese => "zh-TW",
        }
    }

    fn text(self, en: &'static str, zh: &'static str) -> &'static str {
        match self {
            Self::English => en,
            Self::TraditionalChinese => zh,
        }
    }
}

fn main() -> eframe::Result {
    if std::env::args().any(|arg| arg == "--mcp") {
        server::run_mcp_stdio();
        return Ok(());
    }
    let ffmpeg = find_ffmpeg();
    let (settings, settings_error) = match Settings::load_or_create(default_output_dir()) {
        Ok(settings) => (settings, None),
        Err(error) => (
            Settings {
                language: "en-US".to_owned(),
                output_dir: default_output_dir(),
                fps: 30,
            },
            Some(error),
        ),
    };
    let output_dir = settings.output_dir.clone();
    let token = load_or_create_token().unwrap_or_else(|_| new_token());
    let shared = AppState {
        ffmpeg: ffmpeg.clone(),
        output_dir: Arc::new(Mutex::new(output_dir.clone())),
        recording: Arc::new(Mutex::new(None)),
        selected_target: Arc::new(Mutex::new(None)),
        token: Arc::new(token.clone()),
    };

    let server_state = shared.clone();
    std::thread::Builder::new()
        .name("agent-control-api".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build();
            if let Ok(runtime) = runtime {
                runtime.block_on(server::serve(server_state));
            }
        })
        .ok();

    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/recordscreen.png"))
        .expect("embedded RecordScreen icon must be a valid PNG");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 680.0])
            .with_min_inner_size([640.0, 580.0])
            .with_title("RecordScreen")
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native(
        "RecordScreen",
        options,
        Box::new(move |cc| {
            let cjk_font = load_traditional_chinese_font(&cc.egui_ctx);
            Ok(Box::new(RecorderApp::new(
                shared,
                token,
                settings,
                cjk_font,
                settings_error,
            )))
        }),
    )
}

struct RecorderApp {
    shared: AppState,
    settings: Settings,
    token: String,
    output_text: String,
    fps: u32,
    status_message: String,
    ffmpeg: Option<PathBuf>,
    windows: Vec<WindowTarget>,
    selected_hwnd: Option<usize>,
    last_window_refresh: Instant,
    cjk_font: Option<String>,
    language: Language,
    settings_needs_save: bool,
}

impl RecorderApp {
    fn new(
        shared: AppState,
        token: String,
        settings: Settings,
        cjk_font: Option<String>,
        settings_error: Option<String>,
    ) -> Self {
        let language = Language::from_code(&settings.language);
        let settings_needs_save = settings_error.is_some();
        Self {
            shared,
            token,
            output_text: settings.output_dir.display().to_string(),
            fps: settings.fps,
            status_message: settings_error
                .unwrap_or_else(|| language.text("Ready", "準備就緒").to_owned()),
            settings,
            ffmpeg: find_ffmpeg(),
            windows: windows_capture::enumerate_windows(),
            selected_hwnd: None,
            last_window_refresh: Instant::now(),
            cjk_font,
            language,
            settings_needs_save,
        }
    }

    fn selected_target(&self) -> Option<WindowTarget> {
        self.shared
            .selected_target
            .lock()
            .ok()
            .and_then(|target| target.clone())
    }

    fn recording(&self) -> bool {
        self.shared
            .recording
            .lock()
            .map(|r| r.is_some())
            .unwrap_or(false)
    }

    fn ensure_output_dir(&self) -> Result<PathBuf, String> {
        let path = self.settings.output_dir.clone();
        std::fs::create_dir_all(&path).map_err(|e| format!("無法建立資料夾：{e}"))?;
        Ok(path)
    }

    fn has_pending_settings(&self) -> bool {
        self.settings_needs_save
            || self.output_text.trim() != self.settings.output_dir.to_string_lossy()
            || self.fps != self.settings.fps
            || self.language.code() != self.settings.language
    }

    fn apply_settings(&mut self) {
        let output_dir = PathBuf::from(self.output_text.trim());
        if output_dir.as_os_str().is_empty() {
            self.status_message = self
                .language
                .text("Choose an output folder.", "請指定輸出資料夾。")
                .into();
            return;
        }
        if let Err(error) = std::fs::create_dir_all(&output_dir) {
            self.status_message = format!(
                "{}: {error}",
                self.language
                    .text("Could not create output folder", "無法建立輸出資料夾")
            );
            return;
        }
        let settings = Settings {
            language: self.language.code().to_owned(),
            output_dir,
            fps: self.fps,
        };
        if let Err(error) = settings.save() {
            self.status_message = format!(
                "{}: {error}",
                self.language
                    .text("Could not save settings", "無法儲存設定")
            );
            return;
        }
        if let Ok(mut current) = self.shared.output_dir.lock() {
            *current = settings.output_dir.clone();
        }
        self.settings = settings;
        self.settings_needs_save = false;
        self.status_message = self.language.text("Settings applied", "設定已套用").into();
    }

    fn screenshot(&mut self) {
        let Some(ffmpeg) = self.ffmpeg.clone() else {
            self.status_message = self
                .language
                .text(
                    "FFmpeg not found. Install it and add it to PATH.",
                    "找不到 FFmpeg，請先安裝並加入 PATH",
                )
                .into();
            return;
        };
        let dir = match self.ensure_output_dir() {
            Ok(dir) => dir,
            Err(error) => {
                self.status_message = error;
                return;
            }
        };
        let path = dir.join(format!("screenshot-{}.png", timestamp()));
        match capture_screenshot(&ffmpeg, &path, self.selected_target()) {
            Ok(()) => {
                self.status_message = match self.language {
                    Language::English => format!("Screenshot saved: {}", path.display()),
                    Language::TraditionalChinese => format!("截圖已儲存：{}", path.display()),
                }
            }
            Err(error) => {
                self.status_message = match self.language {
                    Language::English => format!("Screenshot failed: {error}"),
                    Language::TraditionalChinese => format!("截圖失敗：{error}"),
                }
            }
        }
    }

    fn start_recording(&mut self) {
        let Some(ffmpeg) = self.ffmpeg.clone() else {
            self.status_message = self
                .language
                .text(
                    "FFmpeg not found. Install it and add it to PATH.",
                    "找不到 FFmpeg，請先安裝並加入 PATH",
                )
                .into();
            return;
        };
        let dir = match self.ensure_output_dir() {
            Ok(dir) => dir,
            Err(error) => {
                self.status_message = error;
                return;
            }
        };
        let path = dir.join(format!("recording-{}.mp4", timestamp()));
        let mut recording = match self.shared.recording.lock() {
            Ok(recording) => recording,
            Err(_) => {
                self.status_message = self
                    .language
                    .text("Recording state is unavailable.", "錄影狀態無法使用")
                    .into();
                return;
            }
        };
        if recording.is_some() {
            self.status_message = self
                .language
                .text("Recording is already in progress.", "錄影已在進行中")
                .into();
            return;
        }
        match start_recording(&ffmpeg, &path, self.settings.fps, self.selected_target()) {
            Ok(child) => {
                *recording = Some(RecordingProcess {
                    child,
                    path: path.clone(),
                    started: Instant::now(),
                });
                self.status_message = match self.language {
                    Language::English => format!("Recording: {}", path.display()),
                    Language::TraditionalChinese => format!("錄影中：{}", path.display()),
                };
            }
            Err(error) => {
                self.status_message = match self.language {
                    Language::English => format!("Could not start recording: {error}"),
                    Language::TraditionalChinese => format!("無法開始錄影：{error}"),
                }
            }
        }
    }

    fn stop_recording(&mut self) {
        match server::stop_capture(&self.shared) {
            Ok(path) => {
                self.status_message = match self.language {
                    Language::English => format!("Recording saved: {}", path.display()),
                    Language::TraditionalChinese => format!("錄影已完成：{}", path.display()),
                }
            }
            Err(error) => {
                self.status_message = match self.language {
                    Language::English => format!("Could not stop recording: {error}"),
                    Language::TraditionalChinese => format!("停止錄影失敗：{error}"),
                }
            }
        }
    }
}

impl eframe::App for RecorderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_secs(2));
        if self.last_window_refresh.elapsed() >= Duration::from_secs(3) {
            self.windows = windows_capture::enumerate_windows();
            self.last_window_refresh = Instant::now();
        }
        self.selected_hwnd = self.selected_target().map(|target| target.hwnd);

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(format!("RecordScreen v{}", env!("CARGO_PKG_VERSION")));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::ComboBox::from_id_salt("language")
                        .selected_text(match self.language {
                            Language::English => "English",
                            Language::TraditionalChinese => "繁體中文",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.language, Language::English, "English");
                            ui.selectable_value(
                                &mut self.language,
                                Language::TraditionalChinese,
                                "繁體中文",
                            );
                        });
                });
            });
            let lang = self.language;
            ui.label(lang.text("Windows screen recorder and screenshot tool", "Windows 螢幕錄影與截圖"));
            ui.add_space(8.0);

            let recording = self.recording();
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(lang.text("Status", "狀態"));
                    ui.strong(if recording {
                        lang.text("● Recording", "● 錄影中")
                    } else {
                        lang.text("● Idle", "● 閒置")
                    });
                });
                if recording {
                    if let Ok(guard) = self.shared.recording.lock() {
                        if let Some(rec) = guard.as_ref() {
                            let seconds = rec.started.elapsed().as_secs();
                            ui.label(match lang {
                                Language::English => format!("Recorded {seconds} seconds"),
                                Language::TraditionalChinese => format!("已錄影 {seconds} 秒"),
                            });
                            ui.small(rec.path.display().to_string());
                        }
                    }
                }
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(lang.text("Capture source", "錄影來源"));
                let selected_name = match self.selected_hwnd {
                    Some(hwnd) => self
                        .windows
                        .iter()
                        .find(|window| window.hwnd == hwnd)
                        .map(|window| {
                            format!(
                                "{}  (PID {}, HWND 0x{:x})",
                                window.title, window.process_id, window.hwnd
                            )
                        })
                        .unwrap_or_else(|| lang.text("Window closed; refresh the list", "原視窗已關閉，請重新整理").into()),
                    None => lang.text("Entire desktop", "整個桌面").into(),
                };
                if recording {
                    ui.label(selected_name);
                } else {
                    egui::ComboBox::from_id_salt("capture-window")
                        .selected_text(selected_name)
                        .width(460.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.selected_hwnd, None, lang.text("Entire desktop", "整個桌面"));
                            egui::ScrollArea::vertical()
                                .max_height(260.0)
                                .show(ui, |ui| {
                                    for window in &self.windows {
                                        ui.selectable_value(
                                            &mut self.selected_hwnd,
                                            Some(window.hwnd),
                                            format!(
                                                "{}  (PID {}, HWND 0x{:x})",
                                                window.title, window.process_id, window.hwnd
                                            ),
                                        );
                                    }
                                });
                        });
                    if ui.button(lang.text("Refresh windows", "重新整理視窗")).clicked() {
                        self.windows = windows_capture::enumerate_windows();
                        self.last_window_refresh = Instant::now();
                    }
                }
            });
            if self.windows.is_empty() {
                ui.small(lang.text("No capturable app windows found. Make sure the target app is open and not minimized.", "目前找不到可擷取的 app 視窗，請確認目標 app 已開啟且未最小化。"));
            }
            ui.small(lang.text("When an app is selected, screenshots and recordings capture only that window. The source cannot be changed while recording.", "指定 app 後，錄影與截圖都只擷取該視窗；錄影期間不可切換來源。"));

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.label(lang.text("Output folder", "輸出資料夾"));
                ui.text_edit_singleline(&mut self.output_text);
            });

            let visible_hwnd = self.selected_hwnd;
            if let Ok(recording_state) = self.shared.recording.lock() {
                if recording_state.is_none() {
                    if let Ok(mut selected) = self.shared.selected_target.lock() {
                        if selected.as_ref().map(|window| window.hwnd) != visible_hwnd {
                            *selected = visible_hwnd.and_then(|hwnd| {
                                self.windows
                                    .iter()
                                    .find(|window| window.hwnd == hwnd)
                                    .cloned()
                            });
                        }
                    }
                }
            }
            ui.horizontal(|ui| {
                ui.label(lang.text("Frame rate", "影格率"));
                ui.add(egui::Slider::new(&mut self.fps, 10..=60).suffix(" FPS"));
            });

            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!recording && self.has_pending_settings(), egui::Button::new(lang.text("Apply settings", "套用設定")))
                    .clicked()
                {
                    self.apply_settings();
                }
                if self.has_pending_settings() {
                    ui.small(lang.text("Unsaved changes — apply before capture", "設定尚未儲存，請先套用再擷取"));
                }
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !recording && self.ffmpeg.is_some() && !self.has_pending_settings(),
                        egui::Button::new(lang.text("● Start recording", "● 開始錄影")),
                    )
                    .clicked()
                {
                    self.start_recording();
                }
                if ui
                    .add_enabled(recording, egui::Button::new(lang.text("■ Stop recording", "■ 停止錄影")))
                    .clicked()
                {
                    self.stop_recording();
                }
                if ui
                    .add_enabled(self.ffmpeg.is_some() && !self.has_pending_settings(), egui::Button::new(lang.text("▣ Take screenshot", "▣ 立即截圖")))
                    .clicked()
                {
                    self.screenshot();
                }
            });

            ui.add_space(12.0);
            ui.label(&self.status_message);
            ui.small(match lang {
                Language::English => format!("Settings: {}", settings::settings_file().display()),
                Language::TraditionalChinese => format!("設定檔：{}", settings::settings_file().display()),
            });
            if let Some(font) = &self.cjk_font {
                ui.small(match lang {
                    Language::English => format!("Chinese font: {font}"),
                    Language::TraditionalChinese => format!("中文字型：{font}"),
                });
            } else {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "Chinese font missing. Install Windows Traditional Chinese fonts.",
                );
            }
            if self.ffmpeg.is_none() {
                ui.colored_label(egui::Color32::RED, lang.text(
                    "FFmpeg not found. Re-run Setup and check Download FFmpeg under Capture dependency.",
                    "找不到 FFmpeg。請重新執行安裝程式，並勾選「Capture dependency」下的 FFmpeg 下載選項。",
                ));
            } else if let Some(path) = &self.ffmpeg {
                ui.small(format!("FFmpeg：{}", path.display()));
            }

            ui.separator();
            ui.heading(lang.text("Agent Control API", "Agent 控制 API"));
            ui.label(lang.text("The API listens only on 127.0.0.1:17321. Share the token below only with trusted local agents.", "API 僅監聽本機 127.0.0.1:17321；請將下方 Token 提供給受信任的本機 Agent。"));
            ui.monospace(format!("Bearer {0}", self.token));
            ui.small(lang.text("Token file: ", "Token 設定檔：").to_owned() + &token_file().display().to_string());
            ui.small(lang.text("See README.md for API details and PowerShell examples.", "API 說明與可直接使用的 PowerShell 範例請見 README.md。"));
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.recording() {
            let _ = server::stop_capture(&self.shared);
        }
    }
}

fn capture_screenshot(
    ffmpeg: &Path,
    output: &Path,
    target: Option<WindowTarget>,
) -> Result<(), String> {
    let mut cmd = hidden_command(ffmpeg);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "gdigrab",
        "-draw_mouse",
        "1",
        "-framerate",
        "1",
        "-i",
    ])
    .arg(capture_input(target.as_ref()))
    .args(["-frames:v", "1", "-update", "1"])
    .arg(output)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::piped());
    let result = cmd.output().map_err(|e| format!("啟動 FFmpeg 失敗：{e}"))?;
    if result.status.success() && output.exists() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&result.stderr).trim().to_string())
    }
}

fn start_recording(
    ffmpeg: &Path,
    output: &Path,
    fps: u32,
    target: Option<WindowTarget>,
) -> Result<Child, String> {
    let mut cmd = hidden_command(ffmpeg);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "gdigrab",
        "-draw_mouse",
        "1",
        "-framerate",
    ])
    .arg(fps.to_string())
    .arg("-i")
    .arg(capture_input(target.as_ref()))
    .args([
        "-vf",
        "scale=trunc(iw/2)*2:trunc(ih/2)*2",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "23",
        "-pix_fmt",
        "yuv420p",
        "-movflags",
        "+faststart",
        "-f",
        "mp4",
    ])
    .arg(output)
    .stdin(Stdio::piped())
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("啟動 FFmpeg 失敗：{e}"))?;
    std::thread::sleep(Duration::from_millis(500));
    if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
        return Err(format!("FFmpeg 無法擷取桌面（結束代碼 {status}）。請確認工作階段未鎖定，並查看 FFmpeg 是否支援 gdigrab。"));
    }
    Ok(child)
}

fn capture_input(target: Option<&WindowTarget>) -> String {
    target
        .map(|window| format!("hwnd=0x{:x}", window.hwnd))
        .unwrap_or_else(|| "desktop".into())
}

fn load_traditional_chinese_font(ctx: &egui::Context) -> Option<String> {
    let font_dir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts");
    let candidates = ["msjh.ttc", "mingliu.ttc", "kaiu.ttf", "msyh.ttc"];
    for filename in candidates {
        let path = font_dir.join(filename);
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let mut fonts = egui::FontDefinitions::default();
        let name = "windows-traditional-chinese".to_owned();
        fonts.font_data.insert(
            name.clone(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .insert(0, name.clone());
        }
        ctx.set_fonts(fonts);
        return Some(filename.to_owned());
    }
    None
}

#[cfg(windows)]
fn hidden_command(exe: &Path) -> Command {
    use std::os::windows::process::CommandExt;
    let mut command = Command::new(exe);
    command.creation_flags(0x08000000);
    command
}

#[cfg(not(windows))]
fn hidden_command(exe: &Path) -> Command {
    Command::new(exe)
}

fn find_ffmpeg() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("ffmpeg.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
            if let Some(candidate) = find_ffmpeg_in_tree(&dir.join("_ffmpeg_runtime"), 4) {
                return Some(candidate);
            }
        }
    }

    #[cfg(windows)]
    for candidate in common_ffmpeg_paths() {
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    #[cfg(windows)]
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        if let Some(candidate) =
            find_winget_ffmpeg(&PathBuf::from(local_app_data).join("Microsoft/WinGet/Packages"))
        {
            return Some(candidate);
        }
    }

    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for name in if cfg!(windows) {
            vec!["ffmpeg.exe", "ffmpeg"]
        } else {
            vec!["ffmpeg"]
        } {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
fn find_winget_ffmpeg(packages: &Path) -> Option<PathBuf> {
    for entry in std::fs::read_dir(packages).ok()?.flatten() {
        let path = entry.path();
        let is_gyan_ffmpeg = entry
            .file_name()
            .to_string_lossy()
            .starts_with("Gyan.FFmpeg");
        if is_gyan_ffmpeg {
            if let Some(candidate) = find_ffmpeg_in_tree(&path, 5) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
fn common_ffmpeg_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut add = |base: Option<std::ffi::OsString>, suffix: &str| {
        if let Some(base) = base {
            paths.push(PathBuf::from(base).join(suffix).join("ffmpeg.exe"));
        }
    };

    add(std::env::var_os("LOCALAPPDATA"), "Microsoft/WinGet/Links");
    add(std::env::var_os("LOCALAPPDATA"), "Programs/ffmpeg/bin");
    add(std::env::var_os("USERPROFILE"), "scoop/shims");
    add(
        std::env::var_os("USERPROFILE"),
        "scoop/apps/ffmpeg-essentials/current/bin",
    );
    add(
        std::env::var_os("USERPROFILE"),
        "scoop/apps/ffmpeg/current/bin",
    );
    add(std::env::var_os("ProgramData"), "chocolatey/bin");
    add(std::env::var_os("ProgramFiles"), "ffmpeg/bin");
    if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
        paths.push(PathBuf::from(program_files_x86).join("ffmpeg/bin/ffmpeg.exe"));
    }

    paths
}

#[cfg(windows)]
fn find_ffmpeg_in_tree(root: &Path, max_depth: usize) -> Option<PathBuf> {
    if max_depth == 0 || !root.is_dir() {
        return None;
    }

    let direct = root.join("ffmpeg.exe");
    if direct.is_file() {
        return Some(direct);
    }

    let bin = root.join("bin").join("ffmpeg.exe");
    if bin.is_file() {
        return Some(bin);
    }

    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(candidate) = find_ffmpeg_in_tree(&path, max_depth - 1) {
                return Some(candidate);
            }
        }
    }

    None
}

fn default_output_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Videos")
        .join("RecordScreen")
}

fn token_file() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("RecordScreen")
        .join("agent-token.txt")
}

fn load_or_create_token() -> Result<String, String> {
    let path = token_file();
    if let Ok(token) = std::fs::read_to_string(&path) {
        let token = token.trim().to_owned();
        if token.len() >= 32 {
            return Ok(token);
        }
    }
    let token = new_token();
    std::fs::create_dir_all(path.parent().ok_or("無效的 token 路徑")?)
        .map_err(|e| e.to_string())?;
    std::fs::write(path, &token).map_err(|e| e.to_string())?;
    Ok(token)
}

fn new_token() -> String {
    rand::random::<[u8; 32]>()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}

fn current_status(state: &AppState) -> Status {
    let recording_file = state
        .recording
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|r| r.path.display().to_string()));
    let output_dir = state
        .output_dir
        .lock()
        .map(|dir| dir.display().to_string())
        .unwrap_or_default();
    let capture_target = state
        .selected_target
        .lock()
        .ok()
        .and_then(|target| target.clone());
    Status {
        recording: recording_file.is_some(),
        recording_file,
        output_dir,
        ffmpeg_available: state.ffmpeg.is_some(),
        ffmpeg_path: state
            .ffmpeg
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        capture_target,
    }
}

// Keep this reference visible to rustdoc and prevent an unused import on older compilers.
#[allow(dead_code)]
fn _capture_types_are_send_sync(_: Arc<Mutex<Option<RecordingProcess>>>) {}
