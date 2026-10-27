//! Work the board opens no pull request of its own for: a branch that brings main nothing (nothing needed changing, the
//! agent merged its own pull request, or pushed to main itself), a pull request the agent already opened for the task's
//! branch, and a repo archived on GitHub. On Azure DevOps the same, its pull requests read with az (`Already merged in !1234`). Seen on HSW-3, SKL-8, ELA-2, ELA-3, HSW-213, HP-5 and HSW-219.
use crate::flow::git::{change_of, gh};
use crate::json::*;
use crate::store::repos::git_out;
use crate::store::tasks::Task;
use std::path::Path;

/// The pull requests GitHub has for this head branch, newest first: { number, url, state, baseRefName }. None when gh
/// cannot say.
fn prs_of(branch: &str, cwd: &Path) -> Vec<Value> {
    if branch.is_empty() { return vec![]; }
    // On Azure DevOps, its pull requests in the same shape (azure/pr.rs).
    if let Some(a) = crate::azure::host(cwd) { return crate::azure::pr::prs_of(&a, cwd, branch); }
    let r = gh(&["pr", "list", "--head", branch, "--state", "all", "--limit", "10", "--json", "number,url,state,baseRefName"], cwd, 60000);
    if r.status != 0 { return vec![]; }
    serde_json::from_str::<Value>(r.stdout.trim()).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default()
}
/// A pull request the agent opened itself: its link, its number and the branch it merges into ("" when not said).
pub struct OwnPr { pub url: String, pub number: String, pub base: String }
/// The pull request the agent opened itself for the task's branch, still open: the board takes it as its own, once it
/// merges into the branch the task lands on (`wrong_base`).
pub fn open_pr(branch: &str, cwd: &Path) -> Option<OwnPr> {
    prs_of(branch, cwd).into_iter().find(|p| p.s("state") == "OPEN" && p.b("url")).map(|p| OwnPr { url: p.t("url"), number: p.t("number"), base: p.t("baseRefName") })
}
/// Why the agent's own pull request cannot be the task's: it merges into another branch than `onto`, the one the task
/// started from and keeps up with. None when it merges into that one.
pub fn wrong_base(p: &OwnPr, onto: &str) -> Option<String> {
    (!p.base.is_empty() && p.base != onto).then(|| format!("The agent opened pull request {} ({}) for this branch itself, and it merges into {}. This task started from {onto} and lands on {onto}, so the board does not take it as its own and opens none beside it.", crate::azure::numbered(&p.url, &p.number), p.url, p.base))
}
/// Whether the task made commits of its own: those on its branch before main was brought in now, that neither the
/// branch it started from nor main as it was last brought in has.
pub fn made_commits(tree: &Path, base: &str, before: &str, prev_main: &str) -> bool {
    let mut args = vec!["rev-list", "--count", before, "--not", base];
    if !prev_main.is_empty() { args.push(prev_main); }
    git_out(tree, &args).ok().and_then(|n| n.parse::<i64>().ok()).unwrap_or(0) > 0
}
/// Why the task is done without a pull request, when its branch brings main nothing: main (brought in just before) has
/// everything on it. `ask`: GitHub may be asked for a pull request of the branch that was merged. `before`: where the
/// branch stood before main was brought in. None: there is work to land. With it, the pull request it was merged in
/// (gh's shape), when the agent merged its own; on Azure DevOps, when the agent pushed it to main itself,
/// `{ direct, mergeCommit, baseRefName }`: the commit that brought it there. Either way the task then waits for that
/// commit's main pipeline (pipeline_end.rs).
pub fn nothing_to_land(t: &Task, tree: &Path, own: bool, ask: bool, before: &str) -> Option<(String, Option<Value>)> {
    let ch = change_of(t)?;
    if !ch.s("stat").is_empty() { return None; }
    let merged = if ask { prs_of(t.s("branch"), tree).into_iter().find(|p| p.s("state") == "MERGED" && holds(tree, p, before)) } else { None };
    Some(match merged {
        Some(p) => (format!("Already merged in {}", crate::azure::numbered(p.s("url"), &p.t("number"))), Some(p)),
        None if own => {
            let base = crate::sync::main_branch(&crate::store::repos::folder_of(t.s("id")), t);
            let direct = crate::azure::host(tree).and_then(|_| landed_at(tree, before, &base)).map(|c| json!({ "direct": true, "mergeCommit": { "oid": c }, "baseRefName": base }));
            (format!("Already on {}", crate::sync::main_word(t)), direct)
        }
        None => ("No change needed".into(), None),
    })
}
/// Whether a merged pull request of the branch is what brought the work at `before`: its last commit has it. One merged
/// before the work was done is not, and neither is its merge commit (nor that commit's pipeline run). Its last commit
/// unknown here (not said, or not fetched): taken as it is.
fn holds(tree: &Path, p: &Value, before: &str) -> bool {
    let head = p.s("headRefOid");
    if head.is_empty() || before.is_empty() || head == before || git_out(tree, &["cat-file", "-e", &format!("{head}^{{commit}}")]).is_err() { return true; }
    git_out(tree, &["merge-base", "--is-ancestor", before, head]).is_ok()
}
/// The commit by which work at `head` came onto `base` at the remote: `head` itself when it is on the branch's line of
/// first parents (it was pushed there as it is: the commit its pipeline runs for), else the oldest commit of that line
/// that holds it (a merge of it made on main). None when the remote's `base` does not hold it.
pub fn landed_at(tree: &Path, head: &str, base: &str) -> Option<String> {
    let main = format!("refs/remotes/origin/{base}");
    let head = git_out(tree, &["rev-parse", "--verify", "-q", &format!("{head}^{{commit}}")]).ok().filter(|h| !h.is_empty())?;
    git_out(tree, &["merge-base", "--is-ancestor", &head, &main]).ok()?;
    let line = git_out(tree, &["rev-list", "--first-parent", "-n", "1000", &main]).ok()?;
    let after = git_out(tree, &["rev-list", "--ancestry-path", &format!("{head}..{main}")]).unwrap_or_default();
    let after: Vec<&str> = after.lines().collect();
    let mut at = None;
    for c in line.lines() {
        if c == head { return Some(head); }
        if !after.contains(&c) { break; }
        at = Some(c.to_string());
    }
    at
}
/// The repo is archived on GitHub: read-only, nothing can be pushed to it or merged.
pub fn archived(cwd: &Path) -> bool {
    if crate::azure::host(cwd).is_some() { return false; }
    let r = gh(&["repo", "view", "--json", "isArchived", "--jq", ".isArchived"], cwd, 60000);
    r.status == 0 && r.stdout.trim() == "true"
}
/// The failure for work that cannot land because its repo is archived. It is not on main, so it is never done.
pub fn archived_failure(branch: &str, tree: &str) -> Obj {
    let mut f = super::shared::failure("push", "The repo is archived on GitHub", &format!("Branch: {branch}\nFolder: {tree}\n\nThe repo on GitHub is archived, so it is read-only: the branch cannot be pushed and its work is not on main."), "If this work should land, unarchive the repo on GitHub and finish again. If it is not needed, archive the task.");
    f.put("agentCanFix", false);
    f
}
