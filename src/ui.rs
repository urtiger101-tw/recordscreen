use super::*;
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::{cell::RefCell, rc::Rc};

slint::include_modules!();

const COUNTDOWNS: [u32; 4] = [0, 3, 5, 10];
const TICK: Duration = Duration::from_millis(200);

/// Models and bookkeeping that mirror `RecorderApp` into the Slint window.
struct View {
    sources: Rc<VecModel<SharedString>>,
    /// Parallel to `sources`: `None` is the desktop, `Some(hwnd)` a window.
    source_hwnds: RefCell<Vec<Option<usize>>>,
    library: Rc<VecModel<CaptureRow>>,
    library_paths: RefCell<Vec<PathBuf>>,
    library_built: RefCell<Option<Instant>>,
}

pub(super) fn run(
    shared: AppState,
    token: String,
    settings: Settings,
    settings_error: Option<String>,
) -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    let app = Rc::new(RefCell::new(RecorderApp::new(
        shared,
        token,
        settings,
        settings_error,
    )));
    let view = Rc::new(View {
        sources: Rc::new(VecModel::default()),
        source_hwnds: RefCell::new(Vec::new()),
        library: Rc::new(VecModel::default()),
        library_paths: RefCell::new(Vec::new()),
        library_built: RefCell::new(None),
    });
    ui.set_sources(ModelRc::from(view.sources.clone()));
    ui.set_library(ModelRc::from(view.library.clone()));
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    {
        let app = app.borrow();
        ui.set_token(app.token.as_str().into());
        ui.set_token_file(token_file().display().to_string().into());
        ui.set_settings_file(settings::settings_file().display().to_string().into());
        ui.set_ffmpeg_ok(app.ffmpeg.is_some());
        ui.set_ffmpeg_path(
            app.ffmpeg
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default()
                .into(),
        );
        push_form(&ui, &app);
    }
    refresh(&ui, &app, &view);

    #[cfg(windows)]
    let tray = tray::Tray::new()
        .map_err(|error| eprintln!("RecordScreen tray icon unavailable: {error}"))
        .ok();
    #[cfg(windows)]
    let tray_available = tray.is_some();
    #[cfg(not(windows))]
    let tray_available = false;

    connect_callbacks(&ui, &app, &view, tray_available);

    let timer = Timer::default();
    {
        let ui = ui.as_weak();
        let app = app.clone();
        let view = view.clone();
        let mut was_minimized = false;
        timer.start(TimerMode::Repeated, TICK, move || {
            let Some(ui) = ui.upgrade() else { return };
            refresh(&ui, &app, &view);
            #[cfg(windows)]
            if let Some(tray) = &tray {
                tray.handle_events(&ui, &app, &view);
                if let Ok(app) = app.try_borrow() {
                    tray.update(&app);
                }
                // Hide on the transition into the minimized state so a later
                // restore from the taskbar is never fought by this timer.
                let minimized = ui.window().is_minimized();
                if minimized && !was_minimized && ui.get_tray_enabled() {
                    let _ = ui.hide();
                }
                was_minimized = minimized;
            }
        });
    }

    ui.window().on_close_requested(|| {
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::HideWindow
    });

    ui.show()?;
    #[cfg(debug_assertions)]
    let _snapshot = debug_snapshot(&ui);
    slint::run_event_loop_until_quit()?;
    drop(timer);

    let app = app.borrow();
    if app.recording() {
        let _ = server::stop_capture(&app.shared);
    }
    Ok(())
}

fn connect_callbacks(
    ui: &AppWindow,
    app: &Rc<RefCell<RecorderApp>>,
    view: &Rc<View>,
    tray_available: bool,
) {
    // Each handler pulls the editable form first so Rust sees unapplied edits,
    // then refreshes the projection immediately instead of waiting for a tick.
    macro_rules! on {
        ($setter:ident, |$app:ident, $ui:ident $(, $arg:ident : $ty:ty)*| $body:block) => {{
            let weak = ui.as_weak();
            let app = app.clone();
            let view = view.clone();
            ui.$setter(move |$($arg: $ty),*| {
                let Some($ui) = weak.upgrade() else { return };
                {
                    let Ok(mut guard) = app.try_borrow_mut() else { return };
                    let $app = &mut *guard;
                    pull_form(&$ui, $app);
                    $body
                }
                refresh(&$ui, &app, &view);
            });
        }};
    }

    on!(on_toggle_record, |app, _ui| { app.toggle_recording() });
    on!(on_cancel_countdown, |app, _ui| {
        app.countdown_deadline = None;
        app.status_message = app
            .language
            .text("Countdown cancelled", "已取消倒數")
            .into();
    });
    on!(on_screenshot, |app, _ui| { app.try_screenshot() });
    on!(on_open_folder, |app, _ui| { app.open_output_folder() });
    on!(on_reveal_latest, |app, _ui| {
        if let Some(path) = app.latest_capture() {
            if let Err(error) = reveal_in_explorer(&path) {
                app.status_message = error;
            }
        }
    });
    {
        let view_for_reveal = view.clone();
        on!(on_reveal, |app, _ui, index: i32| {
            let path = usize::try_from(index)
                .ok()
                .and_then(|index| view_for_reveal.library_paths.borrow().get(index).cloned());
            if let Some(path) = path {
                if let Err(error) = reveal_in_explorer(&path) {
                    app.status_message = error;
                }
            }
        });
    }
    on!(on_refresh_sources, |app, _ui| {
        app.windows = windows_capture::enumerate_windows();
        app.last_window_refresh = Instant::now();
    });
    {
        let view_for_source = view.clone();
        on!(on_source_selected, |app, _ui, index: i32| {
            let hwnd = usize::try_from(index)
                .ok()
                .and_then(|index| view_for_source.source_hwnds.borrow().get(index).copied());
            if let Some(hwnd) = hwnd {
                app.selected_hwnd = hwnd;
                app.sync_selected_target();
            }
        });
    }
    on!(on_refresh_library, |app, _ui| { app.refresh_library() });
    on!(on_page_changed, |app, _ui, page: i32| {
        if page == 1 {
            app.refresh_library();
        }
    });
    on!(on_apply_settings, |app, ui| {
        if !app.recording() && app.countdown_deadline.is_none() {
            app.apply_settings();
            push_form(&ui, app);
        }
    });
    on!(on_discard_settings, |app, ui| {
        app.discard_settings();
        push_form(&ui, app);
    });
    on!(on_copy_token, |app, _ui| {
        app.status_message = match copy_to_clipboard(&app.token) {
            Ok(()) => app
                .language
                .text("Token copied to clipboard", "Token 已複製到剪貼簿")
                .into(),
            Err(error) => error,
        };
    });
    {
        let weak = ui.as_weak();
        ui.on_hide_to_tray(move || {
            let Some(ui) = weak.upgrade() else { return };
            // Without a tray icon a hidden window could never be restored.
            if tray_available {
                let _ = ui.hide();
            } else {
                ui.window().set_minimized(true);
            }
        });
    }
    {
        // The folder dialog runs a nested message loop, so no RefCell borrow
        // may be held while it is open.
        let weak = ui.as_weak();
        ui.on_browse(move || {
            let Some(ui) = weak.upgrade() else { return };
            if let Some(folder) = pick_output_folder(&ui.get_output_text()) {
                ui.set_output_text(folder.display().to_string().into());
            }
        });
    }
}

/// Copy the editable settings form from the window into the app.
fn pull_form(ui: &AppWindow, app: &mut RecorderApp) {
    app.output_text = ui.get_output_text().to_string();
    app.fps = (ui.get_fps().round() as u32).clamp(10, 60);
    app.countdown_seconds = COUNTDOWNS[ui.get_countdown_index().clamp(0, 3) as usize];
    app.language = if ui.get_language_index() == 1 {
        Language::TraditionalChinese
    } else {
        Language::English
    };
    app.minimize_to_tray = ui.get_tray_enabled();
}

/// Overwrite the editable settings form; only on load, apply and discard.
fn push_form(ui: &AppWindow, app: &RecorderApp) {
    ui.set_output_text(app.output_text.as_str().into());
    ui.set_fps(app.fps as f32);
    ui.set_countdown_index(
        COUNTDOWNS
            .iter()
            .position(|seconds| *seconds == app.countdown_seconds)
            .unwrap_or(0) as i32,
    );
    ui.set_language_index(match app.language {
        Language::English => 0,
        Language::TraditionalChinese => 1,
    });
    ui.set_tray_enabled(app.minimize_to_tray);
}

/// Advance timers and project the current state into the window.
fn refresh(ui: &AppWindow, app: &Rc<RefCell<RecorderApp>>, view: &View) {
    let Ok(mut guard) = app.try_borrow_mut() else {
        return;
    };
    let app = &mut *guard;
    pull_form(ui, app);
    app.tick();

    let lang = app.language;
    ui.global::<I18n>()
        .set_zh(lang == Language::TraditionalChinese);
    let recording = app.recording();
    ui.set_recording(recording);
    ui.set_counting(app.countdown_deadline.is_some());
    ui.set_pending_capture(app.has_pending_capture_settings());
    ui.set_pending_settings(app.has_pending_settings());
    ui.set_timer_text(app.timer_text().into());
    ui.set_capture_summary(
        format!(
            "{} FPS  ·  {}  ·  {}",
            app.settings.fps,
            match app.settings.countdown_seconds {
                0 => lang.text("No countdown", "無倒數").to_owned(),
                seconds => format!("{seconds} s {}", lang.text("delay", "倒數")),
            },
            app.settings.output_dir.display()
        )
        .into(),
    );
    ui.set_latest_name(
        app.latest_capture()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_default()
            .into(),
    );
    ui.set_status(app.status_message.as_str().into());
    if let Ok(status) = app.shared.api_status.lock() {
        let (state, error) = match &*status {
            server::ApiStatus::Starting => (0, String::new()),
            server::ApiStatus::Listening => (1, String::new()),
            server::ApiStatus::Failed(error) => (2, error.clone()),
        };
        ui.set_api_state(state);
        ui.set_api_error(error.into());
    }

    update_sources(ui, app, view);
    update_library(app, view);
}

fn update_sources(ui: &AppWindow, app: &RecorderApp, view: &View) {
    let lang = app.language;
    let mut names = vec![SharedString::from(lang.text("Entire desktop", "整個桌面"))];
    let mut hwnds = vec![None];
    for window in &app.windows {
        names.push(format!("{}  (PID {})", window.title, window.process_id).into());
        hwnds.push(Some(window.hwnd));
    }
    if let Some(hwnd) = app.selected_hwnd {
        if !hwnds.contains(&Some(hwnd)) {
            names.push(
                lang.text("Window closed — choose another", "視窗已關閉，請重新選擇")
                    .into(),
            );
            hwnds.push(Some(hwnd));
        }
    }
    let changed = {
        let current = view.source_hwnds.borrow();
        *current != hwnds
            || view.sources.row_count() != names.len()
            || names
                .iter()
                .enumerate()
                .any(|(index, name)| view.sources.row_data(index).as_ref() != Some(name))
    };
    if changed {
        view.sources.set_vec(names);
        *view.source_hwnds.borrow_mut() = hwnds.clone();
    }
    let index = hwnds
        .iter()
        .position(|hwnd| *hwnd == app.selected_hwnd)
        .unwrap_or(0) as i32;
    if changed || ui.get_source_index() != index {
        ui.set_source_index(index);
    }
}

fn update_library(app: &RecorderApp, view: &View) {
    let paths: Vec<PathBuf> = app
        .recent_captures
        .iter()
        .map(|item| item.path.clone())
        .collect();
    // Rebuild when files change, and every 30 s so relative ages stay true.
    let stale = view
        .library_built
        .borrow()
        .is_none_or(|built| built.elapsed() >= Duration::from_secs(30));
    if !stale && *view.library_paths.borrow() == paths {
        return;
    }
    let rows: Vec<CaptureRow> = app
        .recent_captures
        .iter()
        .map(|item| CaptureRow {
            name: item
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .as_ref()
                .into(),
            detail: format!(
                "{}  ·  {}",
                format_size(item.size),
                format_age(item.modified, app.language)
            )
            .into(),
            is_video: item.path.extension().is_some_and(|ext| ext == "mp4"),
        })
        .collect();
    view.library.set_vec(rows);
    *view.library_paths.borrow_mut() = paths;
    *view.library_built.borrow_mut() = Some(Instant::now());
}

use slint::Model;

impl RecorderApp {
    /// Periodic work that the egui frame loop used to do.
    fn tick(&mut self) {
        self.poll_finished_recording();
        if self.last_window_refresh.elapsed() >= Duration::from_secs(3) {
            self.windows = windows_capture::enumerate_windows();
            self.last_window_refresh = Instant::now();
        }
        // An agent may change the target through the API at any time.
        self.selected_hwnd = self.selected_target().map(|target| target.hwnd);
        if let Some(deadline) = self.countdown_deadline {
            if self.recording() {
                self.countdown_deadline = None;
            } else if Instant::now() >= deadline {
                self.countdown_deadline = None;
                self.start_recording();
            }
        }
    }

    fn toggle_recording(&mut self) {
        if self.recording() {
            self.stop_recording();
        } else if self.countdown_deadline.is_some() {
            self.countdown_deadline = None;
            self.status_message = self
                .language
                .text("Countdown cancelled", "已取消倒數")
                .into();
        } else if self.ffmpeg.is_some() && !self.has_pending_capture_settings() {
            self.request_recording();
        }
    }

    fn try_screenshot(&mut self) {
        if !self.recording()
            && self.countdown_deadline.is_none()
            && self.ffmpeg.is_some()
            && !self.has_pending_capture_settings()
        {
            self.screenshot();
        }
    }

    fn timer_text(&self) -> String {
        if let Some(deadline) = self.countdown_deadline {
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .as_millis();
            return format!("{}", remaining.div_ceil(1000));
        }
        self.shared
            .recording
            .lock()
            .ok()
            .and_then(|guard| {
                guard
                    .as_ref()
                    .map(|recording| format_duration(recording.started.elapsed().as_secs()))
            })
            .unwrap_or_else(|| "00:00:00".to_owned())
    }

    fn latest_capture(&self) -> Option<PathBuf> {
        self.last_capture
            .as_ref()
            .filter(|path| path.is_file())
            .cloned()
            .or_else(|| self.recent_captures.first().map(|item| item.path.clone()))
    }

    fn sync_selected_target(&self) {
        // Match the API's lock order and recheck after acquiring the lock: an
        // agent may have started recording since the source list was drawn.
        let Ok(recording) = self.shared.recording.lock() else {
            return;
        };
        if recording.is_some() {
            return;
        }
        if let Ok(mut selected) = self.shared.selected_target.lock() {
            if selected.as_ref().map(|window| window.hwnd) != self.selected_hwnd {
                *selected = self.selected_hwnd.and_then(|hwnd| {
                    self.windows
                        .iter()
                        .find(|window| window.hwnd == hwnd)
                        .cloned()
                });
            }
        }
    }

    fn open_output_folder(&mut self) {
        if let Err(error) = open_folder(&self.settings.output_dir) {
            self.status_message = error;
        }
    }
}

#[cfg(windows)]
mod tray {
    use super::*;
    use tray_icon::{
        menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };

    pub(super) struct Tray {
        icon: TrayIcon,
        show: MenuItem,
        record: MenuItem,
        screenshot: MenuItem,
        quit: MenuItem,
        labels: RefCell<(Language, bool, bool)>,
    }

    impl Tray {
        pub(super) fn new() -> Result<Self, String> {
            let show = MenuItem::new("Show RecordScreen", true, None);
            let record = MenuItem::new("Start recording", true, None);
            let screenshot = MenuItem::new("Screenshot", true, None);
            let quit = MenuItem::new("Quit", true, None);
            let menu = Menu::new();
            menu.append_items(&[
                &show,
                &PredefinedMenuItem::separator(),
                &record,
                &screenshot,
                &PredefinedMenuItem::separator(),
                &quit,
            ])
            .map_err(|error| error.to_string())?;
            // The executable's icon resource (ordinal 1) is embedded by build.rs.
            let image = Icon::from_resource(1, None).map_err(|error| error.to_string())?;
            let icon = TrayIconBuilder::new()
                .with_icon(image)
                .with_tooltip("RecordScreen")
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .build()
                .map_err(|error| error.to_string())?;
            Ok(Self {
                icon,
                show,
                record,
                screenshot,
                quit,
                labels: RefCell::new((Language::English, false, false)),
            })
        }

        pub(super) fn handle_events(
            &self,
            ui: &AppWindow,
            app: &Rc<RefCell<RecorderApp>>,
            view: &View,
        ) {
            while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                let restore = matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } | TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    }
                );
                if restore {
                    show_window(ui);
                }
            }
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                if event.id == *self.show.id() {
                    show_window(ui);
                } else if event.id == *self.quit.id() {
                    let _ = slint::quit_event_loop();
                } else if event.id == *self.record.id() || event.id == *self.screenshot.id() {
                    if let Ok(mut app) = app.try_borrow_mut() {
                        pull_form(ui, &mut app);
                        if event.id == *self.record.id() {
                            app.toggle_recording();
                        } else {
                            app.try_screenshot();
                        }
                    }
                    refresh(ui, app, view);
                }
            }
        }

        pub(super) fn update(&self, app: &RecorderApp) {
            let state = (
                app.language,
                app.recording(),
                app.countdown_deadline.is_some(),
            );
            if *self.labels.borrow() == state {
                return;
            }
            let (lang, recording, counting) = state;
            self.show
                .set_text(lang.text("Show RecordScreen", "顯示 RecordScreen"));
            self.record.set_text(if recording {
                lang.text("Stop recording", "停止錄影")
            } else if counting {
                lang.text("Cancel countdown", "取消倒數")
            } else {
                lang.text("Start recording", "開始錄影")
            });
            self.screenshot.set_text(lang.text("Screenshot", "截圖"));
            self.screenshot.set_enabled(!recording && !counting);
            self.quit.set_text(lang.text("Quit", "結束"));
            let _ = self.icon.set_tooltip(Some(if recording {
                lang.text("RecordScreen — recording", "RecordScreen — 錄影中")
            } else {
                "RecordScreen"
            }));
            *self.labels.borrow_mut() = state;
        }
    }

    fn show_window(ui: &AppWindow) {
        let _ = ui.show();
        ui.window().set_minimized(false);
    }
}

/// Debug-only visual check: `RECORDSCREEN_SNAPSHOT=<out.rgba>;<page>` renders
/// the given page, writes raw RGBA pixels plus `<out.rgba>.size`, then quits.
#[cfg(debug_assertions)]
fn debug_snapshot(ui: &AppWindow) -> Option<Timer> {
    let spec = std::env::var("RECORDSCREEN_SNAPSHOT").ok()?;
    let (path, page) = spec.split_once(';').unwrap_or((spec.as_str(), "0"));
    ui.set_page(page.parse().unwrap_or(0));
    let path = PathBuf::from(path);
    let weak = ui.as_weak();
    let timer = Timer::default();
    timer.start(
        TimerMode::SingleShot,
        Duration::from_millis(1500),
        move || {
            if let Some(ui) = weak.upgrade() {
                if let Ok(buffer) = ui.window().take_snapshot() {
                    let _ = std::fs::write(&path, buffer.as_bytes());
                    let _ = std::fs::write(
                        path.with_extension("size"),
                        format!("{}x{}", buffer.width(), buffer.height()),
                    );
                }
            }
            let _ = slint::quit_event_loop();
        },
    );
    Some(timer)
}

#[cfg(windows)]
fn copy_to_clipboard(text: &str) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_text(text.to_owned()))
        .map_err(|error| format!("Could not copy token: {error}"))
}

#[cfg(not(windows))]
fn copy_to_clipboard(_text: &str) -> Result<(), String> {
    Err("Clipboard is only available on Windows".to_owned())
}

fn format_duration(seconds: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60
    )
}

fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

fn format_age(modified: SystemTime, language: Language) -> String {
    let seconds = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default()
        .as_secs();
    match seconds {
        0..=59 => language.text("just now", "剛剛").to_owned(),
        60..=3599 => format!("{} {}", seconds / 60, language.text("min ago", "分鐘前")),
        3600..=86399 => format!("{} {}", seconds / 3600, language.text("h ago", "小時前")),
        _ => format!("{} {}", seconds / 86400, language.text("d ago", "天前")),
    }
}

#[cfg(windows)]
fn open_folder(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path)
        .map_err(|error| format!("Could not create output folder: {error}"))?;
    Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not open Explorer: {error}"))
}

#[cfg(windows)]
fn reveal_in_explorer(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err("Capture file is no longer available".to_owned());
    }
    Command::new("explorer.exe")
        .arg(format!("/select,{}", path.display()))
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not open Explorer: {error}"))
}

#[cfg(windows)]
fn pick_output_folder(current: &str) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new();
    let current = Path::new(current);
    if current.is_dir() {
        dialog = dialog.set_directory(current);
    }
    dialog.pick_folder()
}

#[cfg(not(windows))]
fn pick_output_folder(_current: &str) -> Option<PathBuf> {
    None
}

#[cfg(not(windows))]
fn open_folder(_path: &Path) -> Result<(), String> {
    Err("Explorer is only available on Windows".to_owned())
}

#[cfg(not(windows))]
fn reveal_in_explorer(_path: &Path) -> Result<(), String> {
    Err("Explorer is only available on Windows".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_sizes_are_formatted_for_display() {
        assert_eq!(format_duration(3_725), "01:02:05");
        assert_eq!(format_size(1_500), "2 KB");
        assert_eq!(format_size(3 * 1024 * 1024), "3.0 MB");
    }

    #[test]
    fn countdown_options_match_settings_validation() {
        assert_eq!(COUNTDOWNS, [0, 3, 5, 10]);
    }
}
