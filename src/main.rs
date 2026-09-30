#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;

use std::fs::OpenOptions;

use clearmic::config::Settings;
use clearmic::{APP_NAME, platform};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let minimized = args.iter().any(|a| a == "--minimized");
    let cli_args: Vec<String> = args
        .iter()
        .filter(|a| *a != "--minimized")
        .cloned()
        .collect();

    if !cli_args.is_empty() {
        platform::attach_parent_console();
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
            .format_timestamp_millis()
            .init();
        if let Err(e) = app::cli::run(&cli_args) {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
        return;
    }

    init_file_logger();
    log::info!("{APP_NAME} {} starting", clearmic::VERSION);
    let Some(_single) = platform::SingleInstance::acquire("ClearMic.SingleInstance") else {
        // A manual second launch brings the running app to the front; autostart stays quiet.
        let shown = !minimized && platform::signal_show_window();
        log::info!("another instance is already running (window shown: {shown}); exiting");
        return;
    };
    platform::set_timer_resolution_1ms();
    if let Err(e) = app::ui::run(minimized) {
        log::error!("ui failed: {e:#}");
        platform::message_box(
            APP_NAME,
            &format!(
                "ClearMic could not open its window: {e}. This usually means the graphics driver does not support OpenGL 2.0. Update the graphics driver, or start ClearMic with --headless to use noise cancellation without a window."
            ),
        );
        std::process::exit(1);
    }
}

fn init_file_logger() {
    let mut builder = env_logger::Builder::new();
    builder
        .filter_level(log::LevelFilter::Info)
        .filter_module("tract", log::LevelFilter::Warn)
        .filter_module("df", log::LevelFilter::Error)
        .filter_module("eframe", log::LevelFilter::Warn)
        .filter_module("egui", log::LevelFilter::Warn)
        .filter_module("winit", log::LevelFilter::Warn)
        .format_timestamp_millis();
    if let Some(path) = Settings::log_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // simple size-based rotation: keep one previous log
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > 5 * 1024 * 1024 {
                let _ = std::fs::rename(&path, path.with_extension("log.1"));
            }
        }
        if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
            builder.target(env_logger::Target::Pipe(Box::new(file)));
        }
    }
    let _ = builder.try_init();
}
