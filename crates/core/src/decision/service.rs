//! Owns the embedded model on a dedicated thread: installs it on first use,
//! loads it, shadows pending decisions, and unloads it when local decisions
//! are turned off. Nothing here runs, and no GPU is probed, while disabled.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use rverdict_core::{Calibration, Request};
use rverdict_engine::install::{install, installed, InstallStep};
use rverdict_engine::{BackendChoice, Checkpoint, Engine, ModelRef, Precision, SelectedBackend};
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::questions::QUESTIONS_VERSION;
use super::worker::{shadow_batch, SHADOW_BATCH_RANGE};
use super::{
    Answered, DecisionModel, Evaluation, ModelConfig, ModelInfo, StartupMode, VerdictSettings,
    DEFAULT_MODEL,
};
use crate::db::Database;

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
/// How long a session stays on the CPU after falling back before the GPU is
/// offered again. A wedged driver usually clears on its own or with a reboot;
/// without this, faults at startup would keep the session on the CPU for days
/// at a fraction of the throughput.
const CPU_FALLBACK_RETRY: Duration = Duration::from_secs(30 * 60);
/// Shadow work has no deadline, so the loop rests between batches instead of
/// pinning the CPU at full duty. The rest is proportional to the last batch's
/// duration (roughly a third of wall time belongs to inference), bounded so
/// short batches do not crawl and long ones do not stall for ever. A wake —
/// settings change, explicit start — still cuts any wait short.
const BATCH_PACE_MIN: Duration = Duration::from_secs(15);
const BATCH_PACE_MAX: Duration = Duration::from_secs(600);

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
enum StartupAction {
    Proceed,
    Wait(Duration),
}

/// Whether background inference may begin. Deferred startup keeps application
/// launch responsive; manual startup only begins after an explicit request.
fn startup_action(
    mode: StartupMode,
    warmed: bool,
    elapsed: Duration,
    delay: Duration,
) -> StartupAction {
    match mode {
        StartupMode::Immediate => StartupAction::Proceed,
        _ if warmed => StartupAction::Proceed,
        StartupMode::Delayed if elapsed >= delay => StartupAction::Proceed,
        StartupMode::Manual => StartupAction::Wait(Duration::from_secs(60)),
        StartupMode::Delayed => StartupAction::Wait(delay - elapsed),
    }
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

/// The backend a configured or reported device name belongs to. Selection can
/// fall back to the CPU when the requested device is unavailable, so recovery
/// follows the loaded model rather than the requested setting alone.
fn session_backend(name: &str) -> Backend {
    if name.starts_with("cpu (flex)") || name.starts_with("reference") {
        Backend::Cpu
    } else {
        Backend::Gpu
    }
}

/// The backend a setting asks for before selection can substitute the CPU.
fn configured_backend(backend: &str) -> Backend {
    match backend.to_ascii_lowercase().as_str() {
        "cpu" | "reference" => Backend::Cpu,
        _ => Backend::Gpu,
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

/// A passed f16 check. The record means the named backend answered a bounded
/// probe with the named model; it is not transferable to another backend,
///
/// model, or application build.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct F16Qualification {
    pub backend: String,
    pub model: String,
    pub app_version: String,
    pub probe_ms: u64,
}

impl F16Qualification {
    fn path(models_dir: &Path) -> PathBuf {
        models_dir.join("f16-qualification.json")
    }

    fn matches(&self, backend: &str, model: &str) -> bool {
        self.backend == backend
            && self.model == model
            && self.app_version == env!("CARGO_PKG_VERSION")
    }
}

fn read_f16_qualification(models_dir: &Path) -> Option<F16Qualification> {
    let text = std::fs::read_to_string(F16Qualification::path(models_dir)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_f16_qualification(
    models_dir: &Path,
    qualification: &F16Qualification,
) -> std::io::Result<()> {
    let path = F16Qualification::path(models_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(
        &temporary,
        serde_json::to_string(qualification).map_err(std::io::Error::other)?,
    )?;
    std::fs::rename(&temporary, &path)
}

/// A one-question request through the loaded model. It exercises full-model
/// f16 inference without queuing a real decision.
fn f16_probe_request() -> Request {
    let mut questions = Map::new();
    questions.insert(
        "probe".into(),
        json!({
            "type": "noul",
            "instructions": "The message asks for the account username.",
            "criteria": {
                "true": "The message asks for the account username.",
                "false": "The message does not ask for the account username.",
            },
        }),
    );
    Request {
        state: Value::String("Subject: username request".into()),
        model: None,
        questions,
    }
}

fn check_f16(model: &LocalModel) -> Result<Duration, String> {
    let started = Instant::now();
    let evaluation = model.evaluate(&f16_probe_request())?;
    let answer = evaluation
        .answers
        .into_iter()
        .next()
        .ok_or_else(|| "the f16 probe produced no answer".to_owned())?;
    let probabilities = model
        .calibration()
        .distribution(&answer.kind, &answer.logits);
    if probabilities.is_empty() || !probabilities.iter().all(|value| value.is_finite()) {
        return Err("the f16 probe produced an invalid distribution".into());
    }
    Ok(started.elapsed())
}

/// Whether a service request asks the worker to begin work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceRequest {
    Warm,
    QualifyF16,
}

/// The embedded engine behind [`DecisionModel`].
pub struct LocalModel {
    engine: Engine,
    info: ModelInfo,
}

impl LocalModel {
    pub fn load(models_dir: &Path, settings: &VerdictSettings) -> Result<Self, String> {
        Self::load_with(models_dir, settings, PrecisionPolicy::Configured)
    }

    /// Loads f16 even when no passing qualification is recorded. Callers must
    /// only use this for an explicit, user-initiated check.
    pub(crate) fn load_for_qualification(
        models_dir: &Path,
        settings: &VerdictSettings,
    ) -> Result<Self, String> {
        Self::load_with(models_dir, settings, PrecisionPolicy::ForceF16)
    }

    fn load_with(
        models_dir: &Path,
        settings: &VerdictSettings,
        policy: PrecisionPolicy,
    ) -> Result<Self, String> {
        let model = ModelRef::parse(DEFAULT_MODEL);
        let checkpoint = installed(models_dir, &model).ok_or("the model is not installed")?;
        let choice: BackendChoice = settings.backend.parse().map_err(|e: String| e)?;
        let selected = rverdict_engine::select(choice);
        let model_id = model_id(
            &checkpoint.name,
            model.revision.as_deref().unwrap_or_default(),
        );
        let precision = policy.resolve(
            &settings.precision,
            &selected.name,
            &model_id,
            &read_f16_qualification(models_dir),
        );
        let backend = selected.name.clone();
        let engine = Self::load_engine(&checkpoint, selected, precision, settings.max_state_tokens)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            engine,
            info: ModelInfo {
                id: model_id,
                backend,
                precision: match precision {
                    Precision::F16 => "f16".into(),
                    Precision::F32 => "f32".into(),
                },
            },
        })
    }

    fn load_engine(
        checkpoint: &Checkpoint,
        selected: SelectedBackend,
        precision: Precision,
        max_state_tokens: u32,
    ) -> Result<Engine, rverdict_engine::EngineError> {
        let mut engine = Engine::load(checkpoint, selected, precision)?;
        engine.set_max_state_tokens(max_state_tokens as usize);
        Ok(engine)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrecisionPolicy {
    Configured,
    ForceF16,
}

impl PrecisionPolicy {
    fn resolve(
        self,
        requested: &str,
        backend: &str,
        model: &str,
        qualification: &Option<F16Qualification>,
    ) -> Precision {
        let force = self == Self::ForceF16;
        if requested == "f16"
            && (force
                || qualification
                    .as_ref()
                    .is_some_and(|qualification| qualification.matches(backend, model)))
        {
            Precision::F16
        } else {
            Precision::F32
        }
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
    /// `off`, `scheduled`, `paused`, `qualifying`, `downloading`, `converting`,
    /// `loading`, `running`, `idle` or `error`.
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
    /// A passed f16 check for the installed backend and model. Absence means
    /// an f16 request runs as f32 until the check passes.
    pub f16_qualification: Option<F16Qualification>,
}

type Notify = Box<dyn Fn(&VerdictStatus) + Send>;

#[derive(Clone)]
pub struct VerdictService {
    status: Arc<Mutex<VerdictStatus>>,
    wake: Arc<(Mutex<bool>, Condvar)>,
    requests: Arc<Mutex<Option<ServiceRequest>>>,
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
            requests: Arc::new(Mutex::new(None)),
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

    /// Starts deferred or manual shadow work without changing settings.
    pub fn request_start(&self) {
        self.request(ServiceRequest::Warm);
    }

    /// Runs one explicit f16 check without changing the requested precision.
    pub fn request_f16_qualification(&self) {
        self.request(ServiceRequest::QualifyF16);
    }

    fn request(&self, request: ServiceRequest) {
        if let Ok(mut pending) = self.requests.lock() {
            *pending = Some(request);
        }
        self.wake();
    }

    fn take_request(&self) -> Option<ServiceRequest> {
        self.requests.lock().ok()?.take()
    }

    fn clear_requests(&self) {
        if let Ok(mut pending) = self.requests.lock() {
            *pending = None;
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

    /// Records a failed load or explicit check with the same bounded recovery
    /// in both paths, so neither can spin on one backend.
    fn fail_load(
        &self,
        backend: &mut Backend,
        attempts: &mut u32,
        cpu_since: &mut Option<Instant>,
        notify: &Notify,
        detail: String,
    ) {
        *attempts += 1;
        match recover(*backend, *attempts) {
            Recovery::Retry => self.fail(notify, format!("{detail}; retrying")),
            Recovery::Cpu => {
                *backend = Backend::Cpu;
                *attempts = 0;
                *cpu_since = Some(Instant::now());
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
    }

    fn run(&self, db: &Database, settings: &dyn Fn() -> VerdictSettings, notify: &Notify) {
        let mut loaded: Option<(LocalModel, ModelConfig)> = None;
        let mut backend = Backend::Gpu;
        let mut attempts = 0;
        let mut last_settings: Option<VerdictSettings> = None;
        let mut startup_mode: Option<StartupMode> = None;
        let mut startup_ready_at: Option<Instant> = None;
        let mut warmed = false;
        let mut qualify = false;
        let mut cpu_since: Option<Instant> = None;
        let startup_delay = Duration::from_secs(120);
        loop {
            let current = settings();
            if last_settings.as_ref() != Some(&current) {
                if last_settings.is_some() {
                    loaded = None;
                    backend = configured_backend(&current.backend);
                    attempts = 0;
                    startup_mode = None;
                    startup_ready_at = None;
                    warmed = false;
                    qualify = false;
                    self.clear_requests();
                }
                last_settings = Some(current.clone());
            }
            if !current.enabled {
                if loaded.take().is_some() {
                    tracing::info!("Local decisions turned off; model unloaded");
                }
                backend = Backend::Gpu;
                attempts = 0;
                startup_mode = None;
                startup_ready_at = None;
                warmed = false;
                qualify = false;
                self.clear_requests();
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
            if let Some(request) = self.take_request() {
                warmed = true;
                if request == ServiceRequest::QualifyF16 {
                    qualify = true;
                }
            }
            if current.shadow_paused {
                if loaded.take().is_some() {
                    tracing::info!("Local decisions paused; model unloaded");
                }
                // A paused worker must not accumulate an f16 check for later. An
                // ordinary start can safely wait until shadowing resumes.
                qualify = false;
                self.update(notify, |s| {
                    s.state = "paused".into();
                    s.detail = Some("Shadow work is paused. Resume it to load the model.".into());
                    s.download = None;
                });
                self.wait(Duration::from_secs(60));
                continue;
            }
            if startup_mode != Some(current.startup) {
                startup_mode = Some(current.startup);
                startup_ready_at = None;
                warmed = false;
            }
            let started_at = *startup_ready_at.get_or_insert_with(Instant::now);
            match startup_action(current.startup, warmed, started_at.elapsed(), startup_delay) {
                StartupAction::Proceed => {}
                StartupAction::Wait(wait) => {
                    if loaded.take().is_some() {
                        tracing::info!("Local decisions waiting to start; model unloaded");
                    }
                    let (state, detail) = if current.startup == StartupMode::Manual {
                        (
                            "manual",
                            "Local shadowing is waiting for an explicit start.".to_owned(),
                        )
                    } else {
                        (
                            "scheduled",
                            "Shadow work starts shortly after startup to keep launch responsive."
                                .to_owned(),
                        )
                    };
                    self.update(notify, |s| {
                        s.state = state.into();
                        s.detail = Some(detail.clone());
                        s.download = None;
                    });
                    self.wait(wait);
                    continue;
                }
            }
            // A CPU fallback after GPU faults is deliberately temporary: the
            // fault usually clears on its own or with a reboot, so the GPU is
            // offered again a while later instead of staying off for the
            // whole session.
            if backend == Backend::Cpu
                && configured_backend(&current.backend) == Backend::Gpu
                && cpu_since.is_some_and(|since| since.elapsed() >= CPU_FALLBACK_RETRY)
            {
                loaded = None;
                backend = configured_backend(&current.backend);
                attempts = 0;
                cpu_since = None;
            }
            if loaded.is_none() {
                // An explicit check replaces whatever is loaded, including the
                // f32 fallback that an unqualified f16 request would otherwise use.
                if qualify && current.precision == "f16" {
                    loaded = None;
                    backend = configured_backend(&current.backend);
                    qualify = false;
                    match self.qualify_f16(&current, settings, notify) {
                        Ok((model, qualification)) => {
                            backend = session_backend(&model.info().backend);
                            self.update(notify, |s| {
                                s.f16_qualification = Some(qualification);
                            });
                            loaded = Some((model, current.model_config()));
                        }
                        Err(detail) => {
                            self.fail_load(&mut backend, &mut attempts, &mut cpu_since, notify, detail);
                            continue;
                        }
                    }
                } else {
                    qualify = false;
                    match self.prepare(&current, settings, notify, backend) {
                        Ok((model, qualification)) => {
                            backend = session_backend(&model.info().backend);
                            self.update(notify, |s| {
                                s.f16_qualification = qualification;
                            });
                            loaded = Some((model, current.model_config()));
                        }
                        Err(detail) => {
                            self.fail_load(&mut backend, &mut attempts, &mut cpu_since, notify, detail);
                            continue;
                        }
                    }
                }
            }
            let Some((model, _)) = loaded.as_ref() else {
                continue;
            };

            let batch = current
                .shadow_batch_size
                .clamp(*SHADOW_BATCH_RANGE.start(), *SHADOW_BATCH_RANGE.end());
            let batch_started = Instant::now();
            let outcome = catch_unwind(AssertUnwindSafe(|| shadow_batch(db, model, batch)));
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
                    } else {
                        self.wait(
                            (batch_started.elapsed() * 2)
                                .max(BATCH_PACE_MIN)
                                .min(BATCH_PACE_MAX),
                        );
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
                            cpu_since = Some(Instant::now());
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
    ) -> Result<(LocalModel, Option<F16Qualification>), String> {
        self.ensure_installed(settings, notify)?;
        self.update(notify, |s| {
            s.state = "loading".into();
            s.download = None;
            s.detail = None;
        });
        let wanted = if backend == Backend::Cpu {
            VerdictSettings {
                backend: "cpu".into(),
                ..current.clone()
            }
        } else {
            current.clone()
        };
        let model = catch_unwind(AssertUnwindSafe(|| {
            LocalModel::load(&self.models_dir, &wanted)
        }))
        .map_err(|_| "loading the model panicked".to_owned())?
        .map_err(|e| format!("loading the model: {e}"))?;
        let qualification = (current.precision == "f16")
            .then(|| read_f16_qualification(&self.models_dir))
            .flatten()
            .filter(|qualification| qualification.matches(&model.info().backend, &model.info().id));
        Ok((model, qualification))
    }

    /// Loads f16 for an explicit check, probes it, and records a pass. This is
    /// the only path that runs an unqualified f16 model.
    fn qualify_f16(
        &self,
        current: &VerdictSettings,
        settings: &dyn Fn() -> VerdictSettings,
        notify: &Notify,
    ) -> Result<(LocalModel, F16Qualification), String> {
        self.ensure_installed(settings, notify)?;
        self.update(notify, |s| {
            s.state = "qualifying".into();
            s.detail = Some(
                "Loading f16 and running one bounded probe. Keep the app open; this can take a few minutes."
                    .into(),
            );
            s.download = None;
        });
        let model = catch_unwind(AssertUnwindSafe(|| {
            LocalModel::load_for_qualification(&self.models_dir, current)
        }))
        .map_err(|_| "loading the f16 model panicked".to_owned())?
        .map_err(|e| format!("loading the f16 model: {e}"))?;
        if session_backend(&model.info().backend) != Backend::Gpu {
            return Err(format!(
                "the f16 check selected {}, not a GPU",
                model.info().backend
            ));
        }
        if settings().precision != "f16" || !settings().enabled {
            return Err("the f16 check was cancelled by a settings change".into());
        }
        let duration = catch_unwind(AssertUnwindSafe(|| check_f16(&model)))
            .map_err(|_| "the f16 probe panicked".to_owned())??;
        let qualification = F16Qualification {
            backend: model.info().backend.clone(),
            model: model.info().id.clone(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            probe_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
        };
        write_f16_qualification(&self.models_dir, &qualification).map_err(|error| {
            format!(
                "recording the f16 check in {}: {error}",
                F16Qualification::path(&self.models_dir).display()
            )
        })?;
        Ok((model, qualification))
    }

    fn ensure_installed(
        &self,
        settings: &dyn Fn() -> VerdictSettings,
        notify: &Notify,
    ) -> Result<ModelRef, String> {
        let model = ModelRef::parse(DEFAULT_MODEL);
        if installed(&self.models_dir, &model).is_some() {
            return Ok(model);
        }
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
        Ok(model)
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

    #[test]
    fn startup_waits_only_until_it_has_been_asked_or_its_delay_passes() {
        let delay = Duration::from_secs(120);
        assert_eq!(
            startup_action(StartupMode::Immediate, false, Duration::ZERO, delay),
            StartupAction::Proceed
        );
        assert_eq!(
            startup_action(StartupMode::Delayed, false, Duration::from_secs(119), delay),
            StartupAction::Wait(Duration::from_secs(1))
        );
        assert_eq!(
            startup_action(StartupMode::Delayed, false, delay, delay),
            StartupAction::Proceed
        );
        assert_eq!(
            startup_action(StartupMode::Manual, false, delay, delay),
            StartupAction::Wait(Duration::from_secs(60))
        );
        assert_eq!(
            startup_action(StartupMode::Manual, true, Duration::ZERO, delay),
            StartupAction::Proceed
        );
        assert_eq!(
            startup_action(StartupMode::Delayed, true, Duration::ZERO, delay),
            StartupAction::Proceed
        );
    }

    #[test]
    fn f16_runs_only_after_a_matching_check() {
        let qualification = F16Qualification {
            backend: "vulkan (Example 1)".into(),
            model: "von-1.2.0@498ceba3/q2".into(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            probe_ms: 1_000,
        };
        assert_eq!(
            PrecisionPolicy::Configured.resolve(
                "f16",
                &qualification.backend,
                &qualification.model,
                &Some(qualification.clone())
            ),
            Precision::F16
        );
        assert_eq!(
            PrecisionPolicy::Configured.resolve(
                "f16",
                "vulkan (Other 2)",
                &qualification.model,
                &Some(qualification.clone())
            ),
            Precision::F32
        );
        assert_eq!(
            PrecisionPolicy::Configured.resolve(
                "f32",
                &qualification.backend,
                &qualification.model,
                &Some(qualification.clone())
            ),
            Precision::F32
        );
        assert_eq!(
            PrecisionPolicy::ForceF16.resolve("f16", "vulkan (Other 2)", "other", &None),
            Precision::F16
        );
    }

    #[test]
    fn recovery_follows_the_loaded_device_not_just_the_setting() {
        assert_eq!(session_backend("cpu (flex)"), Backend::Cpu);
        assert_eq!(session_backend("reference (ndarray)"), Backend::Cpu);
        assert_eq!(session_backend("vulkan (Example 1)"), Backend::Gpu);
        assert_eq!(configured_backend("cpu"), Backend::Cpu);
        assert_eq!(configured_backend("reference"), Backend::Cpu);
        assert_eq!(configured_backend("auto"), Backend::Gpu);
    }

    #[test]
    fn f16_qualification_survives_a_restart_but_not_a_different_build() {
        let dir = std::env::temp_dir().join(format!(
            "po-f16-qualification-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let qualification = F16Qualification {
            backend: "vulkan (Example 1)".into(),
            model: "von-1.2.0@498ceba3/q2".into(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            probe_ms: 1_000,
        };
        write_f16_qualification(&dir, &qualification).unwrap();
        let loaded = read_f16_qualification(&dir).expect("a written qualification");
        assert!(loaded.matches(&qualification.backend, &qualification.model));
        assert!(!F16Qualification {
            app_version: "0.0.0-test".into(),
            ..qualification.clone()
        }
        .matches(&qualification.backend, &qualification.model));
        std::fs::write(F16Qualification::path(&dir), "not a qualification").unwrap();
        assert_eq!(read_f16_qualification(&dir), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_f16_probe_is_a_valid_one_question_request() {
        let questions = f16_probe_request()
            .parse_questions()
            .expect("a valid qualification probe");
        assert_eq!(questions.len(), 1);
    }
}
