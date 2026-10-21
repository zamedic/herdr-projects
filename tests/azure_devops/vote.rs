//! Your yes on the board given in Azure DevOps too: your Approve vote on the pull request and its auto-complete by
//! `merge.method`, cast at once (also while its build still runs), once per approval; what it still waits for from
//! people said on the task, in `details`, in the panel and in yes's answer to a task held there; the auto-complete
//! set only where a push cannot be completed before the board looks again, and both taken back once the approval no
//! longer stands; a rebase (which az cannot set) completed by the board alone.
use super::flow::az_world;
use crate::common;
use common::az::policy;
use common::*;
use serde_json::{json, Value};
use std::path::Path;
use sw::actions::finish_line;
use sw::azure::waits;
use sw::housekeeping::poll_prs;
use sw::json::*;

fn reviewers_policy(need: i64, own_counts: bool) -> Value {
    let mut p = policy("Minimum number of reviewers", "", "queued", None);
    p["configuration"]["settings"] = json!({ "minimumApproverCount": need, "creatorVoteCounts": own_counts });
    p
}
/// A policy of reviewers that resets their votes on a push: one under which an auto-complete is set.
fn resetting() -> Value { let mut p = policy("Minimum number of reviewers", "", "queued", None); p["configuration"]["settings"] = json!({ "resetOnSourcePush": true }); p }
fn reviewer(name: &str, login: &str, vote: i64) -> Value { json!({ "displayName": name, "uniqueName": login, "id": login, "vote": vote }) }
fn approve(w: &World, title: &str, says: Value) -> (String, String) {
    let id = to_approval(w, title);
    let reviewed = git(Path::new(task(&id).s("worktree")), &["rev-parse", "HEAD"]);
    let mut s = says; s["head"] = json!(reviewed);
    w.az_says(s);
    finish_line::approve(&id, None, false).unwrap();
    settle(&id);
    (id, reviewed)
}
fn count(w: &World, what: &str) -> usize { az_log(w).matches(what).count() }

#[test]
fn a_yes_votes_approve_and_sets_the_auto_complete_squashed_with_its_source_branch_deleted_once_per_approval() {
    let w = az_world(json!({}));
    let (id, reviewed) = approve(&w, "Votes", json!({ "completeFail": "The pull request needs a reviewer's approval.", "policies": [resetting()] }));
    let log = az_log(&w);
    assert!(log.contains("repos pr set-vote --id 42 --vote approve"), "{log}");
    assert!(log.contains("repos pr update --id 42 --auto-complete true --squash true --delete-source-branch true --merge-commit-message Votes"), "{log}");
    let t = task(&id);
    assert!(t.at("azureVote").b("voted") && t.at("azureVote").b("autoComplete") && t.at("azureVote").s("head") == reviewed, "{}", t.at("azureVote"));
    // looked at again on the poll: not voted again for the same approval
    poll_prs();
    settle(&id);
    assert_eq!((count(&w, "set-vote"), count(&w, "--auto-complete true")), (1, 1), "{}", az_log(&w));
    // completed by Azure DevOps itself, once the vote is there: merged, waiting for its main pipeline
    w.az_says(json!({ "head": reviewed, "status": "completed", "policies": [] }));
    poll_prs();
    let t = settle(&id);
    assert!(t.b("prMerged") && t.b("pipelineWait"), "{} {}", t.s("msg"), t.at("failure"));
}

#[test]
fn a_yes_while_the_build_still_runs_votes_at_once_and_the_completion_follows_when_it_passes() {
    let w = az_world(json!({ "merge.method": "merge" }));
    let (id, reviewed) = approve(&w, "Early yes", json!({ "policies": [policy("Build", "CI build", "running", Some(7))] }));
    assert_eq!(task(&id).s("msg"), "Waiting for Azure DevOps checks · 0 of 1 passed · Approved here and in Azure");
    assert!(az_log(&w).contains("repos pr set-vote --id 42 --vote approve"), "voted while it runs: {}", az_log(&w));
    assert!(az_log(&w).contains("--auto-complete true --squash false --delete-source-branch true"), "a merge commit: {}", az_log(&w));
    w.az_says(json!({ "head": reviewed, "policies": [policy("Build", "CI build", "approved", Some(7))] }));
    poll_prs();
    let t = settle(&id);
    assert!(t.b("prMerged"), "{} {}", t.s("msg"), t.at("failure"));
    assert_eq!(count(&w, "set-vote"), 1, "{}", az_log(&w));
}

#[test]
fn a_vote_that_cannot_satisfy_the_policy_says_who_still_has_to_vote_on_the_row_in_details_and_in_the_panel() {
    let w = az_world(json!({}));
    let says = json!({ "completeFail": "The pull request needs approvals.", "reviewers": [reviewer("Me", "me@acme.test", 10), reviewer("Bob", "bob@acme.test", 0)], "policies": [reviewers_policy(1, false)] });
    let (id, _) = approve(&w, "Own changes", says);
    let words = "Approved here and in Azure · waits for 1 more reviewer: Bob (policy: Minimum number of reviewers; the creator's own vote does not count)";
    assert_eq!(task(&id).s("msg"), words);
    let d = finish_line::details(&id).unwrap();
    assert_eq!(d.at("azure").s("approved"), words, "{}", d.at("azure"));
    assert!(d.at("azure").at("vote").b("voted"), "{}", d.at("azure"));
    poll_prs();
    let t = task(&id);
    assert_eq!(t.s("msg"), words, "said the same on the poll");
    assert_eq!(t.s("prRemote"), "Reviewers: Me approved, Bob no vote · Waits for 1 more reviewer: Bob (policy: Minimum number of reviewers; the creator's own vote does not count)");
    let mut b = common::board::open(&w.root, 44);
    b.s.sel = id.clone(); b.load();
    let screen = b.screen(200).join("\n");
    assert!(screen.contains("Bob no vote") && screen.contains("Waits for 1 more"), "{screen}");
}

#[test]
fn a_vote_azure_devops_refused_is_said_and_the_board_still_completes_it_once_the_policies_pass() {
    let w = az_world(json!({}));
    let (id, reviewed) = approve(&w, "Refused vote", json!({ "voteFail": "TF401289: You may not vote on this pull request.", "autoFail": "TF401181: auto-complete is not allowed.", "completeFail": "needs a reviewer", "policies": [resetting()] }));
    let t = task(&id);
    assert_eq!(t.s("msg"), "Approved here; Azure DevOps did not take your vote (TF401289: You may not vote on this pull request.) · waits for 1 more reviewer (policy: Minimum number of reviewers)");
    assert!(!t.at("azureVote").b("autoComplete") && t.at("azureVote").s("autoSaid").contains("TF401181"), "{}", t.at("azureVote"));
    // someone else approved: the board's own completion, as before
    w.az_says(json!({ "head": reviewed, "policies": [policy("Minimum number of reviewers", "", "approved", None)] }));
    poll_prs();
    assert!(settle(&id).b("prMerged"));
}

#[test]
fn a_rebase_is_completed_by_the_board_alone_as_az_cannot_set_it_as_an_auto_complete() {
    let w = az_world(json!({ "merge.method": "rebase" }));
    let (id, _) = approve(&w, "Rebased", json!({ "policies": [] }));
    assert!(az_log(&w).contains("set-vote") && !az_log(&w).contains("--auto-complete"), "{}", az_log(&w));
    let sent = w.az.sent.borrow().iter().find(|s| s.s("resource") == "pullRequests").cloned().unwrap_or(Value::Null);
    assert_eq!(sent.at("body").at("completionOptions").s("mergeStrategy"), "rebase", "{sent}");
    assert!(task(&id).b("prMerged"));
}

#[test]
fn an_auto_complete_is_called_off_once_the_pull_request_has_commits_that_were_not_reviewed() {
    let w = az_world(json!({}));
    let (id, _) = approve(&w, "Pushed after", json!({ "completeFail": "needs a reviewer", "policies": [resetting()] }));
    assert!(task(&id).at("azureVote").b("autoComplete"), "{}", task(&id).at("azureVote"));
    // someone pushed to it
    let other = git(&w.root, &["rev-parse", "main"]);
    git(&w.root, &["push", "-q", "--force", "origin", &format!("main:refs/heads/{}", task(&id).s("branch"))]);
    w.az_says(json!({ "head": other, "policies": [policy("Minimum number of reviewers", "", "queued", None)] }));
    poll_prs();
    let t = task(&id);
    assert_eq!(t.s("msg"), "Pull request has commits that were not reviewed");
    assert!(az_log(&w).contains("repos pr update --id 42 --auto-complete false"), "{}", az_log(&w));
    assert!(az_log(&w).contains("repos pr set-vote --id 42 --vote reset"), "your vote is taken back: {}", az_log(&w));
    assert!(!t.at("azureVote").b("autoComplete") && !t.at("azureVote").b("voted"), "{}", t.at("azureVote"));
    // asked again on the next poll: nothing more to take back
    poll_prs();
    assert_eq!((count(&w, "--vote reset"), count(&w, "--auto-complete false")), (1, 1), "{}", az_log(&w));
}

#[test]
fn no_auto_complete_is_set_where_a_push_could_be_completed_before_the_board_looks_again() {
    let w = az_world(json!({}));
    // only a number of reviewers, its votes kept on a push: Azure DevOps would complete an unreviewed push at once
    let (id, reviewed) = approve(&w, "No build", json!({ "completeFail": "needs a reviewer", "policies": [policy("Minimum number of reviewers", "", "queued", None)] }));
    assert!(az_log(&w).contains("--vote approve") && !az_log(&w).contains("--auto-complete"), "{}", az_log(&w));
    assert!(task(&id).at("azureVote").s("autoSkipped").contains("the board completes it at the reviewed commit"), "{}", task(&id).at("azureVote"));
    // the board completes it, at the reviewed commit, once the vote is there
    w.az_says(json!({ "head": reviewed, "policies": [policy("Minimum number of reviewers", "", "approved", None)] }));
    poll_prs();
    assert!(settle(&id).b("prMerged"));
    let sent = w.az.sent.borrow().iter().find(|s| s.s("resource") == "pullRequests").cloned().unwrap_or(Value::Null);
    assert_eq!(sent.at("body").at("lastMergeSourceCommit").s("commitId"), reviewed, "{sent}");
}

#[test]
fn a_yes_while_the_build_runs_says_on_the_row_that_it_was_given_in_azure_and_who_still_has_to_vote() {
    let w = az_world(json!({}));
    let (id, reviewed) = approve(&w, "Row says", json!({ "reviewers": [reviewer("Me", "me@acme.test", 10)], "policies": [policy("Build", "CI build", "running", Some(7)), reviewers_policy(2, false)] }));
    let words = "Waiting for Azure DevOps checks · 0 of 1 passed · Approved here and in Azure · waits for 2 more reviewers (policy: Minimum number of reviewers; the creator's own vote does not count)";
    assert_eq!(task(&id).s("msg"), words);
    w.az_says(json!({ "head": reviewed, "reviewers": [reviewer("Me", "me@acme.test", 10), reviewer("Ann", "ann@acme.test", 10)], "policies": [policy("Build", "CI build", "running", Some(7)), reviewers_policy(2, false)] }));
    poll_prs();
    assert_eq!(task(&id).s("msg"), words.replace("2 more reviewers", "1 more reviewer"), "follows each vote on the poll");
}

#[test]
fn what_a_policy_of_people_still_waits_for_is_said_with_its_name_and_who() {
    let mut required = policy("Required reviewers", "Shop owners", "queued", None);
    required["configuration"]["settings"]["requiredReviewerIds"] = json!(["team"]);
    let team = json!({ "displayName": "Shop Team", "uniqueName": "team", "id": "team", "vote": 0, "isContainer": true, "isRequired": true });
    let ann = reviewer("Ann", "ann@acme.test", 10);
    let all = [required, reviewers_policy(2, true), policy("Comment requirements", "", "rejected", None), policy("Minimum number of reviewers", "", "approved", None)];
    assert_eq!(waits::of(&all, &[team, ann, reviewer("Bob", "bob@acme.test", 0), reviewer("Cy", "cy@acme.test", 0)], "me@acme.test", "me@acme.test"), vec![
        "waits for Shop Team (policy: Shop owners)".to_string(),
        "waits for 1 more reviewer: Bob or Cy (policy: Minimum number of reviewers)".to_string(),
        "waits for its comments to be resolved (policy: Comment requirements)".to_string(),
    ]);
    let mut optional = reviewers_policy(1, false);
    optional["configuration"]["isBlocking"] = json!(false);
    assert!(waits::of(&[optional], &[], "", "").is_empty(), "one not blocking never holds it up");
}
