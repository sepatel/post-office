use super::*;
use crate::db::Database;
use crate::decision::report::RuleReport;
use std::path::Path;

#[test]
fn refit_skips_rules_without_enough_history() {
    let db = Database::open(Path::new(":memory:")).unwrap();
    db.migrate().unwrap();
    let summary = refit(&db, None).unwrap();
    assert_eq!(summary.fitted, 0);
}

#[test]
fn per_rule_params_roundtrip() {
    let params = PerRuleParams::new(0.7, vec![0.1], 50);
    let json = serde_json::to_string(&params).unwrap();
    let back: PerRuleParams = serde_json::from_str(&json).unwrap();
    assert_eq!(back.temperature, 0.7);
    let cal = back.into_calibration(&Calibration::default());
    assert_eq!(cal.scaling.temperature, 0.7);
}

#[allow(dead_code)]
fn _report_type(_: RuleReport) {}
