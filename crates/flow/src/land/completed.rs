//! A pull request on Azure DevOps that was completed there already (by its auto-complete, by you) is merged work: the
//! land step and the sync with main take it for merged before they merge main into its branch or compare its head with
//! the reviewed one (its merge commit is the pull request's `lastMergeCommit`, then the main pipeline's wait as
//! after the board's own completion). A task left waiting for you after a land step ran again on such merged work
//! (its main pipeline's wait taken for a lost runner, hook/lost.rs) is finished as merged on the next poll: the board's
//! own merge of main on top of what merged is taken back, and the wait for its main pipeline goes on.
use super::main_pipeline;
use crate::azure::pr;
use crate::clock::now;
use crate::err::Res;
use crate::flow::{git::dirty, sidebar::tell_sidebar, spaces::tidy_mark};
use crate::json::*;
use crate::runner::{runner_alive, Guard};
use crate::store::repos::{branch_of, folder_of, git_out};
use crate::store::tasks::{update_wait, Change, Task};
use std::path::Path;

/// Where the task's own branch stands here ("" when it has none).
fn tip_of(root: &Path, t: &Obj) -> String {
    if !t.b("branch") { return String::new(); }
    git_out(root, &["rev-parse", "--verify", "-q", &format!("refs/heads/{}", t.s("branch"))]).unwrap_or_default()
}
/// How its clean-up deletes the branch: `always` when what merged (`head`) has everything the branch holds here, else
/// only once git calls it merged (a commit made here since landed nowhere).
fn mode_of(root: &Path, tip: &str, head: &str) -> &'static str {
    let covered = tip.is_empty() || tip == head || (!head.is_empty() && git_out(root, &["merge-base", "--is-ancestor", tip, head]).is_ok());
    if covered { "always" } else { "merged" }
}
/// Its pull request is completed on Azure DevOps: it is taken for merged here, and true. False: it is not (still
/// active, not at Azure DevOps, or az could not say), and the caller goes on. A task whose main pipeline is waited for
/// already has merged: true, and nothing changes.
pub fn finish(id: &str, g: &mut Guard, t: &Task) -> Res<bool> {
    if t.b("pipelineWait") && t.b("prMerged") { return Ok(true); }
    let root = folder_of(id);
    let Some(a) = crate::azure::host(&root).filter(|_| pr::is_azure(t.s("pr"))) else { return Ok(false) };
    let Ok(v) = pr::view(&a, &root, t.s("pr"), "state") else { return Ok(false) };
    if v.s("state") != "MERGED" { return Ok(false); }
    let head = if v.b("headRefOid") { v.t("headRefOid") } else { t.t("prHead") };
    let mode = mode_of(&root, &tip_of(&root, t), &head);
    let kept = if mode == "always" { head.clone() } else { String::new() };
    let commit = v.at("mergeCommit").t("oid");
    let base = if v.b("baseRefName") { v.t("baseRefName") } else { crate::sync::main_branch(&root, t) };
    if main_pipeline::applies(t) {
        let d = g.set(|t| main_pipeline::arm(t, &commit, &base, mode, &kept))?;
        tell_sidebar(&d);
        return Ok(true);
    }
    let mark = tidy_mark(t, mode, &kept);
    let d = g.set(|t| {
        t.put("stage", "done"); t.put("mergedAt", now()); t.put("tidy", mark.clone().unwrap_or(Value::Null)); t.put("msg", format!("Merged: {}", t.s("pr")));
        if !commit.is_empty() { t.put("mergeCommit", commit.as_str()); }
        for k in ["failure", "prFailure", "mergeIntent", "lineAt", "lineTurn", "checksWait"] { t.put(k, Value::Null); }
        for k in ["autoMerge", "autoMergeLocal", "mergeFailed", "requireApproval"] { t.put(k, false); }
        t.put("prMerged", true); t.put("prState", "merged");
    })?;
    tell_sidebar(&d);
    super::cleanup::tidy(id, mode);
    Ok(true)
}
/// Merged on Azure DevOps and its main pipeline waited for, yet back with you: a land step ran again on it and failed
/// (it merged main into the branch, then found the pull request's head was not that commit).
fn stuck(t: &Task) -> bool {
    t.b("pipelineWait") && t.b("prMerged") && ["approval", "failed"].contains(&t.s("stage")) && t.at("failure").s("stage") == "merge" && !t.b("archivedAt") && !runner_alive(t)
}
/// The board's own merge of main on top of what merged, taken back: a merge commit whose first parent is in what merged
/// (`head`) and whose other one is on main, in a clean worktree on the task's own branch. Where the branch stands after.
fn undo_sync(root: &Path, t: &Task, head: &str, base: &str) -> String {
    let tip = tip_of(root, t);
    if head.is_empty() || tip.is_empty() || tip == head { return tip; }
    let parents: Vec<String> = git_out(root, &["rev-list", "--parents", "-n", "1", &tip]).unwrap_or_default().split_whitespace().skip(1).map(String::from).collect();
    let [mine, main] = parents.as_slice() else { return tip };
    let on = |a: &str, b: &str| a == b || git_out(root, &["merge-base", "--is-ancestor", a, b]).is_ok();
    if !on(mine, head) || !on(main, &format!("refs/remotes/origin/{base}")) { return tip; }
    let tree = crate::sync::tree_of(t);
    let undone = if tree.is_empty() { git_out(root, &["update-ref", &format!("refs/heads/{}", t.s("branch")), mine, &tip]).is_ok() } else {
        let tp = Path::new(&tree);
        branch_of(tp) == t.s("branch") && dirty(tp).is_empty() && git_out(tp, &["reset", "-q", "--keep", mine]).is_ok()
    };
    if undone { mine.clone() } else { tip }
}
/// Every task left stuck so: the wait for its main pipeline goes on, as armed when it merged (housekeeping/prs.rs,
/// before the waits are looked at).
pub fn recover(tasks: &[Task]) {
    for t in tasks.iter().filter(|t| stuck(t)) {
        let root = folder_of(t.s("id"));
        let w = t.at("pipelineWait").clone();
        let head = if w.b("head") { w.t("head") } else { t.t("prHead") };
        let tip = undo_sync(&root, t, &head, w.s("base"));
        let mode = mode_of(&root, &tip, &head);
        let kept = if mode == "always" { head.clone() } else { String::new() };
        if let Ok(d) = update_wait(t.s("id"), 10000, |c| {
            if !stuck(c) { return Ok(Change::Keep); }
            main_pipeline::arm(c, w.s("commit"), w.s("base"), mode, &kept);
            Ok(Change::Write)
        }) { tell_sidebar(&d); }
    }
}
