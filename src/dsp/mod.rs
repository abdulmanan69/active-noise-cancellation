//! Signal processing: neural denoiser plus light-weight helpers.

pub mod denoiser;
pub mod filters;
pub mod gate;
pub mod pipeline;

pub use denoiser::{Denoiser, HOP_SIZE, SAMPLE_RATE, strength_to_atten_db};
pub use pipeline::{FrameInfo, Pipeline, ProcParams, VOICE_LSNR_DB};
