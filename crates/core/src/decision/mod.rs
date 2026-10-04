//! Local decisions with rverdict, run in shadow beside the LLM.
//!
//! Nothing here touches the live pipeline. A background worker finds steps
//! the LLM has decided (live and historical), asks rverdict the same rule
//! about the same email, and records both answers so agreement and
//! calibration can be reported per rule. It runs only when enabled in
//! settings; off, no model is downloaded or loaded and no GPU is touched.

pub mod export;
pub mod feedback;
pub mod questions;
pub mod report;
#[cfg(feature = "embedded")]
pub mod service;
pub mod worker;

pub use rverdict_core::Calibration;
use rverdict_core::{Logits, RenderedKind, Request};
use serde::{Deserialize, Serialize};

/// The default checkpoint: Von 1.2, pinned.
pub const DEFAULT_MODEL: &str = "von";

/// User settings for local decisions, stored with the app config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerdictSettings {
    pub enabled: bool,
    /// `f32` or `f16`. f16 is about 3× faster on GPUs, but stays opt-in until
    /// the machine's GPU has passed rverdict's backend checklist in f16.
    pub precision: String,
    /// `auto`, `cpu`, `wgpu`, `cuda` or `rocm`.
    pub backend: String,
    /// Longer emails keep their start and end; attention cost grows with
    /// the square of the length.
    pub max_state_tokens: u32,
}

impl Default for VerdictSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            precision: "f32".into(),
            backend: "auto".into(),
            max_state_tokens: 2_048,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    /// Recorded with every verdict, e.g. `von-1.2.0@498ceba3`.
    pub id: String,
    pub backend: String,
    pub precision: String,
}

/// One question's raw output.
#[derive(Debug, Clone)]
pub struct Answered {
    pub id: String,
    pub kind: RenderedKind,
    pub logits: Logits,
}

#[derive(Debug, Clone)]
pub struct Evaluation {
    pub answers: Vec<Answered>,
    pub truncated: bool,
}

/// A decision model the shadow worker can ask: the embedded engine, or a
/// stub in tests.
pub trait DecisionModel {
    fn info(&self) -> &ModelInfo;
    fn calibration(&self) -> &Calibration;
    fn evaluate(&self, request: &Request) -> Result<Evaluation, String>;
}
