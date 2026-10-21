//! Your yes, given in Azure DevOps too, as the land step takes it (merge_azure.rs) and before it looks at the
//! policies, so that a yes given while the build still runs votes at once: your Approve vote on the pull request and
//! its auto-complete by `merge.method` (azure/vote.rs), once per approval and reviewed commit (`azureVote` on the
//! task). Azure DevOps then completes it the moment its policies are met; the board's own completion stays the fallback
//! (a rebase, which az cannot set, or an auto-complete it refused). The auto-complete is called off as soon as that
//! approval no longer stands: a failed merge, a push the board did not review, the task back at work.
use crate::azure::{vote, Azure};
use crate::err::Res;
use crate::json::*;
use crate::runner::Guard;
use crate::store::repos::folder_of;
use crate::store::tasks::{update_wait, Change, Task};
use std::path::Path;

/// Votes and sets the auto-complete, unless that was done for this approval and commit already. The task as it is then.
pub fn give(g: &mut Guard, t: &Task, root: &Path, a: &Azure, pr: &str, how: &str, message: &str) -> Res<Task> {
    let (approved, head) = (t.at("mergeIntent").i("approvedAt"), t.at("mergeIntent").t("verifiedHead"));
    let was = t.at("azureVote");
    if was.is_object() && was.i("for") == approved && was.s("head") == head { return Ok(t.clone()); }
    let cast = vote::cast(a, root, pr);
    let auto = if vote::auto_completes(how) { Some(vote::auto_complete(a, root, pr, how, message)) } else { None };
    let mut v = json!({ "for": approved, "head": head, "voted": cast.status == 0, "autoComplete": auto.as_ref().map(|r| r.status == 0).unwrap_or(false), "waits": [] });
    if cast.status != 0 { v["said"] = json!(vote::first_line(&cast)); }
    if let Some(r) = auto.as_ref().filter(|r| r.status != 0) { v["autoSaid"] = json!(vote::first_line(r)); }
    g.set(|t| t.put("azureVote", v))
}
/// Its auto-complete still set although the approval it was set for no longer stands: the task is no longer merging,
/// or the pull request names another commit than the one approved. `head`: the pull request's head now ("" unknown).
pub fn stale(t: &Obj, head: &str) -> bool {
    let v = t.at("azureVote");
    v.b("autoComplete") && (t.s("stage") != "merging" || (!head.is_empty() && !v.s("head").is_empty() && head != v.s("head")))
}
/// Calls its auto-complete off, and remembers that. Nothing when none is set.
pub fn withdraw(id: &str) {
    let Some(t) = crate::store::tasks::get(id) else { return };
    if !t.at("azureVote").b("autoComplete") { return; }
    let root = folder_of(id);
    let Some(a) = crate::azure::host(&root) else { return };
    if vote::call_off(&a, &root, t.s("pr")).status != 0 { return; } // asked again on the next poll
    let _ = update_wait(id, 10000, |t| { if t.b("azureVote") { t.obj_mut("azureVote").put("autoComplete", false); } Ok(Change::Write) });
}
