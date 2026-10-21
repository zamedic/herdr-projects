//! Where a task at Azure DevOps stands while its work waits for your yes: finished and reviewed, it is "ready to
//! approve" as on GitHub, its row saying the review and its pull request's pipeline, whatever that pipeline is doing and
//! whether a vote is still required; a turn of its agent that changes nothing leaves it so. Only a real hold needs you:
//! a policy that failed, a reviewer who rejected it, a build held at an approval in Azure DevOps.
use crate::common;
use common::az::policy;
use common::*;
use serde_json::{json, Value};
use std::path::Path;
use sw::actions::finish_line;
use sw::cli::{flags::parse, tasks::command};
use sw::housekeeping::poll_prs;
use sw::json::*;
use sw::shown::{row_says::sentence, states::{group_of, standing}};
use super::azure_devops_flow::az_world;

fn pass() -> Value { json!({ "verdict": { "meets": true, "findings": [], "text": "Looks right." } }) }
/// Reviewed, passed, its pull request open at Azure DevOps with `says` (its policies, its reviewers' votes).
fn reviewed(settings: Value, says: Value) -> (World, String) {
    let mut s = json!({ "review.when": "always" });
    for (k, v) in settings.as_object().cloned().unwrap_or_default() { s[k] = v; }
    let w = az_world(s);
    w.script(json!({ "worker": [{ "files": { "f.txt": "v1\n" }, "commit": "v1", "note": "Did v1." }], "reviewer": [pass()] }));
    let id = to_approval(&w, "Shop change");
    let head = git(Path::new(task(&id).s("worktree")), &["rev-parse", "HEAD"]);
    let mut c = json!({ "head": head });
    for (k, v) in says.as_object().cloned().unwrap_or_default() { c[k] = v; }
    w.az_says(c);
    poll_prs();
    settle(&id);
    (w, id)
}
fn stands(id: &str) -> (String, String) { let r = row(id); (group_of(&r).to_string(), standing(&r)) }
/// `list`: its line, and what `--json` says of it.
fn listed(id: &str) -> (String, Value) {
    let (v, out) = command("list", &parse(&["list".to_string()])).unwrap_or_else(|e| panic!("{}", e.msg));
    (out.lines().find(|l| l.starts_with(&short(id))).unwrap_or_default().to_string(), v.as_array().and_then(|a| a.iter().find(|x| x.s("id") == id).cloned()).unwrap_or(Value::Null))
}
fn list_line(id: &str) -> String { listed(id).0 }
fn panel(w: &World, id: &str) -> String { let mut b = common::board::open(&w.root, 44); b.s.sel = id.to_string(); b.load(); b.text() }

#[test]
fn reviewed_work_whose_pipeline_runs_is_ready_to_approve_with_the_review_and_the_pipeline_on_its_row() {
    let (w, id) = reviewed(json!({}), json!({ "policies": [policy("Build", "reports", "running", Some(7)), policy("Minimum number of reviewers", "Reviewers", "queued", None)] }));
    let t = task(&id);
    assert_eq!((t.s("stage"), t.s("prState")), ("approval", "waiting"));
    assert_eq!(stands(&id), ("ready to approve".to_string(), "ready to approve".to_string()), "a vote still required is no hold of its own");
    assert_eq!(sentence(&row(&id)).text(), "Pass · Pull request open · pipeline running");
    assert!(t.s("msg").starts_with("Approve to merge? Review: Pass."), "{}", t.s("msg"));
    let line = list_line(&id);
    assert!(line.contains("ready to approve") && !line.contains("blocked"), "{line}");
    let json = listed(&id).1;
    assert_eq!((json.s("state"), json.s("kind"), json.s("stands")), ("blocked", "permission", "ready to approve"), "{json}");
    let text = panel(&w, &id);
    assert!(text.contains("1 ready") && text.contains("◆ ready to approve") && !text.contains("Needs you") && !text.contains("asks for your OK"), "{text}");
    assert!(text.contains("Policies: reports running"), "{text}");
}

#[test]
fn a_turn_of_its_agent_that_ends_with_no_summary_and_changes_nothing_leaves_it_ready_to_approve() {
    let (w, id) = reviewed(json!({}), json!({ "policies": [policy("Build", "reports", "running", Some(7))] }));
    let before = task(&id);
    // typed into its own pane: it works, compacts its conversation and stops, with nothing said and nothing changed
    let pane = before.at("link").t("pane");
    w.patch(|s| { for p in s["panes"].as_array_mut().unwrap() { if p.s("pane_id") == pane { p["agent"]["screen"] = json!("Conversation compacted (ctrl+o for history)\n❯ \n"); } } });
    set_status(&w, &pane, "working");
    w.patch(|s| { for p in s["panes"].as_array_mut().unwrap() { if p.s("pane_id") == pane { let c = p["agent"].i("completion") + 1; p["agent"]["completion"] = json!(c); } } });
    set_status(&w, &pane, "idle");
    let t = settle(&id);
    assert_eq!((t.s("stage"), t.s("msg")), ("approval", before.s("msg")), "never stopped without a summary");
    assert!(!t.at("review").b("stale") && !t.b("requireApproval") && !t.b("chat"), "{}", t.at("chat"));
    assert_eq!(stands(&id).0, "ready to approve");
    let says = sentence(&row(&id)).text();
    assert!(says.starts_with("Pass") && says.contains("pipeline running") && !says.contains("compacted"), "{says}");
}

#[test]
fn a_failed_policy_needs_you_and_says_so() {
    let (_w, id) = reviewed(json!({ "review.rounds": 0 }), json!({ "policies": [policy("Build", "reports", "rejected", Some(7))] }));
    let t = task(&id);
    assert_eq!((t.s("stage"), t.s("prState")), ("approval", "failing"), "{}", t.s("msg"));
    assert_eq!(stands(&id).0, "needs you");
    assert!(t.s("msg").contains("reports"), "{}", t.s("msg"));
}

#[test]
fn a_reviewer_who_rejects_it_in_azure_devops_needs_you() {
    let (_w, id) = reviewed(json!({ "review.rounds": 0 }), json!({ "policies": [policy("Build", "reports", "approved", Some(7))], "reviewers": [{ "displayName": "Ann", "vote": -10 }] }));
    let t = task(&id);
    assert_eq!(t.s("prState"), "failing");
    assert_eq!(stands(&id).0, "needs you");
    assert!(t.s("msg").starts_with("A reviewer asked for changes on the pull request") && t.s("msg").contains("comment threads in Azure DevOps"), "{}", t.s("msg"));
}

#[test]
fn a_build_held_at_an_approval_in_azure_devops_needs_you_there() {
    let (w, id) = reviewed(json!({ "merge.pipeline": "none" }), json!({ "policies": [policy("Build", "reports", "running", Some(7))] }));
    let rec = |id: &str, parent: &str, kind: &str, name: &str, state: &str| json!({ "id": id, "parentId": parent, "type": kind, "name": name, "state": state });
    let held = json!({ "records": [rec("s2", "", "Stage", "Staging", "pending"), rec("c2", "s2", "Checkpoint", "Checkpoint", "inProgress"), rec("a2", "c2", "Checkpoint.Approval", "Checkpoint.Approval", "inProgress")] });
    let head = task(&id).t("prHead");
    w.az_says(json!({ "head": head, "policies": [policy("Build", "reports", "running", Some(7))], "timeline": { "7": held } }));
    finish_line::approve(&id, None, false).unwrap();
    let t = settle(&id);
    assert_eq!((t.s("stage"), t.s("msg")), ("merging", "Pipeline reports waits for your approval of stage Staging in Azure DevOps: https://dev.azure.com/acme/Big%20Project/_build/results?buildId=7"));
    assert_eq!(stands(&id).0, "needs you");
    assert_eq!(sentence(&row(&id)).text(), "Pipeline reports waits for your approval of stage Staging in Azure DevOps: https://dev.azure.com/acme/Big%20Project/_build/results?buildId=7");
}
