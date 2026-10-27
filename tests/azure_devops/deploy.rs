//! The deploy of a task's own commit on Azure DevOps (land/main_pipeline.rs), as RPR-24 and RPR-20 met it: only the
//! main pipeline's run of the task's own merge commit counts, never an older run of the same pipeline that passed
//! already; work the agent pushed to main itself (`Already on main`) waits for the run of the commit that brought it
//! there, the same way (squashed onto main: the run of main's tip); the task stays in its stage "merging" through the
//! stages and their approvals, and is done only once the last stage passed. An approval rejected there waits for you
//! with the reason. A pull request that named another merge commit at first (its trial merge) is read again, never
//! taken for one no pipeline ran for. RPR-24's own record (the right merge commit, done after its run passed) fits a
//! correct wait: its test holds what already worked.
use crate::common;
use common::az::run;
use common::*;
use serde_json::{json, Value};
use std::path::Path;
use sw::actions::finish_line;
use sw::housekeeping::poll_prs;
use sw::json::*;
use sw::shown::states::group_of;
use super::flow::{az_world, merge_commit};

const RUN: &str = "https://dev.azure.com/acme/Big%20Project/_build/results?buildId=5";
fn rec(id: &str, parent: &str, kind: &str, name: &str, state: &str, result: &str, order: i64) -> Value {
    json!({ "id": id, "parentId": parent, "type": kind, "name": name, "state": state, "result": if result.is_empty() { Value::Null } else { json!(result) }, "order": order })
}
/// The pipeline `reports` as build 104499 ran: built, then `Staging` and `Production_DE`, each behind an approval of
/// its environment. `staging`/`prod`: where each approval stands (`pending`, `approved`, `rejected`).
fn reports_run(staging: &str, prod: &str) -> Value {
    let stage = |id: &str, name: &str, at: &str, order: i64| {
        let (stage, approval, result) = match at { "approved" => ("completed", "completed", "succeeded"), "rejected" => ("completed", "completed", "failed"), _ => ("pending", "inProgress", "") };
        let done = if at == "approved" { "succeeded" } else if at == "rejected" { "failed" } else { "" };
        vec![rec(id, "", "Stage", name, stage, done, order), rec(&format!("c{id}"), id, "Checkpoint", "Checkpoint", approval, result, 1),
            rec(&format!("a{id}"), &format!("c{id}"), "Checkpoint.Approval", "Checkpoint.Approval", approval, result, 1),
            rec(&format!("p{id}"), id, "Phase", "Deploy", if at == "approved" { "completed" } else { "pending" }, done, 2),
            rec(&format!("j{id}"), &format!("p{id}"), "Job", "Deploy", if at == "approved" { "completed" } else { "pending" }, done, 1)]
    };
    let mut records = vec![rec("b", "", "Stage", "Build & Push", "completed", "succeeded", 1), rec("jb", "b", "Job", "service", "completed", "succeeded", 1)];
    records.extend(stage("st", "Staging", staging, 2));
    // Production_DE comes after Staging: not yet at its checkpoint while Staging is not through.
    if staging == "approved" { records.extend(stage("pd", "Production_DE", prod, 3)); } else { records.push(rec("pd", "", "Stage", "Production_DE", "pending", "", 3)); }
    json!({ "records": records })
}
/// The older run of the same pipeline (build 104492, of the commit before), which passed long ago.
fn older() -> Value { let mut r = run(4, "reports", &"5".repeat(40), "completed", "succeeded"); r["definition"]["id"] = json!(50); r }
fn reports(n: i64, commit: &str, status: &str, result: &str) -> Value { let mut r = run(n, "reports", commit, status, result); r["definition"]["id"] = json!(50); r }
fn shown_done(id: &str) -> bool { sw::flow::live::board(true).iter().find(|r| r.s("id") == id).map(|r| r.t("state")).unwrap_or_default() == "done" }

#[test]
fn an_older_successful_main_run_is_not_the_merge_commits_and_the_task_stays_deploying_through_each_approval() {
    let w = az_world(json!({}));
    let id = to_approval(&w, "Full report update");
    let head = git(Path::new(task(&id).s("worktree")), &["rev-parse", "HEAD"]);
    w.az_says(json!({ "head": head, "runs": [older()] }));
    finish_line::approve(&id, None, false).unwrap();
    let t = settle(&id);
    // merged; the run of the commit before passed, and no run of the merge commit is there yet: it waits
    assert_eq!((t.s("stage"), t.s("msg")), ("merging", format!("Merged: {}. Waiting for the main pipeline to start", t.s("pr")).as_str()), "{}", t.at("failure"));
    poll_prs();
    let t = settle(&id);
    assert_eq!(t.s("stage"), "merging", "the older run is not this task's: {}", t.s("msg"));
    assert!(!shown_done(&id));
    // its own run starts and waits at the approval of Staging: the task waits for you there, not done
    w.az_says(json!({ "head": head, "status": "completed", "runs": [reports(5, &merge_commit(), "inProgress", ""), older()], "timeline": { "5": reports_run("pending", "") } }));
    poll_prs();
    let t = settle(&id);
    assert_eq!((t.s("stage"), t.s("msg")), ("merging", format!("Main pipeline reports waits for your approval of stage Staging in Azure DevOps: {RUN}").as_str()));
    assert_eq!(group_of(&row(&id)), "needs you");
    assert!(!shown_done(&id) && t.at("failure").is_null());
    // Staging approved and deployed, Production_DE waits for its own approval: still not done
    w.az_says(json!({ "head": head, "status": "completed", "runs": [reports(5, &merge_commit(), "inProgress", ""), older()], "timeline": { "5": reports_run("approved", "pending") } }));
    poll_prs();
    let t = settle(&id);
    assert_eq!((t.s("stage"), t.s("msg")), ("merging", format!("Main pipeline reports waits for your approval of stage Production_DE in Azure DevOps: {RUN}").as_str()));
    assert!(!shown_done(&id));
    // the last stage passed: done
    w.az_says(json!({ "head": head, "status": "completed", "runs": [reports(5, &merge_commit(), "completed", "succeeded"), older()] }));
    poll_prs();
    let t = settle(&id);
    assert_eq!(t.s("stage"), "done", "{} {}", t.s("msg"), t.at("failure"));
    assert!(t.s("msg").contains("Main pipeline passed: reports passed"), "{}", t.s("msg"));
}

#[test]
fn work_the_agent_pushed_to_main_itself_waits_for_the_main_pipeline_of_that_commit_through_its_approvals() {
    let w = az_world(json!({}));
    w.az_says(json!({ "runs": [older()] }));
    w.script(json!({ "worker": [{ "files": { "f.txt": "fix\n" }, "commit": "fix(scheduler)", "note": "Pushed the fix to main.", "git": [["push", "-q", "origin", "HEAD:main"]] }] }));
    let id = w.add("Scheduled refresh");
    delegate(&id);
    let t = settle(&id);
    let pushed = git(&w.root, &["rev-parse", "refs/remotes/origin/main"]);
    assert_eq!(t.s("doneWhy"), "Already on main");
    assert!(!az_log(&w).contains("repos pr create"), "{}", az_log(&w));
    // on main is not deployed: the run of the commit it pushed is waited for, as after a merge
    assert_eq!(t.s("stage"), "merging", "{} {}", t.s("msg"), t.at("failure"));
    assert_eq!((t.at("pipelineWait").s("commit"), t.at("pipelineWait").s("base")), (pushed.as_str(), "main"), "{}", t.at("pipelineWait"));
    assert_eq!(t.s("msg"), format!("On main: {}. Waiting for the main pipeline to start", &pushed[..7]));
    assert!(!t.b("prMerged") && !shown_done(&id));
    poll_prs();
    assert_eq!(settle(&id).s("stage"), "merging", "the older run is not this commit's");
    // its run waits at the approval of Staging
    w.az_says(json!({ "runs": [reports(5, &pushed, "inProgress", ""), older()], "timeline": { "5": reports_run("pending", "") } }));
    poll_prs();
    let t = settle(&id);
    assert_eq!((t.s("stage"), t.s("msg")), ("merging", format!("Main pipeline reports waits for your approval of stage Staging in Azure DevOps: {RUN}").as_str()));
    assert_eq!(group_of(&row(&id)), "needs you");
    w.az_says(json!({ "runs": [reports(5, &pushed, "completed", "succeeded"), older()] }));
    poll_prs();
    let t = settle(&id);
    assert_eq!(t.s("stage"), "done", "{} {}", t.s("msg"), t.at("failure"));
    assert_eq!(t.s("msg"), format!("On main: {}. Main pipeline passed: reports passed", &pushed[..7]));
    assert_eq!(t.s("doneWhy"), "Already on main");
}

#[test]
fn a_pull_request_of_the_branch_merged_before_the_work_is_not_what_brought_it_to_main() {
    let w = az_world(json!({}));
    // an earlier pull request of the branch, merged at the commit main started from: not this work's
    let start = git(&w.root, &["rev-parse", "HEAD"]);
    let merged = json!([{ "pullRequestId": 1200, "status": "completed", "targetRefName": "refs/heads/main", "lastMergeSourceCommit": { "commitId": start }, "lastMergeCommit": { "commitId": "5".repeat(40) } }]);
    w.az_says(json!({ "prs": merged, "runs": [older()] }));
    w.script(json!({ "worker": [{ "files": { "f.txt": "fix\n" }, "commit": "fix", "note": "Pushed it.", "git": [["push", "-q", "origin", "HEAD:main"]] }] }));
    let id = w.add("After an old merge");
    delegate(&id);
    let t = settle(&id);
    let pushed = git(&w.root, &["rev-parse", "refs/remotes/origin/main"]);
    assert_eq!(t.s("doneWhy"), "Already on main");
    assert_eq!((t.s("stage"), t.at("pipelineWait").s("commit")), ("merging", pushed.as_str()), "{}", t.s("msg"));
}

#[test]
fn a_stage_approval_rejected_in_azure_devops_waits_for_you_with_the_reason() {
    let w = az_world(json!({}));
    let id = to_approval(&w, "Rejected rollout");
    let head = git(Path::new(task(&id).s("worktree")), &["rev-parse", "HEAD"]);
    w.az_says(json!({ "head": head }));
    finish_line::approve(&id, None, false).unwrap();
    assert!(settle(&id).b("pipelineWait"));
    w.az_says(json!({ "head": head, "status": "completed", "runs": [reports(5, &merge_commit(), "completed", "failed")], "timeline": { "5": reports_run("approved", "rejected") } }));
    poll_prs();
    let t = settle(&id);
    assert_eq!(t.s("stage"), "failed", "{}", t.s("msg"));
    assert_eq!(t.at("failure").s("title"), "Main pipeline stopped: The approval of stage Production_DE was rejected or ran out in Azure DevOps (run 5)");
    assert!(t.at("failure").s("detail").contains(RUN), "{}", t.at("failure"));
    assert_eq!(t.at("failure").at("agentCanFix"), &json!(false));
    assert!(!shown_done(&id));
    // approved there after all, run again: retry waits for it again, and it is done once it passed
    w.az_says(json!({ "head": head, "status": "completed", "runs": [reports(6, &merge_commit(), "completed", "succeeded")] }));
    finish_line::retry(&id).unwrap();
    assert_eq!(settle(&id).s("stage"), "done");
}

#[test]
fn work_squashed_onto_main_by_the_agent_waits_for_the_run_of_mains_tip() {
    let w = az_world(json!({}));
    let squash = json!([["fetch", "-q", "origin", "main"], ["checkout", "-q", "--detach", "FETCH_HEAD"], ["merge", "-q", "--squash", "@{-1}"],
        ["-c", "user.email=agent@test", "-c", "user.name=agent", "commit", "-qm", "Squashed"], ["push", "-q", "origin", "HEAD:main"], ["checkout", "-q", "-"]]);
    w.script(json!({ "worker": [{ "files": { "f.txt": "v1\n" }, "commit": "v1", "note": "Squashed it onto main.", "git": squash }] }));
    let id = w.add("Squashed by hand");
    delegate(&id);
    let t = settle(&id);
    let tip = git(&w.root, &["rev-parse", "refs/remotes/origin/main"]);
    assert_eq!(t.s("doneWhy"), "Already on main");
    assert_eq!((t.s("stage"), t.at("pipelineWait").s("commit")), ("merging", tip.as_str()), "{} {}", t.s("msg"), t.at("failure"));
}

#[test]
fn a_pull_request_that_named_another_merge_commit_at_first_is_read_again_and_its_real_one_waited_for() {
    let w = az_world(json!({}));
    let id = to_approval(&w, "Trial merge first");
    let head = git(Path::new(task(&id).s("worktree")), &["rev-parse", "HEAD"]);
    // completed, Azure DevOps still names its trial merge as the merge commit
    let trial = "7".repeat(40);
    w.az_says(json!({ "head": head, "merge": trial, "runs": [older()] }));
    finish_line::approve(&id, None, false).unwrap();
    assert_eq!(settle(&id).at("pipelineWait").s("commit"), trial);
    // read again: the real one, whose run waits at Staging's approval
    w.az_says(json!({ "head": head, "runs": [reports(5, &merge_commit(), "inProgress", ""), older()], "timeline": { "5": reports_run("pending", "") } }));
    poll_prs();
    let t = settle(&id);
    assert_eq!((t.at("pipelineWait").s("commit"), t.s("mergeCommit")), (merge_commit().as_str(), merge_commit().as_str()));
    poll_prs();
    assert_eq!(settle(&id).s("msg"), format!("Main pipeline reports waits for your approval of stage Staging in Azure DevOps: {RUN}"));
    // past the grace for a commit no pipeline ran for: still not done, its run is under way
    sw::clock::advance(11 * 60000);
    poll_prs();
    let t = settle(&id);
    assert_eq!(t.s("stage"), "merging", "{}", t.s("msg"));
    assert!(!shown_done(&id));
}
