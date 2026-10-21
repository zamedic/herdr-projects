//! What a task shows of its pull request on Azure DevOps: one line for the task panel (its reviewers' votes and its
//! policies, kept on the task by the pull requests' watch as `prRemote`), and for `details` the pull request as Azure
//! DevOps says it now, with the main pipeline's runs of its merge commit while the task waits for them; what your yes,
//! given there too, still waits for; and what a task held at an approval there waits for.
use super::{host, pr, runs};
use crate::json::*;
use crate::store::repos::folder_of;

/// What the task waits for you to approve in Azure DevOps (a build of its checks, or its main pipeline, held at an
/// approval: land/checks.rs, land/main_pipeline.rs), in words. None: nothing is held there.
pub fn held(t: &Obj) -> Option<String> {
    if t.s("stage") != "merging" { return None; }
    [t.at("checksWait").s("approval"), t.at("pipelineWait").s("approval")].into_iter().find(|s| !s.is_empty()).map(String::from)
}
/// What yes says to such a task: the approval is Azure DevOps' to give, not the board's. Your vote on its pull request
/// was cast with your first yes: what it still waits for from people is said too.
pub fn held_answer(t: &Obj, why: &str) -> String {
    let what = why.strip_prefix("Waiting for your approval in Azure DevOps: ").unwrap_or(why);
    let vote = if t.b("azureVote") { format!(" {}.", approved_words(t.at("azureVote"), &strings(t.at("azureVote").at("waits")), "")) } else { String::new() };
    format!("{} waits for your approval in Azure DevOps ({what}), not for a yes here: approve it there and the board goes on by itself.{vote}", t.s("id"))
}
/// What the task says once your yes was given in Azure DevOps too (land/vote_azure.rs), while the pull request waits:
/// `Approved here and in Azure · waits for 1 more reviewer (policy: Minimum number of reviewers)`. `vote`: the task's
/// `azureVote`; `waits`: what its policies of people still wait for (waits.rs); `pr`: its link, said when nothing is ("": only
/// the vote is said).
pub fn approved_words(vote: &Value, waits: &[String], pr: &str) -> String {
    let given = if vote.b("voted") { "Approved here and in Azure".to_string() } else { format!("Approved here; Azure DevOps did not take your vote ({})", vote.s("said")) };
    if !waits.is_empty() { return format!("{given} · {}", waits.join(" · ")); }
    if pr.is_empty() { given } else { format!("{given} · merges when Azure DevOps' policies pass ({pr})") }
}
/// A reviewer's vote in words.
pub fn vote(v: i64) -> &'static str { match v { 10 => "approved", 5 => "approved with suggestions", -5 => "waiting for the author", -10 => "rejected", _ => "no vote" } }
/// A check's result in words: passed, failed, running.
fn result(c: &Value) -> &'static str {
    let w = crate::land::checks::word(c);
    if crate::flow::git::CHECK_BAD.contains(&w.as_str()) { "failed" } else if crate::flow::git::CHECK_GOOD.contains(&w.as_str()) { "passed" } else { "running" }
}
/// The panel's line: `Reviewers: Ann approved, Bob no vote (required) · Policies: CI build passed · Waits for Bob
/// (policy: Required reviewers)`. Empty when there is nothing to say.
pub fn summary(v: &Value) -> String {
    let reviewers: Vec<String> = v.arr("reviewers").iter().map(|r| format!("{} {}{}", r.s("name"), vote(r.i("vote")), if r.b("required") { " (required)" } else { "" })).collect();
    let policies: Vec<String> = v.arr("statusCheckRollup").iter().map(|c| format!("{} {}", crate::land::checks::name(c), result(c))).collect();
    let mut parts = vec![];
    if !reviewers.is_empty() { parts.push(format!("Reviewers: {}", reviewers.join(", "))); }
    if !policies.is_empty() { parts.push(format!("Policies: {}", policies.join(", "))); }
    // Who still has to vote, by policy; else only that someone does.
    if !v.arr("waits").is_empty() { parts.extend(strings(v.at("waits")).into_iter().map(|w| w.replacen("waits for", "Waits for", 1))); }
    else if v.s("reviewDecision") == "REVIEW_REQUIRED" { parts.push("a review is still required".into()); }
    parts.join(" · ")
}
/// For `details`: its pull request on Azure DevOps (status, draft, reviewers, policies, what its own pipeline says)
/// and the main pipeline's runs.
/// None for a task with no pull request there.
pub fn details(t: &Obj) -> Option<Value> {
    if !pr::is_azure(t.s("pr")) { return None; }
    let root = folder_of(t.s("id"));
    let a = host(&root)?;
    let mut out = obj(json!({ "pr": t.at("pr"), "summary": t.at("prRemote"), "prPipeline": t.at("prPipeline").at("words"), "vote": t.at("azureVote") }));
    match pr::view(&a, &root, t.s("pr"), "statusCheckRollup") {
        Ok(v) => { out.put("status", v.at("state").clone()); out.put("draft", v.at("isDraft").clone()); out.put("reviewDecision", v.at("reviewDecision").clone()); out.put("reviewers", v.at("reviewers").clone());
            out.put("policies", v.arr("statusCheckRollup").iter().map(|c| json!({ "name": crate::land::checks::name(c), "result": result(c), "url": c.at("detailsUrl") })).collect::<Vec<_>>()); out.put("summary", summary(&v)); out.put("waits", v.at("waits").clone());
            if t.b("azureVote") && t.s("stage") == "merging" { out.put("approved", approved_words(t.at("azureVote"), &strings(v.at("waits")), "")); }
            // Open, its own pipeline as it is now (the build validation, a deployment to staging): what the row says.
            if v.s("state") == "OPEN" { let p = super::pr_pipeline::look(&a, &root, v.arr("statusCheckRollup"), t.at("prPipeline")); out.put("prPipeline", p.at("words").clone()); } }
        Err(e) => out.put("unread", e),
    }
    let w = if t.b("pipelineWait") { t.at("pipelineWait") } else { t.at("pipelineFailed") };
    if w.is_object() {
        let list = runs::of_commit(&a, &root, w.s("base"), w.s("commit")).unwrap_or_default();
        let wanted = runs::wanted(&list, &crate::store::settings::lookup::setting_text("merge.pipeline", Some(t)));
        out.put("pipeline", json!({ "commit": w.at("commit"), "base": w.at("base"), "waiting": t.b("pipelineWait"),
            "runs": wanted.iter().map(|r| json!({ "id": r.id, "pipeline": r.pipeline, "status": r.status, "result": r.result, "url": a.build_url(&r.id) })).collect::<Vec<_>>() }));
    }
    Some(Value::Object(out))
}
