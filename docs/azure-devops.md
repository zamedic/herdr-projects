# Azure DevOps

This fork of Shepherdr also merges tasks through pull requests on Azure DevOps Repos. A repo whose remote (`origin`,
else its first) is at Azure DevOps goes the way a GitHub repo does: when the task says ready its branch is pushed and
its pull request opened, your Accept merges it once its policies pass, and meanwhile it is watched. After the merge,
the task waits for the base branch's pipeline: it is done only once that run passed for the merge commit. GitHub
repos work exactly as upstream's.

herdr-task knows the host from the remote's URL, in every form Azure DevOps gives a clone:
`git@ssh.dev.azure.com:v3/<org>/<project>/<repo>`, `https://<org>@dev.azure.com/<org>/<project>/_git/<repo>`, and
the older `https://<org>.visualstudio.com/<project>/_git/<repo>` and `<org>@vs-ssh.visualstudio.com:v3/…`.

## What it needs

The Azure CLI with its azure-devops extension, logged in, on the machine where the folder lives:

```sh
az extension add --name azure-devops
az login
```

Without it, the task says so (`Couldn't open a pull request in Azure DevOps: …`, with what to run) and needs you.
The completion is Azure DevOps' REST call through `az rest`, which needs `az login`: a token from `az devops login`
alone does not do for it.

## Where it lives

| What | Where |
|---|---|
| The repo from the remote's URL, the `az` program, a pull request read in gh's shape (its policies as checks, who still has to vote), votes, the completion | `mods/tasks/bin/herdr_task/azure.py` |
| A build's timeline and failed log, stage approvals, the pull request's own pipeline, the base branch's runs | `mods/tasks/bin/herdr_task/azure_pipeline.py` |
| The merge on Accept, an accepted completion watched, the main pipeline's gate, `herdr-task pr-view` | `mods/tasks/bin/herdr_task/azure_flow.py` |
| Where GitHub's way hands over to it | `github.py` (`github`, `pr_open`, `pr_view`, `pr_problem`), `commands.py` (`merge_now`), `tick.py` (`facts`), `ghnews.py` (`pr_news`) |
| The Tasks panel (Merging's line, Needs you for a stage approval) and the Pull request view | `mods/tasks/main.js`, `agents.py` (`task_status`), `mods/task-pr/main.js` |
| The tests | `test/test_azure_devops.py` |

## The pull request

- **Opened ready for review, never as a draft** (`az repos pr create --draft false`), into the task's base branch,
  with the same title and description as on GitHub: its pipeline (build validation, a deployment to staging) runs from
  the moment it exists. One the agent opened itself for the branch is taken on instead. Its number is written `!7`.
- **Its policies are its checks.** Each build validation and status check is a check, as GitHub's are: running,
  passed, failed (with the failing job, step and the end of its log sent to the resting agent as news, as a failed
  GitHub check is). One that is not blocking never holds the merge up. A policy of people not met yet (minimum number
  of reviewers, required reviewers, resolved comments, a linked work item) is waited for, never a problem: the task
  stays **Ready** to accept, and `herdr-task show` says who still has to vote (`in Azure DevOps: Reviewers: Me
  approved, Shop Team no vote (required) · Waits for Shop Team (policy: Required reviewers)`). A reviewer who rejects
  it or waits for the author needs you.
- **Its own pipeline** is kept on the task as `pr_pipeline`: `Pull request open · pipeline running · 1 of 3 stages`,
  `… · deploying Staging`, `… · deployed to Staging · pipeline running`, `… · pipeline passed`.
- **Held at a stage's approval** (an environment's check, `Checkpoint.Approval` under way) before your Accept: the task
  shows under **Needs you**, `Pipeline CI waits for your approval of stage Staging in Azure DevOps: <run>`, and the
  Accept is still yours to give (`herdr-task accept`, or ask the Assistant). After it, the Merging line says the same.
- **Comments people write** on it (not Azure DevOps' own: votes, pushes) reach the resting agent as news, as on
  GitHub. herdr-task's own summary comment is a thread closed at once, so it never holds up a policy of resolved
  comments.
- **The Pull request view** asks `herdr-task pr-view <task> --json` for it (az has no `gh pr view`).

## Your Accept

Accepting a Ready task (or a task that merges on its own, when it gets there):

1. Only the commit that was reviewed may merge: a pull request that names another commit (someone pushed to it) needs
   you, and nothing is completed.
2. A draft is made ready.
3. **Your vote is cast in Azure DevOps**: the Approve vote of the user az is logged in as (`az repos pr set-vote
   --vote approve`), at once, so also while its build still runs. Its **auto-complete** is set too (`az repos pr update
   --auto-complete true`, squash or merge, the source branch deleted), but only where a push can't be completed before
   herdr-task looks again: a blocking build validation, or a policy of reviewers that resets the votes on a push.
   Azure DevOps' auto-complete completes whatever was pushed last, never only the reviewed commit. az can't set a
   rebase. Both are given once per Accept and reviewed commit (`azure_vote` on the task), and taken back
   (`--auto-complete false`, `--vote reset`) as soon as that Accept no longer stands: a push that wasn't reviewed here,
   the task needing you, or no longer merging.
4. Its policies still running, or people still to vote: it shows under **Merging** (`Waiting for Azure DevOps checks ·
   0 of 1 passed · Approved here and in Azure DevOps · waits for 1 more reviewer: Bob (policy: …)`) and merges by
   itself once they pass.
5. **Completed by its merge strategy at the reviewed commit**: `PATCH …/pullRequests/<n>` through `az rest`, with
   `lastMergeSourceCommit` the reviewed commit and `completionOptions.mergeStrategy` from Settings (`squash`;
   `merge`, a merge with no fast-forward; `rebase`; `rebase-merge`), named whatever strategy the pull request already
   had, its source branch deleted.
6. **Accepted is not completed**: Azure DevOps may take the completion and still be merging it. The task stays
   Merging (`Approved: Azure DevOps completes it`) and is watched: a conflict, a policy that rejects the merge or a
   failed merge sends it back to you with the reason, and so does one still not completed after `complete_timeout`
   minutes. Completed there meanwhile, it goes on by itself.

A pull request **completed in Azure DevOps** (by its auto-complete, or by you) is merged work: it is never compared
with the reviewed commit again or taken for an unreviewed push, and the task goes on to its main pipeline. Abandoned,
it needs you, as a closed pull request does on GitHub.

## The main pipeline

After the merge the task stays under **Merging** while the base branch's pipeline runs for the merge commit (Azure
DevOps' `lastMergeCommit`), its line saying how far: `Main pipeline running · 2 of 3 stages · Lint skipped`, or
`Deploying Staging` while a stage named `Deploy…` runs. Only the runs of that very commit count, the newest of each
pipeline; with no run found, the pull request's merge commit is read again in case Azure DevOps named its trial merge
at first.

- **Passed** (or partly succeeded): done, and its worktree and branch cleaned up as any merged task's.
- **Failed or cancelled**: needs you, with the run, and the failing job, step and the end of its log kept on the task
  (`pipeline_failed`). `herdr-task fix` sends it to the agent: it rebases its branch onto the base, fixes it and says
  ready again, which opens a new pull request. Run again in Azure DevOps and passed, it is done by itself.
- **Held at a stage's approval**: needs you, `Main pipeline CI waits for your approval of stage prodDE in Azure DevOps:
  <run>`, with no timeout; approved there, it goes on by itself, through each stage's approval in turn.
- **An approval rejected there** (or one that ran out): needs you, `Main pipeline stopped: …`, nothing for the agent to
  fix.
- **Not started** after `pipeline_timeout` minutes (counted from your last approval there), or queued all that time
  with no job on an agent: needs you. A run under way is never cut off.
- A repo with no pipeline is done at once; with `pipeline` at `all` and no run for the commit after 10 minutes, done.

Work the agent put on the base branch itself (no pull request; its branch is in the base here) waits the same way for
the run of the commit that brought it there (`On main: bdb73e8. Waiting for the main pipeline to start`): its own
commit when it is on the base's first-parent line, else the merge that brought it in.

## Settings

In `~/.herdr/shepherdr.json`, under `azure` (Everywhere's, a machine's or a space's, as the other task settings):

| Key | Default | Means |
|---|---|---|
| `method` | `squash` | How the pull request completes: `squash`, `merge` (no fast-forward), `rebase`, `rebase-merge` |
| `pipeline` | `all` | Which runs of the base branch a merged task waits for: `all`, `none` (done at the merge), or pipelines by name or id (`CI, deploy`) |
| `pipeline_timeout` | `60` | Minutes a pipeline may take to start (or sit queued) before the task needs you |
| `complete_timeout` | `30` | Minutes an accepted completion may take before the task needs you |

There is no row for them in Shepherdr settings' window yet.
