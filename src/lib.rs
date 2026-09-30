//! ClearMic - lightweight, enterprise-grade AI noise cancellation.
//!
//! The library exposes the real-time engine so it can be driven from the
//! desktop UI, from the CLI and from integration tests.

pub mod audio;
pub mod config;
pub mod dsp;
pub mod engine;
pub mod platform;
pub mod stats;

pub const APP_NAME: &str = "ClearMic";
pub const APP_ID: &str = "com.clearmic.app";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MODEL_NAME: &str = "DeepFilterNet3";
pub const VB_CABLE_URL: &str = "https://www.vb-cable.com";
/// Attribution VB-Audio asks distributors to show to end users.
pub const VB_CABLE_CREDIT: &str =
    "Virtual microphone: VB-CABLE by VB-Audio (www.vb-cable.com). VB-CABLE is donationware, all participations are welcome.";
