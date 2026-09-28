use super::*;

const BACKGROUND: egui::Color32 = egui::Color32::from_rgb(12, 18, 30);
const SIDEBAR: egui::Color32 = egui::Color32::from_rgb(17, 25, 40);
const CARD: egui::Color32 = egui::Color32::from_rgb(24, 34, 52);
const MUTED: egui::Color32 = egui::Color32::from_rgb(151, 165, 187);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(80, 151, 244);
const RECORD: egui::Color32 = egui::Color32::from_rgb(231, 71, 84);

pub(super) fn configure_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = BACKGROUND;
    style.visuals.window_fill = CARD;
    style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(38, 51, 72);
    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(53, 73, 101);
    style.visuals.selection.bg_fill = ACCENT;
    style.spacing.item_spacing = egui::vec2(10.0, 9.0);
    style.spacing.button_padding = egui::vec2(13.0, 8.0);
    ctx.set_style(style);
}

impl eframe::App for RecorderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_finished_recording();
        let busy = self.recording() || self.countdown_deadline.is_some();
        ctx.request_repaint_after(if busy {
            Duration::from_millis(200)
        } else {
            Duration::from_secs(2)
        });

        if self.last_window_refresh.elapsed() >= Duration::from_secs(3) {
            self.windows = windows_capture::enumerate_windows();
            self.last_window_refresh = Instant::now();
        }
        self.selected_hwnd = self.selected_target().map(|target| target.hwnd);

        if let Some(deadline) = self.countdown_deadline {
            if self.recording() {
                self.countdown_deadline = None;
            } else if Instant::now() >= deadline {
                self.countdown_deadline = None;
                self.start_recording();
            }
        }

        if !ctx.wants_keyboard_input() {
            let toggle = ctx.input(|input| input.modifiers.ctrl && input.key_pressed(egui::Key::R));
            let screenshot = ctx.input(|input| {
                input.modifiers.ctrl && input.modifiers.shift && input.key_pressed(egui::Key::S)
            });
            if toggle {
                if self.recording() {
                    self.stop_recording();
                } else if self.countdown_deadline.is_some() {
                    self.countdown_deadline = None;
                } else if self.ffmpeg.is_some() && !self.has_pending_capture_settings() {
                    self.request_recording();
                }
            }
            if screenshot
                && !self.recording()
                && self.countdown_deadline.is_none()
                && self.ffmpeg.is_some()
                && !self.has_pending_capture_settings()
            {
                self.screenshot();
            }
        }

        self.show_header(ctx);
        self.show_footer(ctx);
        self.show_sidebar(ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND).inner_margin(20.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| match self.page {
                    AppPage::Capture => self.show_capture(ui),
                    AppPage::Library => self.show_library(ui),
                    AppPage::Settings => self.show_settings(ui),
                    AppPage::Agent => self.show_agent(ui),
                });
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.recording() {
            let _ = server::stop_capture(&self.shared);
        }
    }
}

impl RecorderApp {
    fn show_header(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("app-header")
            .frame(
                egui::Frame::new()
                    .fill(SIDEBAR)
                    .inner_margin(egui::Margin::symmetric(20, 13)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("●").color(RECORD).size(25.0));
                    ui.label(egui::RichText::new("RecordScreen").strong().size(20.0));
                    ui.label(
                        egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION"))).color(MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let state = if self.recording() {
                            self.language.text("● RECORDING", "● 錄影中")
                        } else if self.countdown_deadline.is_some() {
                            self.language.text("● COUNTDOWN", "● 倒數中")
                        } else {
                            self.language.text("● READY", "● 準備就緒")
                        };
                        ui.colored_label(if self.recording() { RECORD } else { ACCENT }, state);
                    });
                });
            });
    }

    fn show_footer(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("app-footer")
            .frame(
                egui::Frame::new()
                    .fill(SIDEBAR)
                    .inner_margin(egui::Margin::symmetric(20, 9)),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(ACCENT, "●");
                    ui.label(&self.status_message);
                });
                if let Ok(status) = self.shared.api_status.lock() {
                    if let server::ApiStatus::Failed(_) = &*status {
                        ui.colored_label(
                            RECORD,
                            self.language.text(
                                "Agent API unavailable — see Agent API for details.",
                                "Agent API 無法使用，請到 Agent API 頁查看原因。",
                            ),
                        );
                    }
                }
            });
    }

    fn show_sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("app-navigation")
            .resizable(false)
            .exact_width(185.0)
            .frame(
                egui::Frame::new()
                    .fill(SIDEBAR)
                    .inner_margin(egui::Margin::symmetric(13, 20)),
            )
            .show(ctx, |ui| {
                ui.colored_label(MUTED, self.language.text("WORKSPACE", "工作區"));
                ui.add_space(10.0);
                self.nav_button(
                    ui,
                    AppPage::Capture,
                    self.language.text("01  Capture", "01  擷取"),
                );
                self.nav_button(
                    ui,
                    AppPage::Library,
                    self.language.text("02  Library", "02  檔案庫"),
                );
                self.nav_button(
                    ui,
                    AppPage::Settings,
                    self.language.text("03  Settings", "03  設定"),
                );
                self.nav_button(ui, AppPage::Agent, "04  Agent API");
                ui.add_space(24.0);
                ui.separator();
                ui.small(
                    self.language
                        .text("Ctrl+R  Start / stop", "Ctrl+R  開始／停止"),
                );
                ui.small(
                    self.language
                        .text("Ctrl+Shift+S  Screenshot", "Ctrl+Shift+S  截圖"),
                );
            });
    }

    fn nav_button(&mut self, ui: &mut egui::Ui, page: AppPage, label: &str) {
        let mut button = egui::Button::new(label).min_size(egui::vec2(ui.available_width(), 34.0));
        if self.page == page {
            button = button.fill(egui::Color32::from_rgb(42, 66, 96));
        }
        if ui.add(button).clicked() {
            if page == AppPage::Library {
                self.refresh_library();
            }
            self.page = page;
        }
    }

    fn show_capture(&mut self, ui: &mut egui::Ui) {
        let lang = self.language;
        page_heading(
            ui,
            lang.text("Capture studio", "擷取工作台"),
            lang.text(
                "Record a window or your desktop, then find the result in Library.",
                "錄製視窗或桌面，完成後到檔案庫查看成果。",
            ),
        );

        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.colored_label(MUTED, lang.text("SESSION", "目前工作階段"));
                    let label = if self.recording() {
                        lang.text("Recording in progress", "正在錄影")
                    } else if self.countdown_deadline.is_some() {
                        lang.text("Recording starts in", "錄影即將開始")
                    } else {
                        lang.text("Ready to capture", "準備擷取")
                    };
                    ui.label(egui::RichText::new(label).size(22.0).strong());
                    ui.colored_label(
                        MUTED,
                        if self.selected_hwnd.is_some() {
                            lang.text("Selected app window", "指定應用程式視窗")
                        } else {
                            lang.text("Entire desktop", "整個桌面")
                        },
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let timer = if let Some(deadline) = self.countdown_deadline {
                        let remaining = deadline
                            .saturating_duration_since(Instant::now())
                            .as_millis();
                        format!("{}", remaining.div_ceil(1000))
                    } else if let Ok(guard) = self.shared.recording.lock() {
                        guard
                            .as_ref()
                            .map(|recording| format_duration(recording.started.elapsed().as_secs()))
                            .unwrap_or_else(|| "00:00:00".to_owned())
                    } else {
                        "00:00:00".to_owned()
                    };
                    ui.label(
                        egui::RichText::new(timer)
                            .monospace()
                            .size(30.0)
                            .color(if self.recording() { RECORD } else { ACCENT }),
                    );
                });
            });
        });

        ui.add_space(15.0);
        card(ui, |ui| {
            section_heading(ui, lang.text("Capture source", "擷取來源"));
            ui.small(lang.text(
                "The source stays locked during recording.",
                "錄影期間來源會鎖定。",
            ));
            ui.add_space(8.0);
            let busy = self.recording() || self.countdown_deadline.is_some();
            ui.horizontal(|ui| {
                let selected_name = self.source_name();
                ui.add_enabled_ui(!busy, |ui| {
                    egui::ComboBox::from_id_salt("capture-window")
                        .selected_text(selected_name)
                        .width((ui.available_width() - 100.0).max(220.0))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.selected_hwnd,
                                None,
                                lang.text("Entire desktop", "整個桌面"),
                            );
                            egui::ScrollArea::vertical()
                                .max_height(280.0)
                                .show(ui, |ui| {
                                    for window in &self.windows {
                                        ui.selectable_value(
                                            &mut self.selected_hwnd,
                                            Some(window.hwnd),
                                            format!(
                                                "{}  (PID {})",
                                                window.title, window.process_id
                                            ),
                                        );
                                    }
                                });
                        });
                });
                if ui
                    .add_enabled(!busy, egui::Button::new(lang.text("Refresh", "重新整理")))
                    .clicked()
                {
                    self.windows = windows_capture::enumerate_windows();
                    self.last_window_refresh = Instant::now();
                }
            });
            if !busy {
                self.sync_selected_target();
            }
            if self.windows.is_empty() {
                ui.colored_label(
                    MUTED,
                    lang.text(
                        "No app windows found. Open the app you want to record, then refresh.",
                        "找不到可擷取視窗。請開啟目標程式後重新整理。",
                    ),
                );
            }
        });

        ui.add_space(15.0);
        card(ui, |ui| {
            section_heading(ui, lang.text("Recording controls", "錄製控制"));
            ui.colored_label(
                MUTED,
                format!(
                    "{}  ·  {} FPS  ·  {}",
                    self.settings.output_dir.display(),
                    self.settings.fps,
                    match self.settings.countdown_seconds {
                        0 => lang.text("No countdown", "無倒數"),
                        _ => lang.text("Countdown enabled", "啟用倒數"),
                    }
                ),
            );
            ui.add_space(10.0);
            if self.has_pending_capture_settings() {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(
                        ACCENT,
                        lang.text(
                            "Capture settings changed. Apply them before starting.",
                            "擷取設定已變更，開始前請先套用。",
                        ),
                    );
                    if ui
                        .add_enabled(
                            !self.recording() && self.countdown_deadline.is_none(),
                            egui::Button::new(lang.text("Apply settings", "套用設定")),
                        )
                        .clicked()
                    {
                        self.apply_settings();
                    }
                });
                ui.add_space(8.0);
            }
            ui.horizontal_wrapped(|ui| {
                if self.recording() {
                    if ui
                        .add(
                            egui::Button::new(lang.text("■  Stop recording", "■  停止錄影"))
                                .fill(RECORD)
                                .min_size(egui::vec2(170.0, 42.0)),
                        )
                        .clicked()
                    {
                        self.stop_recording();
                    }
                } else if self.countdown_deadline.is_some() {
                    if ui
                        .add(
                            egui::Button::new(lang.text("Cancel countdown", "取消倒數"))
                                .min_size(egui::vec2(170.0, 42.0)),
                        )
                        .clicked()
                    {
                        self.countdown_deadline = None;
                        self.status_message = lang.text("Countdown cancelled", "已取消倒數").into();
                    }
                } else if ui
                    .add_enabled(
                        self.ffmpeg.is_some() && !self.has_pending_capture_settings(),
                        egui::Button::new(lang.text("●  Start recording", "●  開始錄影"))
                            .fill(RECORD)
                            .min_size(egui::vec2(170.0, 42.0)),
                    )
                    .clicked()
                {
                    self.request_recording();
                }
                if ui
                    .add_enabled(
                        self.ffmpeg.is_some()
                            && !self.recording()
                            && self.countdown_deadline.is_none()
                            && !self.has_pending_capture_settings(),
                        egui::Button::new(lang.text("▣  Screenshot", "▣  立即截圖"))
                            .min_size(egui::vec2(135.0, 42.0)),
                    )
                    .clicked()
                {
                    self.screenshot();
                }
                if ui.button(lang.text("Open folder", "開啟資料夾")).clicked() {
                    self.open_output_folder();
                }
            });
            if self.ffmpeg.is_none() {
                ui.colored_label(
                    RECORD,
                    lang.text(
                        "FFmpeg is unavailable. Re-run Setup and select Download FFmpeg.",
                        "FFmpeg 無法使用。請重新執行安裝程式並選擇下載 FFmpeg。",
                    ),
                );
            }
        });

        ui.add_space(15.0);
        let recent = self
            .last_capture
            .as_ref()
            .filter(|path| path.is_file())
            .cloned()
            .or_else(|| self.recent_captures.first().map(|item| item.path.clone()));
        card(ui, |ui| {
            section_heading(ui, lang.text("Latest capture", "最近成果"));
            if let Some(path) = &recent {
                ui.horizontal_wrapped(|ui| {
                    ui.label(path.file_name().unwrap_or_default().to_string_lossy());
                    if ui
                        .button(lang.text("Show in Explorer", "在檔案總管中顯示"))
                        .clicked()
                    {
                        if let Err(error) = reveal_in_explorer(path) {
                            self.status_message = error;
                        }
                    }
                });
            } else {
                ui.colored_label(
                    MUTED,
                    lang.text(
                        "Your completed recordings and screenshots will appear here.",
                        "完成的錄影與截圖會顯示在這裡。",
                    ),
                );
            }
        });
    }

    fn show_library(&mut self, ui: &mut egui::Ui) {
        let lang = self.language;
        page_heading(
            ui,
            lang.text("Capture library", "擷取檔案庫"),
            lang.text(
                "Recent files from the configured output folder.",
                "顯示目前輸出資料夾中的近期檔案。",
            ),
        );
        ui.horizontal(|ui| {
            if ui
                .button(lang.text("Refresh library", "重新整理檔案庫"))
                .clicked()
            {
                self.refresh_library();
            }
            if ui
                .button(lang.text("Open output folder", "開啟輸出資料夾"))
                .clicked()
            {
                self.open_output_folder();
            }
        });
        ui.add_space(12.0);
        if self.recent_captures.is_empty() {
            card(ui, |ui| {
                ui.colored_label(
                    MUTED,
                    lang.text(
                        "No RecordScreen captures found in this folder.",
                        "此資料夾目前沒有 RecordScreen 擷取檔。",
                    ),
                );
            });
            return;
        }
        let mut reveal = None;
        for item in &self.recent_captures {
            card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.vertical(|ui| {
                        ui.strong(item.path.file_name().unwrap_or_default().to_string_lossy());
                        ui.colored_label(
                            MUTED,
                            format!(
                                "{}  ·  {}",
                                format_size(item.size),
                                format_age(item.modified, lang)
                            ),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(lang.text("Show in Explorer", "在檔案總管中顯示"))
                            .clicked()
                        {
                            reveal = Some(item.path.clone());
                        }
                    });
                });
            });
            ui.add_space(8.0);
        }
        if let Some(path) = reveal {
            if let Err(error) = reveal_in_explorer(&path) {
                self.status_message = error;
            }
        }
    }

    fn show_settings(&mut self, ui: &mut egui::Ui) {
        let lang = self.language;
        page_heading(
            ui,
            lang.text("Recording settings", "錄製設定"),
            lang.text(
                "Choose where and how RecordScreen saves captures.",
                "設定擷取品質與儲存位置。",
            ),
        );
        card(ui, |ui| {
            section_heading(ui, lang.text("Output", "輸出"));
            ui.label(lang.text("Output folder", "輸出資料夾"));
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 100.0).max(200.0);
                ui.add(egui::TextEdit::singleline(&mut self.output_text).desired_width(width));
                if ui.button(lang.text("Browse…", "瀏覽…")).clicked() {
                    if let Some(folder) = pick_output_folder(&self.output_text) {
                        self.output_text = folder.display().to_string();
                    }
                }
            });
            ui.add_space(10.0);
            section_heading(ui, lang.text("Video", "影片"));
            ui.horizontal(|ui| {
                ui.label(lang.text("Frame rate", "影格率"));
                ui.add(egui::Slider::new(&mut self.fps, 10..=60).suffix(" FPS"));
            });
            ui.horizontal(|ui| {
                ui.label(lang.text("Start delay", "開始倒數"));
                egui::ComboBox::from_id_salt("countdown-setting")
                    .selected_text(if self.countdown_seconds == 0 {
                        lang.text("Off", "關閉").to_owned()
                    } else {
                        format!("{} s", self.countdown_seconds)
                    })
                    .show_ui(ui, |ui| {
                        for seconds in [0, 3, 5, 10] {
                            let label = if seconds == 0 {
                                lang.text("Off", "關閉").to_owned()
                            } else {
                                format!("{seconds} s")
                            };
                            ui.selectable_value(&mut self.countdown_seconds, seconds, label);
                        }
                    });
            });
            ui.add_space(10.0);
            section_heading(ui, lang.text("Language", "語系"));
            egui::ComboBox::from_id_salt("language-setting")
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
            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        !self.recording()
                            && self.countdown_deadline.is_none()
                            && self.has_pending_settings(),
                        egui::Button::new(lang.text("Apply settings", "套用設定")).fill(ACCENT),
                    )
                    .clicked()
                {
                    self.apply_settings();
                }
                if ui
                    .add_enabled(
                        self.has_pending_settings() && self.countdown_deadline.is_none(),
                        egui::Button::new(lang.text("Discard changes", "捨棄變更")),
                    )
                    .clicked()
                {
                    self.discard_settings();
                }
                if self.has_pending_settings() {
                    ui.colored_label(MUTED, lang.text("Unsaved changes", "設定尚未儲存"));
                } else {
                    ui.colored_label(MUTED, lang.text("All changes saved", "所有變更已儲存"));
                }
            });
        });
        ui.add_space(14.0);
        card(ui, |ui| {
            section_heading(ui, lang.text("Diagnostics", "診斷資訊"));
            ui.small(format!(
                "{}: {}",
                lang.text("Settings file", "設定檔"),
                settings::settings_file().display()
            ));
            if let Some(path) = &self.ffmpeg {
                ui.colored_label(ACCENT, format!("FFmpeg: {}", path.display()));
            } else {
                ui.colored_label(RECORD, lang.text("FFmpeg not found", "找不到 FFmpeg"));
            }
            if let Some(font) = &self.cjk_font {
                ui.small(format!("{}: {font}", lang.text("Chinese font", "中文字型")));
            }
        });
    }

    fn show_agent(&mut self, ui: &mut egui::Ui) {
        let lang = self.language;
        page_heading(
            ui,
            lang.text("Agent control", "Agent 控制"),
            lang.text(
                "Connect trusted local agents to RecordScreen.",
                "讓受信任的本機 Agent 操作 RecordScreen。",
            ),
        );
        card(ui, |ui| {
            section_heading(ui, lang.text("Local API", "本機 API"));
            ui.label("http://127.0.0.1:17321");
            if let Ok(status) = self.shared.api_status.lock() {
                match &*status {
                    server::ApiStatus::Starting => {
                        ui.colored_label(MUTED, lang.text("Starting…", "啟動中…"));
                    }
                    server::ApiStatus::Listening => {
                        ui.colored_label(
                            ACCENT,
                            lang.text("Listening — ready for agents", "已啟動，可供 Agent 連線"),
                        );
                    }
                    server::ApiStatus::Failed(error) => {
                        ui.colored_label(
                            RECORD,
                            lang.text("Agent API unavailable", "Agent API 無法使用"),
                        );
                        ui.label(error);
                        ui.small(lang.text(
                            "Check for another RecordScreen instance or an occupied port. Close the conflict and restart this app.",
                            "請檢查是否重複開啟 RecordScreen，或連接埠已被使用；排除衝突後重新啟動程式。"
                        ));
                    }
                }
            }
            ui.colored_label(
                MUTED,
                lang.text(
                    "The API accepts loopback connections only.",
                    "API 僅接受本機連線。",
                ),
            );
            ui.add_space(13.0);
            section_heading(ui, lang.text("Access token", "存取 Token"));
            ui.colored_label(
                MUTED,
                lang.text(
                    "Keep this token private. It is hidden until you reveal it.",
                    "請妥善保管 Token；只有按下顯示才會出現在畫面上。",
                ),
            );
            ui.horizontal_wrapped(|ui| {
                if self.show_token {
                    ui.monospace(&self.token);
                } else {
                    ui.monospace("••••••••••••••••••••••••");
                }
                if ui
                    .button(if self.show_token {
                        lang.text("Hide", "隱藏")
                    } else {
                        lang.text("Show", "顯示")
                    })
                    .clicked()
                {
                    self.show_token = !self.show_token;
                }
                if ui.button(lang.text("Copy token", "複製 Token")).clicked() {
                    ui.ctx().copy_text(self.token.clone());
                    self.status_message = lang
                        .text("Token copied to clipboard", "Token 已複製到剪貼簿")
                        .into();
                }
            });
            ui.small(format!(
                "{}: {}",
                lang.text("Token file", "Token 檔案"),
                token_file().display()
            ));
        });
        ui.add_space(14.0);
        card(ui, |ui| {
            section_heading(ui, lang.text("Agent tools", "Agent 工具"));
            ui.label(lang.text(
                "Status · Window list · Select window · Screenshot · Start / stop recording",
                "狀態 · 視窗清單 · 選取視窗 · 截圖 · 開始／停止錄影",
            ));
            ui.colored_label(
                MUTED,
                lang.text(
                    "See README.md for API and MCP connection examples.",
                    "API 與 MCP 連線範例請參閱 README.md。",
                ),
            );
        });
    }

    fn source_name(&self) -> String {
        match self.selected_hwnd {
            Some(hwnd) => self
                .windows
                .iter()
                .find(|window| window.hwnd == hwnd)
                .map(|window| format!("{}  (PID {})", window.title, window.process_id))
                .unwrap_or_else(|| {
                    self.language
                        .text("Window closed — choose another", "視窗已關閉，請重新選擇")
                        .into()
                }),
            None => self.language.text("Entire desktop", "整個桌面").into(),
        }
    }

    fn sync_selected_target(&self) {
        // Match the API's lock order and recheck after acquiring the lock: an
        // agent may have started recording since this frame drew its controls.
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

fn page_heading(ui: &mut egui::Ui, heading: &str, subtitle: &str) {
    ui.label(egui::RichText::new(heading).size(27.0).strong());
    ui.colored_label(MUTED, subtitle);
    ui.add_space(18.0);
}

fn section_heading(ui: &mut egui::Ui, heading: &str) {
    ui.label(egui::RichText::new(heading).size(17.0).strong());
}

fn card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(CARD)
        .corner_radius(12)
        .inner_margin(18.0)
        .show(ui, |ui| {
            ui.set_min_width((ui.available_width() - 36.0).max(260.0));
            add_contents(ui);
        });
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
