//! Owns the embedded model on a dedicated thread: installs it on first use,
//! loads it, shadows pending decisions, and unloads it when local decisions
//! are turned off. Nothing here runs, and no GPU is probed, while disabled.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use rverdict_core::{Calibration, Request};
use rverdict_engine::install::{install, installed, InstallStep};
use rverdict_engine::{BackendChoice, Engine, ModelRef, Precision};
use serde::Serialize;

use super::questions::QUESTIONS_VERSION;
use super::worker::shadow_batch;
use super::{Answered, DecisionModel, Evaluation, ModelInfo, VerdictSettings, DEFAULT_MODEL};
use crate::db::Database;

/// Steps shadowed between status updates and settings checks.
const BATCH: usize = 4;
const IDLE_WAIT: Duration = Duration::from_secs(60);
const ERROR_WAIT: Duration = Duration::from_secs(300);

/// The embedded engine behind [`DecisionModel`].
pub struct LocalModel {
    engine: Engine,
    info: ModelInfo,
}

impl LocalModel {
    pub fn load(models_dir: &Path, settings: &VerdictSettings) -> Result<Self, String> {
        let model = ModelRef::parse(DEFAULT_MODEL);
        let checkpoint = installed(models_dir, &model).ok_or("the model is not installed")?;
        let choice: BackendChoice = settings.backend.parse().map_err(|e: String| e)?;
        let selected = rverdict_engine::select(choice);
        let backend = selected.name.clone();
        let precision = if settings.precision == "f16" {
            Precision::F16
        } else {
            Precision::F32
        };
        let mut engine =
            Engine::load(&checkpoint, selected, precision).map_err(|e| e.to_string())?;
        engine.set_max_state_tokens(settings.max_state_tokens as usize);
        let revision = model.revision.as_deref().unwrap_or_default();
        Ok(Self {
            engine,
            info: ModelInfo {
                id: model_id(&checkpoint.name, revision),
                backend,
                precision: settings.precision.clone(),
            },
        })
    }
}

impl DecisionModel for LocalModel {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    fn calibration(&self) -> &Calibration {
        self.engine.calibration()
    }

    fn evaluate(&self, request: &Request) -> Result<Evaluation, String> {
        let null_for_nouls = self.engine.calibration().noul_prior.is_some();
        let evaluated = self
            .engine
            .evaluate(request, null_for_nouls)
            .map_err(|e| e.to_string())?;
        Ok(Evaluation {
            truncated: evaluated.truncation.is_some(),
            answers: evaluated
                .questions
                .into_iter()
                .map(|q| Answered {
                    id: q.id,
                    kind: q.rendered.kind,
                    logits: q.logits,
                })
                .collect(),
        })
    }
}

/// The installed model's own calibration, for reports; the default when it
/// is not installed.
pub fn installed_calibration(models_dir: &Path) -> Calibration {
    installed(models_dir, &ModelRef::parse(DEFAULT_MODEL))
        .map(|c| c.settings.calibration)
        .unwrap_or_default()
}

/// The installed model's id as verdicts record it.
pub fn installed_model_id(models_dir: &Path) -> Option<String> {
    let model = ModelRef::parse(DEFAULT_MODEL);
    let checkpoint = installed(models_dir, &model)?;
    Some(model_id(
        &checkpoint.name,
        model.revision.as_deref().unwrap_or_default(),
    ))
}

/// `von-1.2.0@498ceba3/q2`: the checkpoint and the question format, which
/// together determine a verdict.
fn model_id(name: &str, revision: &str) -> String {
    format!(
        "{name}@{}/q{QUESTIONS_VERSION}",
        &revision[..revision.len().min(8)]
    )
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DownloadProgress {
    pub file: String,
    pub done: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct VerdictStatus {
    /// `off`, `downloading`, `converting`, `loading`, `running`, `idle` or `error`.
    pub state: String,
    pub detail: Option<String>,
    pub model: Option<String>,
    pub backend: Option<String>,
    pub precision: Option<String>,
    pub download: Option<DownloadProgress>,
    /// LLM decisions not yet shadowed, and shadowed, for the loaded model.
    pub pending: Option<i64>,
    pub done: Option<i64>,
    pub processed_this_session: u64,
}

type Notify = Box<dyn Fn(&VerdictStatus) + Send>;

#[derive(Clone)]
pub struct VerdictService {
    status: Arc<Mutex<VerdictStatus>>,
    wake: Arc<(Mutex<bool>, Condvar)>,
    models_dir: PathBuf,
}

impl VerdictService {
    /// Starts the service thread. `data_dir` holds the model (`models/`) and
    /// rverdict's caches (`rverdict/`). `settings` is read before every
    /// batch, so changes take effect without a restart once [`wake`] is
    /// called.
    ///
    /// [`wake`]: VerdictService::wake
    pub fn spawn(
        db: Database,
        data_dir: &Path,
        settings: impl Fn() -> VerdictSettings + Send + 'static,
        notify: impl Fn(&VerdictStatus) + Send + 'static,
    ) -> Self {
        rverdict_core::set_cache_root(data_dir.join("rverdict"));
        let service = Self {
            status: Arc::new(Mutex::new(VerdictStatus {
                state: "off".into(),
                ..VerdictStatus::default()
            })),
            wake: Arc::new((Mutex::new(false), Condvar::new())),
            models_dir: data_dir.join("models"),
        };
        let runner = service.clone();
        let notify: Notify = Box::new(notify);
        std::thread::Builder::new()
            .name("verdict-shadow".into())
            .spawn(move || runner.run(&db, &settings, &notify))
            .expect("spawning the verdict thread");
        service
    }

    pub fn status(&self) -> VerdictStatus {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    /// Re-reads the settings now instead of after the current wait.
    pub fn wake(&self) {
        let (flag, condvar) = &*self.wake;
        if let Ok(mut woken) = flag.lock() {
            *woken = true;
            condvar.notify_all();
        }
    }

    fn wait(&self, timeout: Duration) {
        let (flag, condvar) = &*self.wake;
        let Ok(woken) = flag.lock() else {
            return;
        };
        if let Ok((mut woken, _)) = condvar.wait_timeout_while(woken, timeout, |w| !*w) {
            *woken = false;
        }
    }

    fn update(&self, notify: &Notify, change: impl FnOnce(&mut VerdictStatus)) {
        let snapshot = {
            let Ok(mut status) = self.status.lock() else {
                return;
            };
            change(&mut status);
            status.clone()
        };
        notify(&snapshot);
    }

    fn fail(&self, notify: &Notify, detail: String) {
        tracing::warn!("Local decisions: {detail}");
        self.update(notify, |s| {
            s.state = "error".into();
            s.detail = Some(detail);
            s.download = None;
        });
        self.wait(ERROR_WAIT);
    }

    fn run(&self, db: &Database, settings: &dyn Fn() -> VerdictSettings, notify: &Notify) {
        let mut loaded: Option<(LocalModel, VerdictSettings)> = None;
        loop {
            let current = settings();
            if !current.enabled {
                if loaded.take().is_some() {
                    tracing::info!("Local decisions turned off; model unloaded");
                }
                self.update(notify, |s| {
                    *s = VerdictStatus {
                        state: "off".into(),
                        processed_this_session: s.processed_this_session,
                        ..VerdictStatus::default()
                    };
                });
                self.wait(Duration::from_secs(3600));
                continue;
            }
            if loaded.as_ref().is_some_and(|(_, used)| *used != current) {
                loaded = None;
            }
            if loaded.is_none() {
                match self.prepare(&current, settings, notify) {
                    Ok(model) => loaded = Some((model, current.clone())),
                    Err(detail) => {
                        self.fail(notify, detail);
                        continue;
                    }
                }
            }
            let Some((model, _)) = loaded.as_ref() else {
                continue;
            };

            let outcome = catch_unwind(AssertUnwindSafe(|| shadow_batch(db, model, BATCH)));
            let progress = db
                .with_verdicts(|repo| repo.progress(&model.info().id))
                .ok();
            match outcome {
                Ok(Ok(handled)) => {
                    let info = model.info().clone();
                    self.update(notify, |s| {
                        s.state = if handled == 0 { "idle" } else { "running" }.into();
                        s.detail = None;
                        s.download = None;
                        s.model = Some(info.id);
                        s.backend = Some(info.backend);
                        s.precision = Some(info.precision);
                        s.pending = progress.map(|p| p.0);
                        s.done = progress.map(|p| p.1);
                        s.processed_this_session += handled as u64;
                    });
                    if handled == 0 {
                        self.wait(IDLE_WAIT);
                    }
                }
                Ok(Err(error)) => self.fail(notify, format!("database: {error}")),
                Err(panic) => {
                    // A GPU fault can leave the device unusable; reload it.
                    loaded = None;
                    let detail = panic
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                        .unwrap_or_else(|| "unknown panic".into());
                    self.fail(
                        notify,
                        format!("the model failed and will be reloaded: {detail}"),
                    );
                }
            }
        }
    }

    /// Installs the model if needed, then loads it.
    fn prepare(
        &self,
        current: &VerdictSettings,
        settings: &dyn Fn() -> VerdictSettings,
        notify: &Notify,
    ) -> Result<LocalModel, String> {
        let model = ModelRef::parse(DEFAULT_MODEL);
        if installed(&self.models_dir, &model).is_none() {
            self.update(notify, |s| {
                s.state = "downloading".into();
                s.detail = None;
            });
            let mut last_mb = u64::MAX;
            install(&self.models_dir, &model, |step| {
                match step {
                    InstallStep::Downloading { file, done, total } => {
                        // Report about once per MB.
                        if done >> 20 != last_mb {
                            last_mb = done >> 20;
                            self.update(notify, |s| {
                                s.state = "downloading".into();
                                s.download = Some(DownloadProgress {
                                    file: file.clone(),
                                    done: *done,
                                    total: *total,
                                });
                            });
                        }
                    }
                    InstallStep::Converting | InstallStep::Verifying => self.update(notify, |s| {
                        s.state = "converting".into();
                        s.download = None;
                    }),
                }
                settings().enabled
            })
            .map_err(|e| format!("installing the model: {e}"))?;
        }
        self.update(notify, |s| {
            s.state = "loading".into();
            s.download = None;
            s.detail = None;
        });
        catch_unwind(AssertUnwindSafe(|| {
            LocalModel::load(&self.models_dir, current)
        }))
        .map_err(|_| "loading the model panicked".to_owned())?
        .map_err(|e| format!("loading the model: {e}"))
    }
}
