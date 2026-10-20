//! `merge.draft`: a pull request on Azure DevOps opens ready for review, so that its pipeline (build validation, a
//! deployment to staging) runs before your yes, and the yes only completes it; on GitHub it stays a draft until the
//! yes. The setting moves either host, per task, repo or in general. While it is open, the row and `details` say
//! what its pipeline is doing: running, deploying, passed, deployed.
use crate::azure_devops_flow::{az_world, stages};
use crate::common;
use common::az::policy;
use common::*;
use serde_json::{json, Value};
use std::path::Path;
use sw::actions::finish_line;
use sw::azure::{draft, pr_pipeline, runs::Stage};
use sw::housekeeping::poll_prs;
use sw::json::*;
use sw::store::settings::change::set_value;
use sw::store::settings::lookup::{setting_text, Scope};

#[test]
fn merge_draft_is_auto_by_default_the_hosts_way_and_set_per_task_repo_or_in_general() {
    assert!(!draft("auto", true) && draft("auto", false), "auto: ready on Azure DevOps, a draft on GitHub");
    assert!(draft("true", true) && draft("true", false) && !draft("false", true) && !draft("false", false));
    let w = world_with(Opts { repo: true, ..Default::default() });
    let id = w.add("One");
    let on = || setting_text("merge.draft", Some(&task(&id)));
    assert_eq!(on(), "auto");
    set_value("merge.draft", &json!("true"), &Scope::you()).unwrap(); assert_eq!(on(), "true");
    set_value("merge.draft", &json!("false"), &Scope::repo("proj")).unwrap(); assert_eq!(on(), "false");
    set_value("merge.draft", &json!(true), &Scope::task(&task(&id))).unwrap(); assert_eq!(on(), "true", "a switch's word works too");
    let e = set_value("merge.draft", &json!("maybe"), &Scope::you()).unwrap_err();
    assert!(e.msg.contains("merge.draft must be auto, true, false"), "{}", e.msg);
}

#[test]
fn on_azure_devops_it_opens_ready_and_the_yes_only_completes_it() {
    let w = az_world(json!({ "merge.pipeline": "none" }));
    let id = to_approval(&w, "Ready at once");
    assert!(az_log(&w).contains("--draft false"), "{}", az_log(&w));
    let head = git(Path::new(task(&id).s("worktree")), &["rev-parse", "HEAD"]);
    w.az_says(json!({ "head": head }));
    finish_line::approve(&id, None, false).unwrap();
    let t = settle(&id);
    assert_eq!(t.s("stage"), "done", "{} {}", t.s("msg"), t.at("failure"));
    assert!(!az_log(&w).contains("repos pr update"), "never undrafted: {}", az_log(&w));
    let sent = w.az.sent.borrow().iter().find(|s| s.s("resource") == "pullRequests").cloned().unwrap_or(Value::Null);
    assert_eq!(sent.at("body").s("status"), "completed", "{sent}");
}

#[test]
fn merge_draft_true_on_azure_devops_opens_a_draft_made_ready_on_the_yes() {
    let w = az_world(json!({ "merge.pipeline": "none", "merge.draft": "true" }));
    let id = to_approval(&w, "As a draft");
    assert!(az_log(&w).contains("--draft true"), "{}", az_log(&w));
    let head = git(Path::new(task(&id).s("worktree")), &["rev-parse", "HEAD"]);
    w.az_says(json!({ "head": head, "draft": true }));
    finish_line::approve(&id, None, false).unwrap();
    let t = settle(&id);
    assert_eq!(t.s("stage"), "done", "{} {}", t.s("msg"), t.at("failure"));
    assert!(az_log(&w).contains("repos pr update --id 42 --draft false"), "made ready: {}", az_log(&w));
}

#[test]
fn merge_draft_false_on_github_opens_it_ready_for_review() {
    let w = world_with(Opts { repo: true, origin: true, settings: json!({ "review.when": "never", "merge.draft": "false" }), ..Default::default() });
    let id = to_approval(&w, "Ready on GitHub");
    assert!(gh_log(&w).contains("pr create --head work/1-ready-on-github --title Ready on GitHub") && !gh_log(&w).contains("--draft"), "{}", gh_log(&w));
    assert_eq!(task(&id).s("pr"), "https://github.com/o/r/pull/7");
    // GitHub says it is ready already: the yes does not make it ready again
    let t = task(&id);
    w.gh_says(json!({ "view": { "state": "OPEN", "isDraft": false, "headRefOid": t.at("prHead"), "statusCheckRollup": [], "comments": [], "reviews": [], "author": { "login": "me" } } }));
    finish_line::approve(&id, None, false).unwrap();
    settle(&id);
    assert!(gh_log(&w).contains("pr merge") && !gh_log(&w).contains("pr ready"), "{}", gh_log(&w));
}

#[test]
fn an_optional_build_validation_still_running_is_running_and_says_deployed_once_through() {
    let w = az_world(json!({}));
    let id = to_approval(&w, "Optional deploy");
    let head = git(Path::new(task(&id).s("worktree")), &["rev-parse", "HEAD"]);
    let optional = |status: &str| { let mut p = policy("Build", "Deploy to staging", status, Some(7)); p["configuration"]["isBlocking"] = json!(false); p };
    w.az_says(json!({ "head": head, "policies": [optional("running")], "timeline": { "7": stages(&[("Build", "completed"), ("Deploy staging", "inProgress")]) } }));
    poll_prs();
    assert_eq!(task(&id).at("prPipeline").s("words"), "Pull request open · deploying staging", "{}", task(&id).at("prPipeline"));
    let mut done = stages(&[("Build", "completed"), ("Deploy staging", "completed")]);
    for r in done["records"].as_array_mut().unwrap() { r["result"] = json!("succeeded"); }
    w.az_says(json!({ "head": head, "policies": [optional("approved")], "timeline": { "7": done } }));
    poll_prs();
    assert_eq!(task(&id).at("prPipeline").s("words"), "Pull request open · deployed to staging", "{}", task(&id).at("prPipeline"));
}

#[test]
fn the_rows_words_say_how_far_its_pipeline_is() {
    let st = |n: &str, done: bool, running: bool| Stage { name: n.into(), done, running, ..Default::default() };
    assert_eq!(pr_pipeline::words("", &[]), "Pull request open");
    assert_eq!(pr_pipeline::words("running", &[]), "Pull request open · pipeline running");
    assert_eq!(pr_pipeline::words("running", &[st("Build", true, false), st("Test", false, true), st("Deploy_Staging", false, false)]), "Pull request open · pipeline running · 1 of 3 stages");
    assert_eq!(pr_pipeline::words("running", &[st("Build", true, false), st("Deploy_Staging", false, true)]), "Pull request open · deploying Staging");
    assert_eq!(pr_pipeline::words("running", &[Stage { name: "Staging".into(), running: true, deploying: true, ..Default::default() }]), "Pull request open · deploying Staging");
    assert_eq!(pr_pipeline::words("passed", &[st("Build", true, false)]), "Pull request open · pipeline passed");
    assert_eq!(pr_pipeline::words("passed", &[st("Build", true, false), Stage { name: "Deploy staging".into(), done: true, deployed: true, ..Default::default() }]), "Pull request open · deployed to staging");
    assert_eq!(pr_pipeline::words("failed", &[]), "Pull request open · pipeline failed");
    // production still waits (held at an approval): staging is there to try already
    let staged = Stage { name: "Deploy_Staging".into(), done: true, deployed: true, ..Default::default() };
    assert_eq!(pr_pipeline::words("running", &[st("Build", true, false), staged, st("Deploy_Prod", false, true)]), "Pull request open · deploying Prod");
    let staged = Stage { name: "Deploy_Staging".into(), done: true, deployed: true, ..Default::default() };
    assert_eq!(pr_pipeline::words("running", &[st("Build", true, false), staged, st("Production", false, false)]), "Pull request open · deployed to Staging · pipeline running");
    // a stage named for a deployment that was skipped deployed nothing
    let tl = json!({ "records": [{ "id": "s", "type": "Stage", "name": "Deploy_Staging", "state": "completed", "result": "skipped", "order": 1 }] });
    assert!(!sw::azure::runs::stages_of(&sw::azure::logs::records_of(&tl))[0].deployed);
}

#[test]
fn while_it_waits_for_your_yes_the_row_and_details_say_its_pull_request_is_open_and_its_pipeline_runs_then_deployed() {
    let w = az_world(json!({}));
    let id = to_approval(&w, "Try on staging");
    let head = git(Path::new(task(&id).s("worktree")), &["rev-parse", "HEAD"]);
    let row = || sw::shown::row_says::sentence(&sw::flow::live::board(true).into_iter().find(|r| r.s("id") == id).unwrap()).text();
    w.az_says(json!({ "head": head, "policies": [policy("Build", "CI build", "running", Some(7))], "timeline": { "7": stages(&[("Build", "completed"), ("Deploy staging", "inProgress")]) } }));
    poll_prs();
    assert_eq!(task(&id).at("prPipeline").s("words"), "Pull request open · deploying staging");
    assert!(row() == "Pull request open · deploying staging", "{}", row());
    let d = finish_line::details(&id).unwrap();
    assert_eq!((d.at("azure").s("prPipeline"), d.at("azure").at("draft")), ("Pull request open · deploying staging", &json!(false)), "{}", d.at("azure"));
    let mut done = stages(&[("Build", "completed"), ("Deploy staging", "completed")]);
    for r in done["records"].as_array_mut().unwrap() { r["result"] = json!("succeeded"); }
    w.az_says(json!({ "head": head, "policies": [policy("Build", "CI build", "approved", Some(7))], "timeline": { "7": done } }));
    poll_prs();
    let t = task(&id);
    assert_eq!((t.s("stage"), t.at("prPipeline").s("words")), ("approval", "Pull request open · deployed to staging"));
    assert!(row() == "Pull request open · deployed to staging", "{}", row());
    // looked at again with nothing changed: its timeline is not read again
    let reads = || az_log(&w).matches("--resource timeline").count();
    let before = reads();
    poll_prs();
    assert_eq!(reads(), before, "{}", az_log(&w));
}
