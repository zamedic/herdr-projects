//! What a pull request on Azure DevOps still waits for from people, in words, one per blocking policy of people not
//! met: `waits for 1 more reviewer (policy: Minimum number of reviewers)`, `waits for Shop Team (policy: Required
//! reviewers)`, with why your own vote does not do (`the creator's own vote does not count`). Read from the policies'
//! evaluations and the pull request's reviewers as az gives them (policies.rs, pr.rs).
use super::policies::{kind, name, Kind};
use crate::json::*;

/// A vote that approves: approved, or approved with suggestions.
fn approves(r: &Value) -> bool { r.i("vote") >= 5 }
fn login(r: &Value) -> String { r.t("uniqueName").to_lowercase() }
/// Names as said: `Ann`, `Ann or Bob`, `Ann, Bob or 2 more`.
fn either(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        2 | 3 => format!("{} or {}", names[..names.len() - 1].join(", "), names[names.len() - 1]),
        n => format!("{} or {} more", names[..2].join(", "), n - 2),
    }
}
/// One policy of people not met, in words. `creator`: who opened the pull request; `me`: who az is logged in as.
fn one(p: &Value, reviewers: &[Value], creator: &str, me: &str) -> String {
    let s = p.at("configuration").at("settings");
    let ty = p.at("configuration").at("type").s("displayName").to_lowercase();
    let policy = name(p);
    // The creator's own vote counts only where the policy lets it (`creatorVoteCounts`).
    let own_counts = s.at("creatorVoteCounts") == &json!(true);
    let is_creator = |r: &Value| !creator.is_empty() && login(r) == creator.to_lowercase();
    let counts = |r: &Value| !r.b("isContainer") && (own_counts || !is_creator(r));
    let mine_void = !own_counts && !me.is_empty() && me.eq_ignore_ascii_case(creator) && reviewers.iter().any(|r| is_creator(r) && approves(r));
    let note = if mine_void { "; the creator's own vote does not count" } else { "" };
    let still = |r: &Value| !approves(r) && counts(r);
    if ty.contains("minimum number of reviewers") {
        let need = if s.b("minimumApproverCount") { s.i("minimumApproverCount") } else { 1 };
        let left = (need - reviewers.iter().filter(|r| approves(r) && counts(r)).count() as i64).max(1);
        let who: Vec<String> = reviewers.iter().filter(|r| still(r)).map(|r| r.t("displayName")).collect();
        let more = format!("{left} more reviewer{}", if left == 1 { "" } else { "s" });
        return format!("waits for {more}{} (policy: {policy}{note})", if who.is_empty() { String::new() } else { format!(": {}", either(&who)) });
    }
    if ty.contains("required reviewers") {
        let ids = strings(s.at("requiredReviewerIds"));
        let named: Vec<&Value> = reviewers.iter().filter(|r| ids.iter().any(|i| i.eq_ignore_ascii_case(r.s("id")))).collect();
        let named = if named.is_empty() { reviewers.iter().filter(|r| r.b("isRequired")).collect() } else { named };
        // A group counts here: it is who the policy names.
        let who: Vec<String> = named.iter().filter(|r| !approves(r) && (own_counts || !is_creator(r))).map(|r| r.t("displayName")).collect();
        return format!("waits for {} (policy: {policy}{note})", if who.is_empty() { "a required reviewer".to_string() } else { either(&who) });
    }
    if ty.contains("comment") { return format!("waits for its comments to be resolved (policy: {policy})"); }
    if ty.contains("work item") { return format!("waits for a linked work item (policy: {policy})"); }
    format!("waits for it to be met (policy: {policy})")
}
/// Each blocking policy of people not met, in words, in the order Azure DevOps lists them.
pub fn of(evaluations: &[Value], reviewers: &[Value], creator: &str, me: &str) -> Vec<String> {
    evaluations.iter()
        .filter(|p| p.at("configuration").at("isEnabled") != &json!(false) && p.at("configuration").at("isBlocking") != &json!(false) && kind(p) == Kind::People)
        .filter(|p| !["approved", "notapplicable"].contains(&p.s("status").to_lowercase().as_str()))
        .map(|p| one(p, reviewers, creator, me)).collect()
}
