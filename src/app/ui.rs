//! Main window (egui).

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use clearmic::audio::{self, DeviceInfo, DeviceKind, DeviceList};
use clearmic::config::Settings;
use clearmic::engine::{Engine, EngineParams, Event, StartInfo};
use clearmic::stats::{Snapshot, to_db};
use clearmic::{APP_NAME, MODEL_NAME, VB_CABLE_CREDIT, VB_CABLE_URL, VERSION, platform};
use egui::{
    Align, Color32, CornerRadius, Layout, Margin, RichText, Sense, Stroke, Ui, Vec2,
    ViewportCommand,
};

use super::hotkey::{self, Hotkeys};
use super::icon;
use super::tray::{Tray, TrayMsg};

const BG: Color32 = Color32::from_rgb(15, 17, 21);
const PANEL: Color32 = Color32::from_rgb(24, 27, 34);
const PANEL_2: Color32 = Color32::from_rgb(33, 37, 47);
const HOVER: Color32 = Color32::from_rgb(45, 50, 62);
const TEXT: Color32 = Color32::from_rgb(228, 231, 236);
const MUTED: Color32 = Color32::from_rgb(142, 150, 163);
const ACCENT: Color32 = Color32::from_rgb(46, 230, 166);
const ACCENT_DIM: Color32 = Color32::from_rgb(24, 112, 84);
const WARN: Color32 = Color32::from_rgb(255, 184, 77);
const DANGER: Color32 = Color32::from_rgb(255, 107, 107);
const BLUE: Color32 = Color32::from_rgb(98, 170, 255);

const PRESETS: [(&str, u8); 4] = [
    ("Light", 40),
    ("Balanced", 70),
    ("Strong", 85),
    ("Maximum", 100),
];
const BUFFERS: [(u32, &str); 4] = [
    (20, "Lowest latency (20 ms)"),
    (30, "Balanced (30 ms)"),
    (60, "Safe (60 ms)"),
    (100, "Very safe (100 ms)"),
];

#[derive(Debug, Clone)]
enum EngineState {
    Starting,
    Running(StartInfo),
    Failed(String),
}

pub struct App {
    settings: Settings,
    applied: Settings,
    dirty_since: Option<Instant>,
    devices: DeviceList,
    devices_refreshed: Instant,
    devices_rx: Option<Receiver<(DeviceList, bool)>>,
    /// VB-CABLE driver registered with Windows (its endpoints may still need a reboot).
    driver_present: bool,
    /// VB-CABLE setup shipped next to the executable, if any.
    bundled_setup: Option<std::path::PathBuf>,
    driver_job: Option<Receiver<Result<(), String>>>,
    show_rx: Receiver<()>,
    running_since: Option<Instant>,
    engine: Option<Engine>,
    state: EngineState,
    snap: Snapshot,
    tray: Option<Tray>,
    hotkeys: Option<Hotkeys>,
    quit: bool,
    hidden: bool,
    restart_at: Option<Instant>,
    restart_attempts: u32,
    notice: Option<String>,
    meter_in_db: f32,
    meter_out_db: f32,
}

pub fn run(start_minimized: bool) -> anyhow::Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_app_id(clearmic::APP_ID)
        .with_inner_size([460.0, 740.0])
        .with_min_inner_size([420.0, 520.0])
        .with_icon(icon::egui_icon())
        .with_visible(!start_minimized);
    let options = eframe::NativeOptions {
        viewport,
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, start_minimized)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

fn apply_theme(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = Color32::from_rgb(10, 12, 15);
    v.faint_bg_color = PANEL_2;
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = BLUE;
    v.selection.bg_fill = ACCENT_DIM;
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    let r = CornerRadius::same(6);
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.corner_radius = r;
    v.widgets.open.corner_radius = r;
    v.widgets.inactive.bg_fill = PANEL_2;
    v.widgets.inactive.weak_bg_fill = PANEL_2;
    v.widgets.hovered.bg_fill = HOVER;
    v.widgets.hovered.weak_bg_fill = HOVER;
    v.widgets.active.bg_fill = ACCENT_DIM;
    v.widgets.active.weak_bg_fill = ACCENT_DIM;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(42, 47, 58));
    ctx.set_visuals(v);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(10.0, 5.0);
        s.spacing.interact_size.y = 26.0;
    });
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, start_minimized: bool) -> Self {
        let ctx = cc.egui_ctx.clone();
        apply_theme(&ctx);
        let mut settings = Settings::load();
        settings.autostart = platform::autostart_enabled(APP_NAME);
        let start_minimized = start_minimized || settings.start_minimized;

        let tray = match Tray::new(ctx.clone(), settings.enabled) {
            Ok(t) => Some(t),
            Err(e) => {
                log::warn!("system tray unavailable: {e:#}");
                None
            }
        };
        let hotkeys = if settings.hotkey_enabled {
            Hotkeys::new(ctx.clone())
                .map_err(|e| log::warn!("global hotkey unavailable: {e:#}"))
                .ok()
        } else {
            None
        };

        // Heartbeat: keeps `logic()` running (auto-restart, tray) while the window is hidden.
        {
            let ctx = ctx.clone();
            let _ = std::thread::Builder::new()
                .name("clearmic-heartbeat".into())
                .spawn(move || {
                    loop {
                        std::thread::sleep(Duration::from_millis(500));
                        ctx.request_repaint();
                    }
                });
        }

        // A second launch of the app asks this instance to show its window.
        let (show_tx, show_rx) = channel();
        {
            let ctx = ctx.clone();
            platform::on_show_window_signal(move || {
                let _ = show_tx.send(());
                ctx.request_repaint();
            });
        }

        let hidden = start_minimized && tray.is_some();
        ctx.send_viewport_cmd(ViewportCommand::Visible(!hidden));

        let devices = audio::enumerate();
        let driver_present =
            devices.virtual_cable_output().is_some() || platform::virtual_mic_driver_present();

        let mut app = Self {
            applied: settings.clone(),
            settings,
            dirty_since: None,
            devices,
            devices_refreshed: Instant::now(),
            devices_rx: None,
            driver_present,
            bundled_setup: platform::bundled_driver_setup(),
            driver_job: None,
            show_rx,
            running_since: None,
            engine: None,
            state: EngineState::Starting,
            snap: Snapshot::default(),
            tray,
            hotkeys,
            quit: false,
            hidden,
            restart_at: None,
            restart_attempts: 0,
            notice: None,
            meter_in_db: -60.0,
            meter_out_db: -60.0,
        };
        app.start_engine();
        app
    }

    fn engine_params(&self) -> EngineParams {
        EngineParams {
            input_device: self.settings.input_device.clone(),
            output_device: self.settings.output_device.clone(),
            monitor_device: self
                .settings
                .monitor_enabled
                .then(|| self.settings.monitor_device.clone())
                .flatten(),
            proc: self.settings.proc_params(),
            buffer_ms: self.settings.buffer_ms,
            mute_output: false,
        }
    }

    fn start_engine(&mut self) {
        self.engine = None; // the previous audio thread stops on its own (non-blocking)
        self.running_since = None;
        self.state = EngineState::Starting;
        self.restart_at = None;
        self.applied = self.settings.clone();
        self.engine = Some(Engine::start(self.engine_params()));
    }

    /// Re-enumerate audio endpoints on a helper thread (it can take a while with many devices).
    fn refresh_devices(&mut self) {
        self.devices_refreshed = Instant::now();
        if self.devices_rx.is_some() {
            return;
        }
        let (tx, rx) = channel();
        self.devices_rx = Some(rx);
        let _ = std::thread::Builder::new()
            .name("clearmic-devices".into())
            .spawn(move || {
                let list = audio::enumerate();
                let driver = list.virtual_cable_output().is_some()
                    || platform::virtual_mic_driver_present();
                let _ = tx.send((list, driver));
            });
    }

    /// Install the bundled VB-CABLE driver on a helper thread (Windows asks for consent once).
    fn start_driver_install(&mut self) {
        let Some(setup) = self.bundled_setup.clone() else {
            return;
        };
        if self.driver_job.is_some() {
            return;
        }
        let (tx, rx) = channel();
        self.driver_job = Some(rx);
        let _ = std::thread::Builder::new()
            .name("clearmic-driver-install".into())
            .spawn(move || {
                let result =
                    platform::install_virtual_mic_driver(&setup).map_err(|e| format!("{e:#}"));
                let _ = tx.send(result);
            });
    }

    fn poll_driver_job(&mut self) {
        let Some(rx) = &self.driver_job else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(())) => {
                log::info!("virtual microphone driver installed");
                self.driver_job = None;
                self.notice = None;
                self.driver_present = true;
                self.devices_rx = None;
                self.refresh_devices();
            }
            Ok(Err(msg)) => {
                log::warn!("virtual microphone driver install failed: {msg}");
                self.driver_job = None;
                self.notice = Some(format!("The virtual microphone was not installed: {msg}"));
            }
            Err(TryRecvError::Disconnected) => self.driver_job = None,
            Err(TryRecvError::Empty) => {}
        }
    }

    fn poll_devices(&mut self) {
        let Some(rx) = &self.devices_rx else {
            return;
        };
        match rx.try_recv() {
            Ok((list, driver_present)) => {
                let cable_now = list.virtual_cable_output().is_some();
                self.devices = list;
                self.driver_present = driver_present;
                self.devices_rx = None;
                // The cable just became available: move the audio onto it automatically.
                let on_fallback = matches!(
                    &self.state,
                    EngineState::Running(info) if !info.output_is_virtual_cable
                );
                if cable_now && on_fallback && self.settings.output_device.is_none() {
                    log::info!("virtual microphone detected; switching output to it");
                    self.start_engine();
                }
            }
            Err(TryRecvError::Disconnected) => self.devices_rx = None,
            Err(TryRecvError::Empty) => {}
        }
    }

    fn poll_engine(&mut self) {
        let mut stopped = false;
        if let Some(engine) = &self.engine {
            while let Some(ev) = engine.poll_event() {
                match ev {
                    Event::Starting => self.state = EngineState::Starting,
                    Event::Started(info) => {
                        log::info!(
                            "audio running: '{}' -> '{}'",
                            info.input_name,
                            info.output_name
                        );
                        self.state = EngineState::Running(info);
                        self.running_since = Some(Instant::now());
                    }
                    Event::Error(msg) => {
                        log::error!("audio engine error: {msg}");
                        self.state = EngineState::Failed(msg);
                    }
                    Event::Stopped => stopped = true,
                }
            }
            self.snap = engine.stats().snapshot();
        }
        if stopped {
            self.engine = None;
            self.snap = Snapshot::default();
            // Only a run that stayed up for a while resets the retry backoff.
            let was_stable = self
                .running_since
                .take()
                .is_some_and(|t| t.elapsed() > Duration::from_secs(30));
            if was_stable {
                self.restart_attempts = 0;
            }
            if !matches!(self.state, EngineState::Failed(_)) {
                self.state = EngineState::Failed("audio engine stopped".into());
            }
            if self.settings.auto_restart {
                let secs = (2u64 << self.restart_attempts.min(4)).min(30);
                self.restart_at = Some(Instant::now() + Duration::from_secs(secs));
                self.restart_attempts += 1;
            }
        }
        if self.restart_at.is_some_and(|at| Instant::now() >= at) {
            self.refresh_devices();
            self.start_engine();
        }
    }

    fn apply_changes(&mut self, ctx: &egui::Context) {
        if self.settings == self.applied {
            return;
        }
        let s = &self.settings;
        let a = &self.applied;
        let restart = s.input_device != a.input_device
            || s.output_device != a.output_device
            || s.monitor_device != a.monitor_device
            || s.monitor_enabled != a.monitor_enabled
            || s.buffer_ms != a.buffer_ms;
        if s.enabled != a.enabled {
            if let Some(t) = &self.tray {
                t.set_active(s.enabled);
            }
        }
        if s.hotkey_enabled != a.hotkey_enabled {
            self.hotkeys = None;
            if s.hotkey_enabled {
                self.hotkeys = Hotkeys::new(ctx.clone())
                    .map_err(|e| log::warn!("global hotkey unavailable: {e:#}"))
                    .ok();
            }
        }
        if restart {
            self.restart_attempts = 0;
            self.start_engine();
        } else {
            if let Some(e) = &self.engine {
                e.update(self.settings.proc_params());
            }
            self.applied = self.settings.clone();
        }
        self.dirty_since.get_or_insert_with(Instant::now);
    }

    fn maybe_save(&mut self, force: bool) {
        let due = self
            .dirty_since
            .is_some_and(|t| force || t.elapsed() > Duration::from_millis(800));
        if due {
            if let Err(e) = self.settings.save() {
                log::warn!("could not save settings: {e:#}");
            }
            self.dirty_since = None;
        }
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        self.hidden = false;
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    fn status_line(&self) -> (String, Color32) {
        match &self.state {
            EngineState::Starting => ("Starting audio engine...".into(), MUTED),
            EngineState::Failed(msg) => {
                let retry = match self.restart_at {
                    Some(at) => format!(
                        " - retrying in {} s",
                        at.saturating_duration_since(Instant::now()).as_secs() + 1
                    ),
                    None => String::new(),
                };
                (format!("Audio problem: {msg}{retry}"), DANGER)
            }
            EngineState::Running(_) => {
                if !self.settings.enabled || self.settings.strength == 0 {
                    ("Pass-through: microphone is sent unprocessed".into(), MUTED)
                } else if !self.snap.gate_open {
                    ("Silence - voice gate closed".into(), MUTED)
                } else if self.snap.voice_active {
                    ("Voice detected - background noise removed".into(), ACCENT)
                } else {
                    ("Listening - background noise removed".into(), TEXT)
                }
            }
        }
    }

    // ------------------------------------------------------------------ drawing

    fn draw(&mut self, ui: &mut Ui) {
        self.header(ui);
        self.power_card(ui);
        self.meters_card(ui);
        self.devices_card(ui);
        self.suppression_card(ui);
        self.advanced_card(ui);
        self.footer(ui);
    }

    fn header(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(APP_NAME).size(22.0).strong());
            ui.label(RichText::new("AI noise cancellation").size(12.0).color(MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(format!("v{VERSION}")).size(11.0).color(MUTED));
            });
        });
    }

    fn power_card(&mut self, ui: &mut Ui) {
        let (status, status_color) = self.status_line();
        let warn_output = match &self.state {
            EngineState::Running(info) if !info.output_is_virtual_cable => {
                Some(info.output_name.clone())
            }
            _ => None,
        };
        card(ui, |ui| {
            ui.horizontal(|ui| {
                let mut on = self.settings.enabled;
                if toggle_switch(ui, &mut on).changed() {
                    self.settings.enabled = on;
                }
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let (title, color) = if self.settings.enabled {
                        ("Noise cancellation is ON", ACCENT)
                    } else {
                        ("Noise cancellation is OFF", MUTED)
                    };
                    ui.label(RichText::new(title).size(16.0).strong().color(color));
                    ui.label(RichText::new(status).size(12.0).color(status_color));
                });
            });
            if let Some(name) = warn_output {
                ui.label(
                    RichText::new(format!(
                        "Clean audio is playing on \"{}\" because no virtual microphone is installed. \
                         Other apps cannot use it yet - see Devices below.",
                        elide(&name, 40)
                    ))
                    .size(12.0)
                    .color(WARN),
                );
            }
            if let Some(n) = &self.notice {
                ui.label(RichText::new(n).size(12.0).color(WARN));
            }
        });
    }

    fn meters_card(&mut self, ui: &mut Ui) {
        let target_in = to_db(self.snap.input_peak).max(-60.0);
        let target_out = to_db(self.snap.output_peak).max(-60.0);
        self.meter_in_db = smooth_meter(self.meter_in_db, target_in);
        self.meter_out_db = smooth_meter(self.meter_out_db, target_out);
        let voice = self.snap.voice_active && matches!(self.state, EngineState::Running(_));
        card(ui, |ui| {
            level_meter(ui, "Microphone", self.meter_in_db, MUTED);
            level_meter(ui, "Clean output", self.meter_out_db, ACCENT);
            ui.horizontal(|ui| {
                dot(ui, if voice { ACCENT } else { PANEL_2 });
                ui.label(
                    RichText::new(if voice { "Voice" } else { "No voice" })
                        .size(12.0)
                        .color(if voice { TEXT } else { MUTED }),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("speech/noise {:+.0} dB", self.snap.lsnr_db))
                            .size(11.0)
                            .monospace()
                            .color(MUTED),
                    );
                });
            });
        });
    }

    fn devices_card(&mut self, ui: &mut Ui) {
        let cable_out = self.devices.virtual_cable_output().map(|d| d.name.clone());
        let cable_in = self.devices.virtual_cable_input().map(|d| d.name.clone());
        let mut refresh = false;
        let mut install = false;
        let driver_present = self.driver_present;
        let installing = self.driver_job.is_some();
        let has_bundled = self.bundled_setup.is_some();
        let speakers_hijacked = self
            .devices
            .outputs
            .iter()
            .any(|d| d.is_default && d.kind == DeviceKind::VirtualCable);
        card(ui, |ui| {
            section_title(ui, "DEVICES");
            ui.label(RichText::new("Microphone").size(12.0).color(MUTED));
            device_combo(
                ui,
                "mic",
                &mut self.settings.input_device,
                &self.devices.inputs,
                "System default microphone",
            );

            ui.label(RichText::new("Send clean audio to").size(12.0).color(MUTED));
            let auto_label = match &cable_out {
                Some(n) => format!("Auto: {n}"),
                None => "Auto (no virtual microphone found)".to_string(),
            };
            device_combo(
                ui,
                "out",
                &mut self.settings.output_device,
                &self.devices.outputs,
                &auto_label,
            );

            match &cable_out {
                Some(_) => {
                    let mic = cable_in.as_deref().unwrap_or("CABLE Output");
                    ui.horizontal_wrapped(|ui| {
                        dot(ui, ACCENT);
                        ui.label(
                            RichText::new(format!(
                                "Virtual microphone ready. In Zoom / Teams / Meet pick \"{mic}\" as your microphone."
                            ))
                            .size(12.0)
                            .color(TEXT),
                        );
                    });
                }
                None => {
                    let msg = if installing {
                        "Installing the virtual microphone driver. Approve the Windows prompt to continue."
                    } else if driver_present {
                        "The virtual microphone driver is installed but not active yet. Restart Windows to finish the setup."
                    } else {
                        "The virtual microphone is not installed yet. It is needed so other apps can use the clean audio."
                    };
                    ui.horizontal_wrapped(|ui| {
                        dot(ui, WARN);
                        ui.label(RichText::new(msg).size(12.0).color(WARN));
                    });
                }
            }
            if speakers_hijacked {
                ui.horizontal_wrapped(|ui| {
                    dot(ui, WARN);
                    ui.label(
                        RichText::new(
                            "Windows is using the virtual cable as your speakers, so you may hear nothing. Choose your real speakers in Sound settings.",
                        )
                        .size(12.0)
                        .color(WARN),
                    );
                });
            }
            ui.horizontal(|ui| {
                if cable_out.is_none() && !driver_present {
                    if has_bundled {
                        let button = egui::Button::new("Install virtual microphone");
                        if ui.add_enabled(!installing, button).clicked() {
                            install = true;
                        }
                    } else if ui.button("Get VB-CABLE").clicked() {
                        platform::open_url(VB_CABLE_URL);
                    }
                }
                if speakers_hijacked && ui.button("Open Sound settings").clicked() {
                    platform::open_url("ms-settings:sound");
                }
                if ui.button("Refresh devices").clicked() {
                    refresh = true;
                }
            });
            ui.hyperlink_to(RichText::new(VB_CABLE_CREDIT).size(10.5), VB_CABLE_URL);
        });
        if refresh {
            self.devices_rx = None;
            self.refresh_devices();
        }
        if install {
            self.start_driver_install();
        }
    }

    fn suppression_card(&mut self, ui: &mut Ui) {
        card(ui, |ui| {
            section_title(ui, "NOISE REMOVAL");
            ui.horizontal(|ui| {
                ui.label("Strength");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{}%", self.settings.strength))
                            .monospace()
                            .color(ACCENT),
                    );
                });
            });
            ui.spacing_mut().slider_width = ui.available_width();
            ui.add(egui::Slider::new(&mut self.settings.strength, 0..=100).show_value(false));
            ui.horizontal(|ui| {
                for (name, value) in PRESETS {
                    if ui
                        .selectable_label(self.settings.strength == value, name)
                        .clicked()
                    {
                        self.settings.strength = value;
                    }
                }
            });
            ui.add_space(2.0);
            ui.checkbox(
                &mut self.settings.voice_gate,
                "Voice gate - mute completely between words",
            );
            ui.checkbox(
                &mut self.settings.post_filter,
                "Extra filter for constant noise (fans, air conditioning)",
            );
            ui.checkbox(
                &mut self.settings.high_pass,
                "Low-cut filter for rumble below 80 Hz (usually not needed)",
            );
        });
    }

    fn advanced_card(&mut self, ui: &mut Ui) {
        let mut restart = false;
        let mut open_logs = false;
        card(ui, |ui| {
            egui::CollapsingHeader::new(RichText::new("Advanced").color(MUTED))
                .default_open(false)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Microphone gain");
                        ui.add(
                            egui::Slider::new(&mut self.settings.input_gain_db, -12.0..=12.0)
                                .step_by(0.5)
                                .suffix(" dB"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Buffer");
                        let current = BUFFERS
                            .iter()
                            .find(|(ms, _)| *ms == self.settings.buffer_ms)
                            .map(|(_, l)| l.to_string())
                            .unwrap_or_else(|| format!("{} ms", self.settings.buffer_ms));
                        egui::ComboBox::from_id_salt("buffer")
                            .selected_text(current)
                            .show_ui(ui, |ui| {
                                for (ms, label) in BUFFERS {
                                    ui.selectable_value(&mut self.settings.buffer_ms, ms, label);
                                }
                            });
                    });

                    ui.checkbox(
                        &mut self.settings.monitor_enabled,
                        "Hear the cleaned audio (monitor)",
                    );
                    if self.settings.monitor_enabled {
                        device_combo(
                            ui,
                            "monitor",
                            &mut self.settings.monitor_device,
                            &self.devices.outputs,
                            "Choose headphones...",
                        );
                    }

                    ui.separator();
                    ui.checkbox(
                        &mut self.settings.close_to_tray,
                        "Keep running in the tray when closed",
                    );
                    ui.checkbox(
                        &mut self.settings.start_minimized,
                        "Start minimized to the tray",
                    );
                    ui.checkbox(
                        &mut self.settings.hotkey_enabled,
                        format!("Global shortcut {} toggles noise cancellation", hotkey::LABEL),
                    );
                    ui.checkbox(
                        &mut self.settings.auto_restart,
                        "Recover automatically when a device disconnects",
                    );
                    let mut auto = self.settings.autostart;
                    if ui.checkbox(&mut auto, "Start with Windows").changed() {
                        match platform::set_autostart(APP_NAME, auto) {
                            Ok(()) => {
                                self.settings.autostart = auto;
                                self.notice = None;
                            }
                            Err(e) => {
                                self.notice = Some(format!("Could not change autostart: {e}"));
                            }
                        }
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Restart audio").clicked() {
                            restart = true;
                        }
                        if ui.button("Open log folder").clicked() {
                            open_logs = true;
                        }
                    });
                });
        });
        if restart {
            self.refresh_devices();
            self.restart_attempts = 0;
            self.start_engine();
        }
        if open_logs {
            if let Some(dir) = Settings::config_dir() {
                let _ = std::fs::create_dir_all(&dir);
                platform::open_url(&dir.to_string_lossy());
            }
        }
    }

    fn footer(&mut self, ui: &mut Ui) {
        let s = &self.snap;
        let line1 = format!(
            "CPU {:.1}% of one core  |  {:.1} ms per 10 ms frame (peak {:.1})",
            s.cpu_pct,
            s.proc_avg_us / 1000.0,
            s.proc_max_us / 1000.0
        );
        let line2 = match &self.state {
            EngineState::Running(info) => format!(
                "Latency ~{:.0} ms  |  dropouts {}/{}  |  {} Hz -> 48k -> {} Hz  |  {}{}",
                s.latency_ms,
                s.input_overruns,
                s.output_underruns,
                info.input_rate,
                info.output_rate,
                MODEL_NAME,
                if info.realtime_priority { "  |  RT" } else { "" }
            ),
            _ => format!("{MODEL_NAME}  |  engine not running"),
        };
        ui.add_space(2.0);
        ui.label(RichText::new(line1).size(10.5).monospace().color(MUTED));
        ui.label(RichText::new(line2).size(10.5).monospace().color(MUTED));
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_engine();

        let msgs = self.tray.as_ref().map(|t| t.poll()).unwrap_or_default();
        for m in msgs {
            match m {
                TrayMsg::Show => self.show_window(ctx),
                TrayMsg::Toggle => self.settings.enabled = !self.settings.enabled,
                TrayMsg::Quit => {
                    self.quit = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
        if self.hotkeys.as_ref().is_some_and(|h| h.poll_toggle()) {
            self.settings.enabled = !self.settings.enabled;
        }
        if self.show_rx.try_iter().count() > 0 {
            self.show_window(ctx);
        }
        self.poll_devices();
        self.poll_driver_job();

        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if close_requested && !self.quit && self.settings.close_to_tray && self.tray.is_some() {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            self.hidden = true;
        }

        self.apply_changes(ctx);
        self.maybe_save(false);

        if !self.hidden {
            if self.devices_refreshed.elapsed() > Duration::from_secs(10) {
                self.refresh_devices();
            }
            let running = matches!(self.state, EngineState::Running(_));
            ctx.request_repaint_after(Duration::from_millis(if running { 50 } else { 250 }));
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        egui::Frame::NONE
            .fill(BG)
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.draw(ui);
                    });
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.dirty_since.get_or_insert_with(Instant::now);
        self.maybe_save(true);
        if let Some(engine) = self.engine.take() {
            // give the audio thread a moment to close its streams cleanly
            engine.stop(Duration::from_millis(1500));
        }
    }
}

// ---------------------------------------------------------------------- widgets

fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::NONE
        .fill(PANEL)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn section_title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(11.0).strong().color(MUTED));
}

fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(3)).collect();
        format!("{head}...")
    }
}

fn dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// iOS-style on/off switch.
fn toggle_switch(ui: &mut Ui, on: &mut bool) -> egui::Response {
    let size = Vec2::new(56.0, 30.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool(response.id, *on);
        let radius = rect.height() / 2.0;
        ui.painter()
            .rect_filled(rect, radius, mix(Color32::from_rgb(58, 63, 76), ACCENT, t));
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        ui.painter()
            .circle_filled(egui::pos2(x, rect.center().y), radius - 3.0, Color32::WHITE);
    }
    response.on_hover_text("Toggle noise cancellation")
}

fn smooth_meter(current: f32, target: f32) -> f32 {
    let k = if target > current { 0.6 } else { 0.12 };
    current + (target - current) * k
}

fn level_meter(ui: &mut Ui, label: &str, db: f32, color: Color32) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(12.0).color(MUTED));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(format!("{db:>5.0} dB"))
                    .size(11.0)
                    .monospace()
                    .color(MUTED),
            );
        });
    });
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 8.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, PANEL_2);
    let frac = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    if frac > 0.0 {
        let mut fill = rect;
        fill.set_width((rect.width() * frac).max(8.0));
        let c = if db > -3.0 {
            DANGER
        } else if db > -10.0 {
            WARN
        } else {
            color
        };
        painter.rect_filled(fill, 4.0, c);
    }
}

fn device_combo(
    ui: &mut Ui,
    id: &str,
    current: &mut Option<String>,
    devices: &[DeviceInfo],
    default_label: &str,
) {
    let selected = current.clone().unwrap_or_else(|| default_label.to_string());
    let missing = current
        .as_ref()
        .is_some_and(|c| !devices.iter().any(|d| &d.name == c));
    let text = if missing {
        format!("{} (disconnected)", elide(&selected, 36))
    } else {
        elide(&selected, 52)
    };
    egui::ComboBox::from_id_salt(id)
        .width(ui.available_width())
        .selected_text(text)
        .show_ui(ui, |ui| {
            ui.selectable_value(current, None, default_label);
            for d in devices {
                let mut label = d.name.clone();
                if d.is_default {
                    label.push_str("  (default)");
                }
                if d.kind == DeviceKind::VirtualCable {
                    label.push_str("  (virtual)");
                }
                ui.selectable_value(current, Some(d.name.clone()), label);
            }
        });
}
