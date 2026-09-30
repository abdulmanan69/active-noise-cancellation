//! Persistent user settings (JSON in the per-user config directory).

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::dsp::ProcParams;

pub const DEFAULT_STRENGTH: u8 = 85;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Microphone name; `None` = system default.
    pub input_device: Option<String>,
    /// Output endpoint the cleaned signal is sent to (normally the virtual cable).
    pub output_device: Option<String>,
    /// Optional second output so the user can hear the result.
    pub monitor_device: Option<String>,
    pub monitor_enabled: bool,
    /// Master switch for noise cancellation (audio still flows when off).
    pub enabled: bool,
    /// 0..=100 - how aggressively noise is removed.
    pub strength: u8,
    pub post_filter: bool,
    pub voice_gate: bool,
    pub high_pass: bool,
    pub input_gain_db: f32,
    /// Target output pre-buffer in milliseconds (trade latency vs. robustness).
    pub buffer_ms: u32,
    pub start_minimized: bool,
    pub close_to_tray: bool,
    pub hotkey_enabled: bool,
    pub autostart: bool,
    pub auto_restart: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            monitor_device: None,
            monitor_enabled: false,
            enabled: true,
            strength: DEFAULT_STRENGTH,
            post_filter: false,
            voice_gate: false,
            high_pass: false,
            input_gain_db: 0.0,
            buffer_ms: 30,
            start_minimized: false,
            close_to_tray: true,
            hotkey_enabled: true,
            autostart: false,
            auto_restart: true,
        }
    }
}

impl Settings {
    pub fn proc_params(&self) -> ProcParams {
        ProcParams {
            enabled: self.enabled,
            strength: self.strength,
            post_filter: self.post_filter,
            voice_gate: self.voice_gate,
            high_pass: self.high_pass,
            input_gain_db: self.input_gain_db,
        }
    }

    pub fn config_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("com", "clearmic", "ClearMic")
            .map(|d| d.config_local_dir().to_path_buf())
    }

    pub fn path() -> Option<PathBuf> {
        Self::config_dir().map(|d| d.join("settings.json"))
    }

    pub fn log_path() -> Option<PathBuf> {
        Self::config_dir().map(|d| d.join("clearmic.log"))
    }

    /// Load settings, falling back to defaults on any error.
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Settings>(&text) {
                Ok(s) => s.sanitized(),
                Err(e) => {
                    log::warn!("settings at {} are invalid ({e}); using defaults", path.display());
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path().context("no config directory available")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn sanitized(mut self) -> Self {
        self.strength = self.strength.min(100);
        self.input_gain_db = self.input_gain_db.clamp(-24.0, 24.0);
        self.buffer_ms = self.buffer_ms.clamp(10, 200);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_json() {
        let mut s = Settings::default();
        s.strength = 42;
        s.input_device = Some("Mic".into());
        let text = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&text).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn unknown_or_missing_fields_use_defaults() {
        let s: Settings = serde_json::from_str(r#"{"strength": 250, "future_field": 1}"#).unwrap();
        assert!(!s.high_pass);
        assert!(s.close_to_tray);
        assert_eq!(s.sanitized().strength, 100);
    }
}
