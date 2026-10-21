//! A pull request's own pipeline on Azure DevOps held at an approval before your yes (an environment's check before a
//! stage, `Checkpoint.Approval` under way in its timeline): the task needs you there, its row and panel say which
//! stage with the run's URL, nothing failed and nothing goes to the agent, and your yes here still merges it once the
//! approval was given there and the policies passed. Read from recorded timelines: one held, one with a skipped stage,
//! one with a failed stage.
use crate::common;
use common::az::policy;
use common::*;
use serde_json::{json, Value};
use std::path::Path;
use sw::actions::finish_line;
use sw::azure::{logs, pr_pipeline, runs};
use sw::housekeeping::poll_prs;
use sw::json::*;
use sw::shown::{row_says::sentence, states::group_of};
use super::flow::{az_world, merge_commit};

const RUN: &str = "https://dev.azure.com/acme/Big%20Project/_build/results?buildId=8";
fn rec(id: &str, parent: &str, kind: &str, name: &str, state: &str, result: Option<&str>, order: i64) -> Value {
    json!({ "id": id, "parentId": parent, "type": kind, "name": name, "state": state, "result": result, "order": order, "log": null })
}
/// A run as Azure Pipelines recorded it (build 104403 in 360-Backend): built, planned and reviewed, its lint skipped,
/// and its stage `Staging` held at the approval of its environment, `Apply - (prodDE)` still to come.
fn held_timeline() -> Value {
    let ok = Some("succeeded");
    json!({ "records": [
        rec("s1", "", "Stage", "Build & Push", "completed", ok, 1), rec("p1", "s1", "Phase", "service", "completed", ok, 1), rec("j1", "p1", "Job", "service", "completed", ok, 1),
        rec("s2", "", "Stage", "Plan - (prodDE)", "completed", ok, 2), rec("s3", "", "Stage", "AI code review", "completed", ok, 3),
        rec("s4", "", "Stage", "Lint", "completed", Some("skipped"), 4),
        rec("s5", "", "Stage", "Staging", "pending", None, 5), rec("c5", "s5", "Checkpoint", "Checkpoint", "inProgress", None, 1),
        rec("a5", "c5", "Checkpoint.Approval", "Checkpoint.Approval", "inProgress", None, 1), rec("e5", "c5", "Checkpoint.ExtendsCheck", "Checkpoint.ExtendsCheck", "completed", ok, 2),
        rec("p5", "s5", "Phase", "Deploy", "pending", None, 2),
        rec("s6", "", "Stage", "Apply - (prodDE)", "pending", None, 6),
    ] })
}
/// The same run once approved there: `Staging` runs.
fn approved_timeline() -> Value {
    let mut t = held_timeline();
    for r in t["records"].as_array_mut().unwrap() { match r.s("id") { "a5" => { r["state"] = json!("completed"); r["result"] = json!("succeeded"); } "s5" | "c5" | "p5" => r["state"] = json!("inProgress"), _ => {} } }
    t
}
/// A run whose stage `Staging` failed in its job `Deploy`, step `Helm upgrade`; the stage after it skipped.
fn failed_timeline() -> Value {
    let (ok, bad) = (Some("succeeded"), Some("failed"));
    let mut step = rec("t5", "j5", "Task", "Helm upgrade", "completed", bad, 2);
    step["log"] = json!({ "id": 31 });
    json!({ "records": [
        rec("s1", "", "Stage", "Build & Push", "completed", ok, 1), rec("s5", "", "Stage", "Staging", "completed", bad, 2), rec("p5", "s5", "Phase", "Deploy", "completed", bad, 1),
        rec("j5", "p5", "Job", "Deploy", "completed", bad, 1), step, rec("s6", "", "Stage", "Apply - (prodDE)", "completed", Some("skipped"), 3),
    ] })
}
fn stages(v: &Value) -> Vec<runs::Stage> { runs::stages_of(&logs::records_of(v)) }

#[test]
fn a_recorded_timeline_says_the_stage_held_at_an_approval_the_stages_skipped_and_what_failed() {
    let held = stages(&held_timeline());
    assert_eq!(runs::approvals_of(&logs::records_of(&held_timeline())), vec!["Staging".to_string()]);
    let mut pending = held_timeline(); pending["records"][8]["state"] = json!("pending");
    assert_eq!(pending["records"][8].s("type"), "Checkpoint.Approval");
    assert_eq!(runs::approvals_of(&logs::records_of(&pending)), vec!["Staging".to_string()], "a pending approval holds it too");
    assert_eq!(pr_pipeline::words("running", &held), "Pull request open · waits for your approval of stage Staging in Azure DevOps");
    assert_eq!(runs::awaiting_of(&[("Pipeline reports".into(), RUN.into(), logs::records_of(&held_timeline()))]), format!("Pipeline reports waits for your approval of stage Staging in Azure DevOps: {RUN}"));
    // approved there: it runs on, its skipped stage no stage of the run
    let going = stages(&approved_timeline());
    assert!(runs::approvals_of(&logs::records_of(&approved_timeline())).is_empty());
    assert_eq!(pr_pipeline::words("running", &going), "Pull request open · pipeline running · 3 of 5 stages · Lint skipped");
    assert_eq!(runs::progress(&going, 1), "Main pipeline running · 3 of 5 stages · Lint skipped");
    // failed: the failing job and step, as before; nothing waits for you
    let recs = logs::records_of(&failed_timeline());
    assert!(runs::approvals_of(&recs).is_empty());
    let failed = logs::failed(&recs);
    assert_eq!((failed[0].0.as_str(), failed[0].1.name.as_str(), failed[0].1.log.as_str()), ("Deploy", "Helm upgrade", "31"));
    assert_eq!(pr_pipeline::words("failed", &stages(&failed_timeline())), "Pull request open · pipeline failed");
}

#[test]
fn a_pull_request_pipeline_held_at_an_approval_before_your_yes_needs_you_there_and_your_yes_still_merges_it() {
    let w = az_world(json!({ "merge.pipeline": "none", "review.rounds": 0 }));
    let id = to_approval(&w, "Staging needs a yes");
    let head = git(Path::new(task(&id).s("worktree")), &["rev-parse", "HEAD"]);
    // its first run failed: a real failure, with the failing job and the end of its log
    w.az_says(json!({ "head": head, "policies": [policy("Build", "reports", "rejected", Some(7))], "timeline": { "7": failed_timeline() }, "log": ["Error: UPGRADE FAILED: timed out"] }));
    poll_prs();
    let t = settle(&id);
    assert_eq!(t.at("prFailure").s("title"), "Pull request checks failed");
    assert!(t.at("prFailure").s("detail").contains("Failed in job Deploy, step Helm upgrade") && t.at("prFailure").s("detail").contains("UPGRADE FAILED"), "{}", t.at("prFailure"));
    // run again, it waits at the approval of Staging: nothing failed any more, it waits for you there
    w.az_says(json!({ "head": head, "policies": [policy("Build", "reports", "running", Some(8))], "timeline": { "8": held_timeline() } }));
    poll_prs();
    let t = settle(&id);
    assert!(t.at("prFailure").is_null() && t.at("failure").is_null(), "{}", t.at("prFailure"));
    assert!(t.s("msg").starts_with("Approve to merge?"), "the approval question stays: {}", t.s("msg"));
    assert_eq!(t.i("prRounds"), 0, "nothing was sent to the agent");
    assert_eq!(t.at("prPipeline").s("approval"), format!("Pipeline reports waits for your approval of stage Staging in Azure DevOps: {RUN}"));
    let r = row(&id);
    assert_eq!(group_of(&r), "needs you");
    assert!(sentence(&r).text().ends_with("Pull request open · waits for your approval of stage Staging in Azure DevOps"), "{}", sentence(&r).text());
    let mut b = common::board::open(&w.root, 44);
    b.s.sel = id.clone(); b.load();
    let text = b.text();
    assert!(text.contains("Approve      Pipeline reports waits for") && text.contains("your approval of stage Staging") && text.contains("_build/results?buil"), "the panel says what to approve, and where: {text}");
    assert!(!text.contains("tell this task's agent to fix it") && !text.contains("checks failed"), "no fix offered: {text}");
    // your yes is still yours to give here: remembered, it waits for the approval there, then for the build
    finish_line::approve(&id, None, false).unwrap();
    let t = settle(&id);
    assert_eq!((t.s("stage"), t.s("msg")), ("merging", format!("Pipeline reports waits for your approval of stage Staging in Azure DevOps: {RUN}").as_str()));
    w.az_says(json!({ "head": head, "policies": [policy("Build", "reports", "running", Some(8))], "timeline": { "8": approved_timeline() } }));
    poll_prs();
    assert_eq!(settle(&id).s("msg"), "Waiting for Azure DevOps checks · 0 of 1 passed · Approved here and in Azure");
    w.az_says(json!({ "head": head, "policies": [policy("Build", "reports", "approved", Some(8))] }));
    poll_prs();
    let t = settle(&id);
    assert_eq!(t.s("stage"), "done", "{} {}", t.s("msg"), t.at("failure"));
}

#[test]
fn the_main_pipeline_held_at_the_approval_of_prod_de_waits_for_you_neither_running_nor_failed() {
    let w = az_world(json!({}));
    let id = to_approval(&w, "Deploy to prodDE");
    let head = git(Path::new(task(&id).s("worktree")), &["rev-parse", "HEAD"]);
    w.az_says(json!({ "head": head }));
    finish_line::approve(&id, None, false).unwrap();
    assert!(settle(&id).b("pipelineWait"));
    let mut tl = approved_timeline();
    for r in tl["records"].as_array_mut().unwrap() { match r.s("id") { "s5" | "c5" | "p5" => { r["state"] = json!("completed"); r["result"] = json!("succeeded"); } "s6" => r["state"] = json!("pending"), _ => {} } }
    tl["records"].as_array_mut().unwrap().extend([rec("c6", "s6", "Checkpoint", "Checkpoint", "inProgress", None, 1), rec("a6", "c6", "Checkpoint.Approval", "Checkpoint.Approval", "inProgress", None, 1)]);
    w.az_says(json!({ "head": head, "status": "completed", "runs": [common::az::run(5, "reports", &merge_commit(), "inProgress", "")], "timeline": { "5": tl } }));
    poll_prs();
    let t = settle(&id);
    assert_eq!((t.s("stage"), t.s("msg")), ("merging", "Main pipeline reports waits for your approval of stage Apply - (prodDE) in Azure DevOps: https://dev.azure.com/acme/Big%20Project/_build/results?buildId=5"));
    assert_eq!(group_of(&row(&id)), "needs you");
    assert!(t.at("failure").is_null());
}

#[test]
fn a_pull_request_build_still_running_past_check_timeout_is_waited_for_and_the_approval_stays_in_force() {
    let w = az_world(json!({ "merge.pipeline": "none", "check.timeout": "15m" }));
    let id = to_approval(&w, "A long build");
    let head = git(Path::new(task(&id).s("worktree")), &["rev-parse", "HEAD"]);
    w.az_says(json!({ "head": head, "policies": [policy("Build", "webapp", "running", Some(8))], "timeline": { "8": approved_timeline() } }));
    finish_line::approve(&id, None, false).unwrap();
    settle(&id);
    sw::clock::advance(40 * 60000);
    poll_prs();
    let t = settle(&id);
    assert_eq!((t.s("stage"), t.s("msg")), ("merging", "Waiting for Azure DevOps checks · 0 of 1 passed · Approved here and in Azure"), "{}", t.at("failure"));
    assert!(t.at("failure").is_null() && t.b("autoMerge"));
    w.az_says(json!({ "head": head, "policies": [policy("Build", "webapp", "approved", Some(8))] }));
    poll_prs();
    let t = settle(&id);
    assert_eq!(t.s("stage"), "done", "completed by itself: {} {}", t.s("msg"), t.at("failure"));
}

#[test]
fn a_main_pipeline_run_under_way_past_its_timeout_is_still_running_not_failed() {
    let w = az_world(json!({ "merge.pipelineTimeout": "30m" }));
    let id = to_approval(&w, "A long deploy");
    let head = git(Path::new(task(&id).s("worktree")), &["rev-parse", "HEAD"]);
    w.az_says(json!({ "head": head }));
    finish_line::approve(&id, None, false).unwrap();
    settle(&id);
    w.az_says(json!({ "head": head, "status": "completed", "runs": [common::az::run(5, "reports", &merge_commit(), "inProgress", "")], "timeline": { "5": approved_timeline() } }));
    sw::clock::advance(45 * 60000);
    poll_prs();
    let t = settle(&id);
    assert_eq!((t.s("stage"), t.s("msg")), ("merging", "Main pipeline running · 3 of 5 stages · Lint skipped"), "{}", t.at("failure"));
    w.az_says(json!({ "head": head, "status": "completed", "runs": [common::az::run(5, "reports", &merge_commit(), "completed", "succeeded")] }));
    poll_prs();
    assert_eq!(settle(&id).s("stage"), "done");
}

#[test]
fn a_wait_for_its_builds_that_ran_out_before_is_cleared_once_the_build_passed() {
    let w = az_world(json!({ "merge.pipeline": "none" }));
    let id = to_approval(&w, "Ran out before");
    let head = git(Path::new(task(&id).s("worktree")), &["rev-parse", "HEAD"]);
    // as an earlier yes left it (webapp/13): the build took longer than check.timeout
    let title = "Azure DevOps' checks did not finish";
    sw::store::tasks::update_wait(&id, 10000, |t| {
        t.put("mergeFailed", true); t.put("msg", title);
        t.put("failure", json!({ "stage": "merge", "title": title, "detail": "Still not finished after 15m (check.timeout): webapp\n\nNothing was merged.", "agentCanFix": false, "auth": false, "sentAt": null }));
        Ok(sw::store::tasks::Change::Write)
    }).unwrap();
    assert_eq!(group_of(&row(&id)), "needs you");
    w.az_says(json!({ "head": head, "policies": [policy("Build", "webapp", "approved", Some(8))], "timeline": { "8": approved_timeline() } }));
    poll_prs();
    let t = settle(&id);
    assert!(t.at("failure").is_null() && !t.b("mergeFailed"), "{}", t.at("failure"));
    assert_eq!(t.s("stage"), "approval");
    assert!(t.s("msg").starts_with("Approve to merge? Did v1."), "the approval question again: {}", t.s("msg"));
    assert_eq!(group_of(&row(&id)), "ready to approve");
}

#[test]
fn a_main_pipeline_run_queued_with_no_agent_past_its_timeout_waits_for_you() {
    let w = az_world(json!({ "merge.pipelineTimeout": "30m" }));
    let id = to_approval(&w, "No agent");
    let head = git(Path::new(task(&id).s("worktree")), &["rev-parse", "HEAD"]);
    w.az_says(json!({ "head": head }));
    finish_line::approve(&id, None, false).unwrap();
    settle(&id);
    let queued = json!({ "records": [rec("s1", "", "Stage", "Build & Push", "pending", None, 1), rec("j1", "s1", "Job", "service", "pending", None, 1)] });
    w.az_says(json!({ "head": head, "status": "completed", "runs": [common::az::run(5, "reports", &merge_commit(), "notStarted", "")], "timeline": { "5": queued } }));
    poll_prs();
    assert_eq!(settle(&id).s("stage"), "merging", "not yet past its timeout");
    sw::clock::advance(10 * 60 * 60000);
    poll_prs();
    let t = settle(&id);
    assert_eq!((t.s("stage"), t.at("failure").s("title")), ("failed", "The main pipeline did not finish"), "{}", t.s("msg"));
    assert!(t.at("failure").s("detail").contains("no job started"), "{}", t.at("failure"));
}
