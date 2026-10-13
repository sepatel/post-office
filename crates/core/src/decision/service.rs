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
/// How long to stop trying after a backend has failed repeatedly. Long enough
/// that a fault which clears on its own — the user closing something else that
/// was holding GPU memory — is noticed, without logging every attempt.
const BACKOFF_WAIT: Duration = Duration::from_secs(3600);
/// Consecutive failures allowed on one backend before moving on. A reload
/// demonstrably recovers a GPU that was momentarily short of memory, so the
/// device is asked again rather than written off on the first fault.
const MAX_ATTEMPTS: u32 = 3;

/// What to do after the model fails, by panicking or refusing to load.
///
/// Every attempt costs a full model reload, so the failures are bounded: past
/// that, the same device is not going to do better and the worker moves to the
/// CPU, which is always present. wgpu reports its own errors as fatal, so
/// staying on a GPU that has failed this often only fills the log.
///
/// Failing on the CPU too points at something other than the device, so the
/// worker then waits an hour rather than spinning on it. Long enough to notice
/// a fault that clears on its own, short enough not to fill a log with it.
fn recover(backend: Backend, attempts: u32) -> Recovery {
    if attempts < MAX_ATTEMPTS {
        Recovery::Retry
    } else if backend == Backend::Cpu {
        Recovery::BackOff
    } else {
        Recovery::Cpu
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recovery {
    Retry,
    Cpu,
    BackOff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    Gpu,
    Cpu,
}

impl Backend {
    fn label(self) -> &'static str {
        match self {
            Self::Gpu => "the GPU backend",
            Self::Cpu => "the CPU backend",
        }
    }
}

/// What a panic was complaining about. A GPU fault reaches us as cubecl's
/// `unwrap` on a failed call, which says nothing useful on its own, so at
/// least keep whatever text the panic carried.
fn panic_detail(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
        })
        .unwrap_or_else(|| "unknown panic".into())
}

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
        self.fail_after(notify, detail, ERROR_WAIT)
    }

    fn fail_after(&self, notify: &Notify, detail: String, wait: Duration) {
        tracing::warn!("Local decisions: {detail}");
        self.update(notify, |s| {
            s.state = "error".into();
            s.detail = Some(detail);
            s.download = None;
        });
        self.wait(wait);
    }

    fn run(&self, db: &Database, settings: &dyn Fn() -> VerdictSettings, notify: &Notify) {
        let mut loaded: Option<(LocalModel, VerdictSettings)> = None;
        // Where the session is running, and how many failures it has taken there
        // in a row. Both reset when local decisions are turned off.
        let mut backend = Backend::Gpu;
        let mut attempts = 0;
        loop {
            let current = settings();
            if !current.enabled {
                if loaded.take().is_some() {
                    tracing::info!("Local decisions turned off; model unloaded");
                }
                backend = Backend::Gpu;
                attempts = 0;
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
                backend = Backend::Gpu;
                attempts = 0;
            }
            if loaded.is_none() {
                match self.prepare(&current, settings, notify, backend) {
                    Ok(model) => loaded = Some((model, current.clone())),
                    Err(detail) => {
                        attempts += 1;
                        // A backend that cannot even hold the model is in the
                        // same position as one that faults on it, and is only
                        // worth telling apart to say so.
                        match recover(backend, attempts) {
                            Recovery::Retry => self.fail(notify, format!("{detail}; retrying")),
                            Recovery::Cpu => {
                                backend = Backend::Cpu;
                                attempts = 0;
                                self.fail_after(
                                    notify,
                                    format!("{detail}; continuing on the CPU, which is slower"),
                                    Duration::ZERO,
                                );
                            }
                            Recovery::BackOff => self.fail_after(
                                notify,
                                format!("the CPU backend will not load the model ({detail})"),
                                BACKOFF_WAIT,
                            ),
                        }
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
                    attempts = 0;
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
                    // The device may be unusable now, but a reload clears more
                    // than one fault, so the same backend is asked a few times
                    // before it is given up on.
                    loaded = None;
                    attempts += 1;
                    let detail = panic_detail(&panic);
                    match recover(backend, attempts) {
                        Recovery::Retry => self.fail(
                            notify,
                            format!(
                                "{} failed, reloading ({attempts} of {MAX_ATTEMPTS}): {detail}",
                                backend.label()
                            ),
                        ),
                        Recovery::Cpu => {
                            backend = Backend::Cpu;
                            attempts = 0;
                            self.fail_after(
                                notify,
                                format!(
                                    "the GPU backend failed {MAX_ATTEMPTS} times in a row ({detail}); continuing on the CPU, which is slower"
                                ),
                                Duration::ZERO,
                            );
                        }
                        Recovery::BackOff => self.fail_after(
                            notify,
                            format!("the CPU backend keeps failing ({detail})"),
                            BACKOFF_WAIT,
                        ),
                    }
                }
            }
        }
    }

    /// Installs the model if needed, then loads it. `backend` overrides the
    /// configured one once the session has moved off the GPU.
    fn prepare(
        &self,
        current: &VerdictSettings,
        settings: &dyn Fn() -> VerdictSettings,
        notify: &Notify,
        backend: Backend,
    ) -> Result<LocalModel, String> {
        let wanted = if backend == Backend::Cpu {
            VerdictSettings {
                backend: "cpu".into(),
                ..current.clone()
            }
        } else {
            current.clone()
        };
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
            LocalModel::load(&self.models_dir, &wanted)
        }))
        .map_err(|_| "loading the model panicked".to_owned())?
        .map_err(|e| format!("loading the model: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this guards: a GPU that kept faulting was reloaded and asked
    /// again every five minutes for seventeen hours, so the worker made no
    /// progress and the log filled with the same panic.
    #[test]
    fn a_backend_is_asked_a_few_times_before_it_is_written_off() {
        for attempts in 1..MAX_ATTEMPTS {
            assert_eq!(
                recover(Backend::Gpu, attempts),
                Recovery::Retry,
                "{attempts}"
            );
            assert_eq!(
                recover(Backend::Cpu, attempts),
                Recovery::Retry,
                "{attempts}"
            );
        }
        assert_eq!(recover(Backend::Gpu, MAX_ATTEMPTS), Recovery::Cpu);
        assert_eq!(recover(Backend::Cpu, MAX_ATTEMPTS), Recovery::BackOff);
    }

    /// The attempts only count while they keep failing: one good batch resets
    /// the run, so an occasional fault never accumulates into a demotion.
    #[test]
    fn progress_resets_the_attempt_run() {
        let mut attempts = 0;
        for _ in 0..(MAX_ATTEMPTS - 1) {
            assert_eq!(recover(Backend::Gpu, attempts), Recovery::Retry);
            attempts += 1;
        }
        // A step went through, so the count starts over.
        attempts = 0;
        assert_eq!(recover(Backend::Gpu, attempts), Recovery::Retry);
    }

    /// A panic carrying a `&str` payload is the common shape for a GPU fault,
    /// and it has to reach the user rather than becoming "unknown panic".
    #[test]
    fn the_panic_detail_survives_both_payload_shapes() {
        assert_eq!(panic_detail(&String::from("device lost")), "device lost");
        assert_eq!(panic_detail(&"device lost"), "device lost");
        assert_eq!(panic_detail(&7), "unknown panic");
    }
}
