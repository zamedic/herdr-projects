//! Your yes on the board, given in Azure DevOps too: the signed-in user's Approve vote on the pull request (`az repos pr
//! set-vote`), and its auto-complete by `merge.method` with its source branch deleted (`az repos pr update
//! --auto-complete`), so that Azure DevOps completes it the moment its policies are met. Cast once per approval
//! (`azureVote` on the task: land/vote_azure.rs); the auto-complete is called off when that approval no longer stands.
use super::cli::{run, said};
use super::pr::id_of;
use super::url::Azure;
use crate::flow::git::GhOut;
use std::path::Path;

/// The Approve vote of the user az is logged in as.
pub fn cast(a: &Azure, cwd: &Path, pr: &str) -> GhOut { run(a, cwd, &["repos", "pr", "set-vote", "--id", &id_of(pr), "--vote", "approve"], 60000) }
/// Whether az can set the auto-complete for `merge.method`: it says squash or a merge commit, never a rebase. Not: the
/// board's own completion (pr::complete) is all there is.
pub fn auto_completes(how: &str) -> bool { how != "rebase" }
/// Its auto-complete set: squashed or merged by `how`, its source branch deleted, with the board's commit message.
pub fn auto_complete(a: &Azure, cwd: &Path, pr: &str, how: &str, message: &str) -> GhOut {
    run(a, cwd, &["repos", "pr", "update", "--id", &id_of(pr), "--auto-complete", "true", "--squash", if how == "squash" { "true" } else { "false" }, "--delete-source-branch", "true", "--merge-commit-message", message], 60000)
}
/// Its auto-complete called off: the approval it was set for no longer stands (a push, a failure, a withdrawn yes).
pub fn call_off(a: &Azure, cwd: &Path, pr: &str) -> GhOut { run(a, cwd, &["repos", "pr", "update", "--id", &id_of(pr), "--auto-complete", "false"], 60000) }
/// The first line of what az said, for a task's message.
pub fn first_line(r: &GhOut) -> String { said(r).lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("no answer").trim_start_matches("ERROR: ").chars().take(160).collect() }
