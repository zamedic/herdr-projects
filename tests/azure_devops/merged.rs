//! A pull request completed on Azure DevOps (by the board, its auto-complete or you), squashed onto main: main then
//! moves, and nothing merges main into its branch or takes it for a push nobody reviewed. Its wait for the main
//! pipeline is no lost runner, a land step or a sync that finds it completed takes it for merged, and a task left
//! waiting for you so (RPR-25) is finished as merged on the next poll. A push to the pull request after its review is
//! still one that was not reviewed.
use super::flow::{az_world, merge_commit};
use crate::common;
use common::az::{policy, run};
use common::*;
use serde_json::json;
use std::path::Path;
use sw::actions::finish_line;
use sw::housekeeping::{housekeeping, poll_prs, Force};
use sw::json::*;
use sw::store::tasks::{update_wait, Change};

const UNREVIEWED: &str = "Pull request has commits that were not reviewed";

fn pass(w: &World) { housekeeping(&w.socket, Force::Yes); }
fn jobs() -> Vec<String> { sw::runner::JOBS.with(|j| j.borrow().iter().map(|(job, _, _)| job.clone()).collect()) }
/// Where the task's branch stands in its worktree.
fn tip(id: &str) -> String { git(Path::new(task(id).s("worktree")), &["rev-parse", "HEAD"]) }
/// Azure DevOps squashed the task's branch onto main, as its completion does: main moved, with the task's own work.
fn squashed_onto_main(w: &World, id: &str) {
    git(&w.root, &["merge", "-q", "--squash", task(id).s("branch")]);
    git(&w.root, &["-c", "user.email=az@test", "-c", "user.name=az", "commit", "-qm", "Merged PR 42: squashed"]);
    git(&w.root, &["push", "-q", "origin", "main"]);
}
/// Approved with its policies passed: the board completes it, and it waits for its main pipeline.
fn completed(w: &World, title: &str) -> (String, String) {
    let id = to_approval(w, title);
    let reviewed = tip(&id);
    w.az_says(json!({ "head": reviewed, "policies": [policy("Build", "CI build", "approved", Some(7))] }));
    finish_line::approve(&id, None, false).unwrap();
    let t = settle(&id);
    assert!(t.b("prMerged") && t.at("pipelineWait").s("commit") == merge_commit(), "{} {}", t.s("msg"), t.at("failure"));
    (id, reviewed)
}

#[test]
fn a_completed_squash_waiting_for_its_main_pipeline_is_no_lost_runner_when_main_moves_and_is_done_once_it_passed() {
    // reviewed also with nothing left to land, as where RPR-25 was: a land step run again would review the board's own
    // merge of main and take that for a push nobody reviewed
    let w = az_world(json!({ "review.when": "always" }));
    let (id, reviewed) = completed(&w, "Squashed");
    squashed_onto_main(&w, &id);
    // well past the grace a step's runner has to take the task: the wait has no runner, and needs none
    sw::clock::advance(sw::hook::GRACE_MS * 4);
    pass(&w);
    assert!(!jobs().contains(&"land".to_string()), "no land step for merged work: {:?}", jobs());
    let t = settle(&id);
    assert_eq!(t.s("stage"), "merging", "{} {}", t.s("msg"), t.at("failure"));
    assert!(t.b("pipelineWait") && !t.b("failure") && t.i("retried") == 0, "{} {}", t.s("msg"), t.at("failure"));
    assert_eq!(tip(&id), reviewed, "main was not merged into its branch");
    w.az_says(json!({ "head": reviewed, "runs": [run(5, "CI", &merge_commit(), "completed", "succeeded")] }));
    poll_prs();
    let t = settle(&id);
    assert_eq!(t.s("stage"), "done", "{} {}", t.s("msg"), t.at("failure"));
    assert!(t.s("msg").starts_with("Merged: ") && t.b("cleaned"), "{}", t.s("msg"));
}

#[test]
fn a_land_step_that_finds_its_pull_request_auto_completed_takes_it_for_merged_without_merging_main_into_it() {
    let w = az_world(json!({}));
    let id = to_approval(&w, "Auto-completed");
    let reviewed = tip(&id);
    w.az_says(json!({ "head": reviewed, "policies": [policy("Build", "CI build", "running", Some(7))] }));
    finish_line::approve(&id, None, false).unwrap();
    assert!(settle(&id).b("autoMerge"));
    // its policies passed and Azure DevOps' auto-complete squashed it at once; the board's land step comes after that
    w.az_says(json!({ "head": reviewed, "status": "completed", "policies": [policy("Build", "CI build", "approved", Some(7))] }));
    squashed_onto_main(&w, &id);
    update_wait(&id, 1000, |t| { t.put("autoMerge", false); t.put("autoMergeLocal", false); t.put("msg", "Merging"); Ok(Change::Write) }).unwrap();
    sw::runner::spawn_runner("land", &id, &[]);
    let t = settle(&id);
    assert_eq!(t.s("stage"), "merging", "{} {}", t.s("msg"), t.at("failure"));
    assert_ne!(t.s("msg"), UNREVIEWED);
    assert_eq!(t.at("pipelineWait").s("commit"), merge_commit(), "its merge commit is the pull request's lastMergeCommit");
    assert_eq!(tip(&id), reviewed, "main was not merged into its branch");
    assert!(w.az.sent.borrow().iter().all(|s| s.s("resource") != "pullRequests"), "the board completed nothing");
}

#[test]
fn a_sync_with_main_that_finds_its_pull_request_completed_by_you_takes_it_for_merged() {
    let w = az_world(json!({}));
    let id = to_approval(&w, "Completed by you");
    let reviewed = tip(&id);
    w.az_says(json!({ "head": reviewed, "status": "completed" }));
    squashed_onto_main(&w, &id);
    sw::runner::spawn_runner("sync", &id, &[]);
    let t = settle(&id);
    assert_eq!(t.s("stage"), "merging", "{} {}", t.s("msg"), t.at("failure"));
    assert!(t.b("prMerged") && t.at("pipelineWait").s("commit") == merge_commit(), "{}", t.at("pipelineWait"));
    assert_eq!(tip(&id), reviewed, "main was not merged into its branch");
}

#[test]
fn a_task_left_with_you_after_a_land_step_ran_on_its_merged_work_is_finished_as_merged_on_the_next_poll() {
    let w = az_world(json!({}));
    let (id, reviewed) = completed(&w, "Stuck");
    squashed_onto_main(&w, &id);
    // as RPR-25 was left: main merged into its branch by the board, then that head taken for one nobody reviewed
    let wt = task(&id).t("worktree");
    git(Path::new(&wt), &["merge", "-q", "--no-edit", "origin/main"]);
    let made = tip(&id);
    assert_ne!(made, reviewed);
    update_wait(&id, 1000, |t| {
        t.put("stage", "approval"); t.put("mergeFailed", true); t.put("msg", UNREVIEWED);
        t.put("failure", json!({ "stage": "merge", "title": UNREVIEWED, "detail": format!("Reviewed here: {made}\nOn the pull request: {reviewed}"), "agentCanFix": true, "auth": false }));
        Ok(Change::Write)
    }).unwrap();
    poll_prs();
    let t = task(&id);
    assert_eq!(t.s("stage"), "merging", "{} {}", t.s("msg"), t.at("failure"));
    assert!(!t.b("failure") && t.at("pipelineWait").s("commit") == merge_commit() && t.at("pipelineWait").s("mode") == "always", "{}", t.at("pipelineWait"));
    assert_eq!(tip(&id), reviewed, "the board's own merge of main is taken back");
    w.az_says(json!({ "head": reviewed, "runs": [run(5, "CI", &merge_commit(), "completed", "succeeded")] }));
    poll_prs();
    let t = settle(&id);
    assert_eq!(t.s("stage"), "done", "{} {}", t.s("msg"), t.at("failure"));
    assert!(t.b("cleaned"), "{}", t.s("msg"));
}

#[test]
fn a_push_to_the_pull_request_after_its_review_is_still_one_that_was_not_reviewed_also_once_main_moved() {
    let w = az_world(json!({}));
    let id = to_approval(&w, "Pushed after");
    let reviewed = tip(&id);
    w.az_says(json!({ "head": reviewed, "policies": [policy("Build", "CI build", "running", Some(7))] }));
    finish_line::approve(&id, None, false).unwrap();
    assert!(settle(&id).b("autoMerge"));
    // main moved, and someone pushed to the pull request's branch outside the board
    std::fs::write(w.root.join("other.txt"), "other\n").unwrap();
    git(&w.root, &["add", "other.txt"]);
    git(&w.root, &["-c", "user.email=x@test", "-c", "user.name=x", "commit", "-qm", "Another change on main"]);
    git(&w.root, &["push", "-q", "origin", "main"]);
    let other = git(&w.root, &["rev-parse", "main"]);
    git(&w.root, &["push", "-q", "--force", "origin", &format!("main:refs/heads/{}", task(&id).s("branch"))]);
    w.az_says(json!({ "head": other, "policies": [policy("Build", "CI build", "approved", Some(7))] }));
    poll_prs();
    let t = settle(&id);
    assert_eq!((t.s("stage"), t.s("msg")), ("approval", UNREVIEWED), "{}", t.at("failure"));
    assert!(t.at("failure").s("detail").contains(&other) && !t.b("prMerged"), "{}", t.at("failure"));
    assert!(w.az.sent.borrow().iter().all(|s| s.s("resource") != "pullRequests"), "nothing completed");
}
