//! Your yes, given in Azure DevOps too, as the land step takes it (merge_azure.rs) and before it looks at the
//! policies, so that a yes given while the build still runs votes at once: your Approve vote on the pull request and
//! its auto-complete by `merge.method` (azure/vote.rs), once per approval and reviewed commit (`azureVote` on the
//! task). Azure DevOps then completes it the moment its policies are met, where a push cannot slip through
//! (azure/vote.rs `push_holds`); the board's own completion stays the fallback (a rebase, which az cannot set, an
//! auto-complete it refused or did not set). Both are taken back as soon as that approval no longer stands: a failed
//! merge, a push the board did not review, the task back at work.
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
    // Only where a push cannot be completed before the board looks at it again (vote::push_holds).
    let holds = crate::azure::policies::list(a, root, pr).map(|l| vote::push_holds(&l)).unwrap_or(false);
    let auto = if vote::auto_completes(how) && holds { Some(vote::auto_complete(a, root, pr, how, message)) } else { None };
    let mut v = json!({ "for": approved, "head": head, "voted": cast.status == 0, "autoComplete": auto.as_ref().map(|r| r.status == 0).unwrap_or(false), "waits": [] });
    if cast.status != 0 { v["said"] = json!(vote::first_line(&cast)); }
    if !holds { v["autoSkipped"] = json!("no build validation and no policy that resets the votes on a push: the board completes it at the reviewed commit"); }
    if let Some(r) = auto.as_ref().filter(|r| r.status != 0) { v["autoSaid"] = json!(vote::first_line(r)); }
    g.set(|t| t.put("azureVote", v))
}
/// Your vote or its auto-complete still stands although the approval it was given for does not: the task is no longer
/// merging, or the pull request names another commit than the one approved. `head`: its head now ("" unknown).
pub fn stale(t: &Obj, head: &str) -> bool {
    let v = t.at("azureVote");
    (v.b("autoComplete") || v.b("voted")) && (t.s("stage") != "merging" || (!head.is_empty() && !v.s("head").is_empty() && head != v.s("head")))
}
/// Calls its auto-complete off and takes your vote back, and remembers that. Nothing when neither stands.
pub fn withdraw(id: &str) {
    let Some(t) = crate::store::tasks::get(id) else { return };
    let v = t.at("azureVote");
    if !v.b("autoComplete") && !v.b("voted") { return; }
    let root = folder_of(id);
    let Some(a) = crate::azure::host(&root) else { return };
    // What did not go through is asked again on the next poll.
    let off = !v.b("autoComplete") || vote::call_off(&a, &root, t.s("pr")).status == 0;
    let back = !v.b("voted") || vote::reset(&a, &root, t.s("pr")).status == 0;
    let _ = update_wait(id, 10000, |t| { if t.b("azureVote") { let v = t.obj_mut("azureVote"); if off { v.put("autoComplete", false); } if back { v.put("voted", false); v.put("reset", true); } } Ok(Change::Write) });
}
