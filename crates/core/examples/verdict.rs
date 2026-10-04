//! Headless shadow-mode tool: runs, reports and exports local decisions
//! against a Post Office database without the app or Gmail.
//!
//! ```sh
//! cargo run --release -p post-office-core --features embedded --example verdict -- \
//!     shadow <app.db> [--models DIR] [--limit N] [--precision f32|f16] [--backend auto]
//! cargo run ... -- report <app.db> [--models DIR]
//! cargo run ... -- export <app.db> <out-dir>
//! ```
//!
//! Use a copy of the database: `shadow` migrates it and writes verdicts.

use std::path::{Path, PathBuf};
use std::time::Instant;

use post_office_core::db::Database;
use post_office_core::decision::report::rule_reports;
use post_office_core::decision::service::{installed_calibration, installed_model_id, LocalModel};
use post_office_core::decision::worker::shadow_batch;
use post_office_core::decision::{export, DecisionModel, VerdictSettings, DEFAULT_MODEL};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(command), Some(db_path)) = (args.first(), args.get(1)) else {
        eprintln!("usage: verdict <shadow|report|export> <app.db> [...]");
        std::process::exit(2);
    };
    let db_path = PathBuf::from(db_path);
    let models = flag(&args, "--models").map_or_else(
        || db_path.parent().unwrap_or(Path::new(".")).join("models"),
        PathBuf::from,
    );
    let db = Database::open(&db_path)?;
    db.migrate()?;

    match command.as_str() {
        "shadow" => shadow(&db, &models, &args),
        "report" => report(&db, &models),
        "export" => {
            let out = args.get(2).ok_or("export needs an output directory")?;
            let model = installed_model_id(&models);
            let summary = export::export(&db, Path::new(out), model.as_deref())?;
            println!(
                "{} decisions ({} labelled by the user), {} captures\n  {}\n  {}",
                summary.decisions,
                summary.from_user,
                summary.captures,
                summary.decisions_path.display(),
                summary.captures_path.display()
            );
            Ok(())
        }
        other => Err(format!("unknown command {other:?}").into()),
    }
}

fn shadow(db: &Database, models: &Path, args: &[String]) -> Result<()> {
    let settings = VerdictSettings {
        enabled: true,
        precision: flag(args, "--precision").unwrap_or_else(|| "f32".into()),
        backend: flag(args, "--backend").unwrap_or_else(|| "auto".into()),
        max_state_tokens: flag(args, "--max-state-tokens").map_or(Ok(2_048), |v| v.parse())?,
    };
    let limit: usize = flag(args, "--limit").map_or(Ok(usize::MAX), |v| v.parse())?;
    rverdict_core::set_cache_root(models.parent().unwrap_or(models).join("rverdict"));
    let reference = rverdict_engine::ModelRef::parse(DEFAULT_MODEL);
    rverdict_engine::install::install(models, &reference, |_| true)?;
    let model = LocalModel::load(models, &settings)?;
    let info = model.info().clone();
    eprintln!("{} on {} ({})", info.id, info.backend, info.precision);

    let started = Instant::now();
    let mut handled = 0;
    while handled < limit {
        let n = shadow_batch(db, &model, 8.min(limit - handled))?;
        if n == 0 {
            break;
        }
        handled += n;
        let (pending, done) = db.with_verdicts(|repo| repo.progress(&info.id))?;
        let rate = handled as f64 / started.elapsed().as_secs_f64();
        eprintln!("{done} shadowed, {pending} pending ({rate:.1} steps/s)");
    }
    eprintln!(
        "shadowed {handled} steps in {:.0} s",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn report(db: &Database, models: &Path) -> Result<()> {
    let model = installed_model_id(models);
    let reports = rule_reports(db, model.as_deref(), &installed_calibration(models))?;
    let pct = |v: Option<f64>| v.map_or_else(|| "-".to_owned(), |v| format!("{:.0}%", v * 100.0));
    println!(
        "{:<28} {:<7} {:>5} {:>6} {:>8} {:>9} {:>9} {:>10} {:>11} {:>6}",
        "rule",
        "framing",
        "n",
        "agree",
        "llm-maj",
        "minority",
        "conf≥0.9",
        "conf-wrong",
        "nll b→a",
        "ms"
    );
    for r in &reports {
        let nll = r.calibration.as_ref().map_or_else(
            || "-".to_owned(),
            |c| format!("{:.2}→{:.2}", c.nll_before, c.nll_after),
        );
        println!(
            "{:<28} {:<7} {:>5} {:>6} {:>8} {:>4} of {:<3} {:>9} {:>4} of {:<4} {:>11} {:>6}",
            format!(
                "{} ({})",
                truncate(&r.rule_name, 18),
                truncate(&r.account_email, 3)
            ),
            r.framing,
            r.decisions,
            pct(r.agreement),
            pct(r.llm_majority_share),
            pct(r.minority_agreement),
            r.minority_decisions,
            pct(r.confident_share),
            r.confident_disagreements,
            r.confident,
            nll,
            r.median_ms.map_or_else(|| "-".into(), |m| m.to_string()),
        );
        if r.skipped + r.errors > 0 {
            println!("{:<28} skipped {}, errors {}", "", r.skipped, r.errors);
        }
    }
    Ok(())
}

fn truncate(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}
