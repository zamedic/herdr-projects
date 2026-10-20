//! The pull request's own pipeline on Azure DevOps while it is open: its build validations (a build, its tests, a
//! deployment to staging), in the row's words: `Pull request open · pipeline running`, `… · deploying Staging`,
//! `… · pipeline passed`, `… · deployed to Staging`, `… · pipeline failed`. Kept on the task as `prPipeline` by the
//! pull requests' watch (housekeeping/prs.rs), so you know when the work can be tried on staging before your yes.
use super::logs::timeline;
use super::runs::{deploy_target, stages_of, Stage};
use super::url::Azure;
use crate::flow::git::{CHECK_BAD, CHECK_GOOD};
use crate::json::*;
use std::path::Path;

/// The checks that are builds of a pipeline (policies.rs gives them their build's id).
fn builds(checks: &[Value]) -> impl Iterator<Item = &Value> { checks.iter().filter(|c| c.b("buildId")) }
/// Where its builds are, all together: `failed`, `running`, `passed`, or empty with no build at all.
pub fn state_of(checks: &[Value]) -> &'static str {
    let words: Vec<String> = builds(checks).map(crate::land::checks::word).collect();
    if words.is_empty() { "" } else if words.iter().any(|w| CHECK_BAD.contains(&w.as_str())) { "failed" } else if words.iter().all(|w| CHECK_GOOD.contains(&w.as_str())) { "passed" } else { "running" }
}
/// The row's words, of that state and the stages of its builds.
pub fn words(state: &str, stages: &[Stage]) -> String {
    let named = |s: &Stage| deploy_target(&s.name).filter(|to| !to.is_empty()).unwrap_or_else(|| s.name.replace('_', " "));
    let deploying = stages.iter().find(|s| s.running && (s.deploying || deploy_target(&s.name).is_some()));
    let deployed: Vec<String> = stages.iter().filter(|s| s.deployed).map(named).collect();
    let said = match state {
        "failed" => "pipeline failed".to_string(),
        "running" => match deploying {
            Some(s) if deploy_target(&s.name).map(|to| to.is_empty()).unwrap_or(false) => "deploying".into(),
            Some(s) => format!("deploying {}", named(s)),
            None if stages.is_empty() => "pipeline running".into(),
            None => format!("pipeline running · {} of {} stages", stages.iter().filter(|s| s.done).count(), stages.len()),
        },
        "passed" if !deployed.is_empty() => format!("deployed to {}", deployed.join(", ")),
        "passed" => "pipeline passed".into(),
        _ => String::new(),
    };
    if said.is_empty() { "Pull request open".into() } else { format!("Pull request open · {said}") }
}
/// What its pipeline says now: its builds' timelines are read while one runs (how far it is), and once when they have
/// passed (what they deployed); `was`, the last look, is kept while nothing changed since.
pub fn look(a: &Azure, root: &Path, checks: &[Value], was: &Value) -> Value {
    let state = state_of(checks);
    let key = builds(checks).map(|c| format!("{}:{}", c.t("buildId"), crate::land::checks::word(c))).collect::<Vec<_>>().join(",");
    if state != "running" && was.s("key") == key && was.b("words") { return was.clone(); }
    let stages: Vec<Stage> = if state == "running" || state == "passed" { builds(checks).flat_map(|c| stages_of(&timeline(a, root, &c.t("buildId")))).collect() } else { vec![] };
    json!({ "state": state, "words": words(state, &stages), "key": key, "builds": builds(checks).map(|c| json!({ "id": c.t("buildId"), "name": crate::land::checks::name(c), "url": c.at("detailsUrl") })).collect::<Vec<_>>() })
}
