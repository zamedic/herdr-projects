//! The main pipeline's gate, on Azure DevOps: a pull request merged into the base branch is not done until the
//! pipeline run(s) of that branch for the merge commit have passed (`merge.pipeline`: `all`, pipelines by name or id,
//! or `none`). Meanwhile the task stays at stage "merging" with `pipelineWait`, its row saying how far the runs are,
//! looked at on the pull requests' cadence (housekeeping/prs.rs). Passed: done, and cleaned up. Failed or cancelled:
//! it waits for you with the reason and the end of the failing job's log, `fix` sends that to the agent, whose next
//! finish opens a new pull request; `retry` looks at the runs again. Not started (or queued, no job with an agent yet)
//! after `merge.pipelineTimeout`: it waits for you. A run under way is waited for as long as it runs, and one held at
//! an approval in Azure DevOps (its environment's check) waits for you as it is, with no timeout; one whose approval
//! was rejected (or ran out) there waits for you with that. Work the agent pushed to the base branch itself (`Already
//! on main`) waits for the run of the commit that brought it there the same way (`arm_direct`). Only the runs of that
//! very commit count: no run for it yet is no run, whatever older run of the pipeline passed.
use crate::azure::{self, runs};
use crate::clock::now;
use crate::flow::{git::drop_remote_branch, sidebar::tell_sidebar, spaces::tidy_mark};
use crate::json::*;
use crate::store::failures::{failure_patch, is_auth_failure};
use crate::store::repos::folder_of;
use crate::store::settings::lookup::setting_text;
use crate::store::settings::values::ms;
use crate::store::tasks::{get, update_wait, Change, Task};

/// With `merge.pipeline` at `all` and no run of the merge commit after this long, no pipeline runs for it.
const START_GRACE_MS: i64 = 10 * 60000;
/// Does the gate hold this task's merge? Its repo is at Azure DevOps and `merge.pipeline` is not `none`.
pub fn applies(t: &Obj) -> bool { setting_text("merge.pipeline", Some(t)) != "none" && azure::host(&folder_of(t.s("id"))).is_some() }
/// Merged, waiting for its main pipeline: no runner stands in its stage "merging" meanwhile (hook/lost.rs).
pub fn waits(t: &Obj) -> bool { t.b("pipelineWait") && t.s("stage") == "merging" }
/// The task merged at `commit` into `base`: it waits for the pipeline. `mode`/`head`: how its clean-up deletes its
/// branch, and the commit that merged, for when it is done.
pub fn arm(t: &mut Obj, commit: &str, base: &str, mode: &str, head: &str) {
    t.put("stage", "merging"); t.put("mergedAt", now()); t.put("since", now());
    if !commit.is_empty() { t.put("mergeCommit", commit); }
    for k in ["failure", "prFailure", "mergeIntent", "lineAt", "lineTurn", "checksWait", "pipelineFailed"] { t.put(k, Value::Null); }
    for k in ["autoMerge", "autoMergeLocal", "mergeFailed", "requireApproval"] { t.put(k, false); }
    t.put("prMerged", true); t.put("prState", "merged");
    t.put("pipelineWait", json!({ "commit": commit, "base": base, "since": now(), "mode": mode, "head": head }));
    t.put("msg", format!("{}. Waiting for the main pipeline to start", landed(t)));
}
/// The agent pushed the task's work to `base` itself, `commit` bringing it there: it waits for that commit's runs as
/// after a merge. Its pull request, if it had one, is left as it is: nothing merged it.
pub fn arm_direct(t: &mut Obj, commit: &str, base: &str) {
    let kept: Vec<(&str, Value)> = ["prMerged", "prState"].into_iter().map(|k| (k, t.at(k).clone())).collect();
    arm(t, commit, base, "merged", "");
    for (k, v) in kept { t.put(k, v); }
    t.obj_mut("pipelineWait").put("direct", true);
    t.put("msg", format!("{}. Waiting for the main pipeline to start", landed(t)));
}
/// What came onto the base branch, for the row: the pull request merged (`Merged: <its link>`), or the commit the agent
/// pushed there itself (`On main: bdb73e8`).
fn landed(t: &Obj) -> String {
    let w = t.at("pipelineWait");
    if w.b("direct") { format!("On {}: {}", w.s("base"), w.s("commit").chars().take(7).collect::<String>()) } else { format!("Merged: {}", t.s("pr")) }
}
/// The same in a word, before what the runs say (`Merged. Waiting for the pipeline Release to start`).
fn landed_word(t: &Obj) -> String { let w = t.at("pipelineWait"); if w.b("direct") { format!("On {}", w.s("base")) } else { "Merged".into() } }
/// The row's words. `held`: a run waits for your approval in Azure DevOps, so the task waits for you (state.rs).
fn say_held(id: &str, msg: &str, held: bool) {
    let mark = if held { json!(msg) } else { Value::Null };
    if let Ok(t) = update_wait(id, 10000, |t| {
        if !t.b("pipelineWait") || (t.s("msg") == msg && *t.at("pipelineWait").at("approval") == mark) { return Ok(Change::Keep); }
        // `merge.pipelineTimeout` counts from your yes there: from when the hold ended.
        if !held && t.at("pipelineWait").b("approval") { t.obj_mut("pipelineWait").put("since", now()); }
        t.put("msg", msg); t.obj_mut("pipelineWait").put("approval", mark.clone());
        Ok(Change::Write)
    }) { tell_sidebar(&t); }
}
fn say(id: &str, msg: &str) { say_held(id, msg, false) }
/// It is done: passed, or nothing to wait for. Cleaned up as a merged task is.
fn done(id: &str, words: &str) {
    let (mut mode, mut head) = ("always".to_string(), String::new());
    let Ok(t) = update_wait(id, 10000, |t| {
        if !t.b("pipelineWait") { return Ok(Change::Keep); }
        let w = t.at("pipelineWait").clone();
        mode = if w.s("mode").is_empty() { "always".into() } else { w.t("mode") }; head = w.t("head");
        let m = tidy_mark(t, &mode, &head);
        let said = format!("{}. {words}", landed(t));
        t.put("stage", "done"); t.put("tidy", m.unwrap_or(Value::Null)); t.put("pipelineWait", Value::Null); t.put("msg", said);
        Ok(Change::Write)
    }) else { return };
    if t.s("stage") != "done" { return; }
    tell_sidebar(&t);
    crate::land::cleanup::cleanup_job(id, &mode);
    let t = get(id).unwrap_or(t);
    if mode == "always" { drop_remote_branch(&folder_of(id), t.s("branch"), &head); }
}
/// It waits for you: the runs failed (`agent`: the agent may fix it), or did not finish in time.
fn stopped(id: &str, title: &str, detail: &str, next: &str, agent: bool) {
    let Ok(t) = update_wait(id, 10000, |t| {
        if !t.b("pipelineWait") { return Ok(Change::Keep); }
        let w = t.at("pipelineWait").clone();
        t.put("stage", "failed"); t.put("pipelineWait", Value::Null); t.put("msg", title);
        t.put("pipelineFailed", json!({ "commit": w.at("commit"), "base": w.at("base"), "mode": w.at("mode"), "head": w.at("head"), "pr": t.at("pr"), "direct": w.b("direct") }));
        for (k, v) in failure_patch(&obj(json!({ "stage": "pipeline", "title": title, "detail": detail, "next": next, "agentCanFix": agent, "auth": is_auth_failure(detail) && !agent }))) { t.insert(k, v); }
        Ok(Change::Write)
    }) else { return };
    tell_sidebar(&t);
}
fn overdue(t: &Task) -> bool { let l = ms(&setting_text("merge.pipelineTimeout", Some(t))); l > 0 && now() - t.at("pipelineWait").i("since") >= l }
/// One look at the runs of a task that waits for them.
pub fn look(id: &str) {
    let Some(t) = get(id) else { return };
    let w = t.at("pipelineWait").clone();
    if !w.is_object() || t.s("stage") != "merging" { return; }
    let root = folder_of(id);
    let setting = setting_text("merge.pipeline", Some(&t));
    let Some(a) = azure::host(&root).filter(|_| setting != "none") else { return done(id, "The main pipeline is not waited for (merge.pipeline is none).") };
    let (commit, base) = (w.t("commit"), w.t("base"));
    let timeout = setting_text("merge.pipelineTimeout", Some(&t));
    let all = match runs::of_commit(&a, &root, &base, &commit) {
        Ok(r) => r,
        Err(said) => {
            let why: String = said.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("no answer").chars().take(160).collect();
            if overdue(&t) { return stopped(id, "Could not read the main pipeline", &format!("Merge commit: {commit} on {base}\n\naz pipelines runs list:\n{said}"), &format!("{} Then retry the task to look at its pipeline again.", azure::cli::LOGIN), false); }
            return say(id, &format!("{}. Waiting for the main pipeline · could not read its runs ({why})", landed_word(&t)));
        }
    };
    // No run of the commit: the pull request may have named it before Azure DevOps had made it. Read again, the next look
    // goes by the commit it names now.
    if all.is_empty() && recommit(id, &a, &root, &t, &commit) { return; }
    let wanted = runs::wanted(&all, &setting);
    let missing = runs::not_started(&all, &setting);
    let late = |what: &str| stopped(id, "The main pipeline did not finish", &format!("Merge commit: {commit} on {base}\nNot finished after {timeout} (merge.pipelineTimeout): {what}\n\nThe work is merged; the task is not done until its pipeline passed."),
        "Look at the pipeline in Azure DevOps: it may wait for an agent or an approval. Retry the task to wait for it again.", false);
    match runs::look(&a, &root, &wanted, &missing) {
        runs::Look::Passed(words) => done(id, &format!("Main pipeline passed: {words}")),
        runs::Look::Failed { why, detail } => stopped(id, &format!("Main pipeline failed: {why}"), &format!("{detail}\n\nMerge commit: {commit} on {base}. The work is merged; the task is not done until its pipeline passed."),
            &format!("Fix what failed on your branch (it has {base} with this merge in it), run the checks and finish again: a new pull request then brings the fix in. Or run the pipeline again in Azure DevOps and retry the task."), true),
        runs::Look::NotStarted(names) if names.is_empty() => {
            // `all`, and no run: a repo with no pipeline, or none that runs for this commit (its triggers, its paths).
            if runs::any_pipeline(&a, &root) == Some(false) { return done(id, "No pipeline in this repo to wait for."); }
            if now() - w.i("since") >= START_GRACE_MS { return done(id, &format!("No pipeline ran for the merge commit on {base}.")); }
            say(id, &format!("{}. Waiting for the main pipeline to start", landed(&t)));
        }
        runs::Look::NotStarted(names) => { if overdue(&t) { return late(&format!("{} not started", names.join(", "))); } say(id, &format!("{}. Waiting for the pipeline {} to start", landed_word(&t), names.join(", "))); }
        // A run under way is not cut off: a long build, or one that sat at an approval, is still running. One still
        // queued (no job has had an agent yet) is, past `merge.pipelineTimeout`.
        runs::Look::Running { words, started } => { if !started && overdue(&t) { return late(&format!("{words}, no job started")); } say(id, &words); }
        // Waiting for a person is not the pipeline taking too long: no timeout while it is so.
        runs::Look::Approval(words) => say_held(id, &words, true),
        // Said no to there: not the agent's to fix, nor the machine's to wait for.
        runs::Look::Refused { why, detail } => stopped(id, &format!("Main pipeline stopped: {why}"), &format!("{detail}\n\nCommit: {commit} on {base}. The work is on {base}; the task is not done until its pipeline passed."),
            "If it should go out after all, run the stage again in Azure DevOps and approve it there, then retry the task to wait for it again. If it should not, archive the task.", false),
    }
}
/// The merge commit its pull request names now, when that is another than the one waited for: kept, and true.
fn recommit(id: &str, a: &azure::Azure, root: &std::path::Path, t: &Task, commit: &str) -> bool {
    if t.at("pipelineWait").b("direct") || !azure::pr::is_azure(t.s("pr")) { return false; }
    let Ok(v) = azure::pr::view(a, root, t.s("pr"), "state") else { return false };
    let now_at = v.at("mergeCommit").t("oid");
    if v.s("state") != "MERGED" || now_at.is_empty() || now_at.eq_ignore_ascii_case(commit) { return false; }
    update_wait(id, 10000, |t| {
        if !t.b("pipelineWait") { return Ok(Change::Keep); }
        t.obj_mut("pipelineWait").put("commit", now_at.as_str()); t.put("mergeCommit", now_at.as_str());
        Ok(Change::Write)
    }).is_ok()
}
/// `retry` on a task whose pipeline failed or did not finish: it waits for (and looks at) the runs again.
pub fn rearm(id: &str) -> crate::err::Res<Task> {
    let t = update_wait(id, 10000, |t| {
        let f = t.at("pipelineFailed").clone();
        if !f.is_object() { crate::fail!("{} has no pipeline to look at again.", t.s("id")); }
        if f.b("direct") { arm_direct(t, f.s("commit"), f.s("base")); } else { arm(t, f.s("commit"), f.s("base"), f.s("mode"), f.s("head")); }
        Ok(Change::Write)
    })?;
    look(id);
    Ok(get(id).unwrap_or(t))
}
/// Every task that waits for its main pipeline gets a look (housekeeping/prs.rs, on the pull requests' cadence).
pub fn pass(tasks: &[Task]) {
    for t in tasks { if t.b("pipelineWait") && t.s("stage") == "merging" && !t.b("archivedAt") && !crate::runner::runner_alive(t) { look(t.s("id")); } }
}
