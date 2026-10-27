# Azure DevOps

A repo whose origin is at Azure DevOps Repos goes through the finish line as one on GitHub does: its branch is pushed,
its pull request opened (ready for review, so its pipeline runs at once), merged on your yes (or by itself) once its policies pass, and watched meanwhile. Then it
waits for the base branch's pipeline: the task is done only once that run passed for the merge commit.

The board knows the host from the origin's URL, in any form Azure DevOps gives a clone:

- `git@ssh.dev.azure.com:v3/<org>/<project>/<repo>`
- `https://<org>@dev.azure.com/<org>/<project>/_git/<repo>`
- the older `https://<org>.visualstudio.com/<project>/_git/<repo>` and `<org>@vs-ssh.visualstudio.com:v3/…`

Anything else is GitHub, as before.

## What it needs

The Azure CLI with its azure-devops extension, logged in, where the board runs:

```sh
az extension add --name azure-devops
az login
```

Without it (no `az`, no extension, no login) the task says so as a login the human must give, the way a missing gh
login is said: `Could not open the pull request`, with what az said and what to run.

## The pull request

- **Opened** at the finish line: the branch pushed, a pull request into the task's base branch (`workspace.from`),
  with the same title and description as on GitHub, **ready for review, not a draft** (`az repos pr create --draft
  false`). Its pipeline (build validation, a deployment to staging) runs from the moment it exists, so the work can be
  tried on staging before your yes. `merge.draft` changes it: `true` opens a draft here too, made ready on your yes
  (`auto`, the default, is ready on Azure DevOps and a draft on GitHub). One the agent opened itself for the branch is
  taken on instead.
- **Its pipeline, while it waits for your yes**: the row says that the pull request is open and how far its own
  pipeline is, from its build validations and their stages: `Pull request open · pipeline running · 1 of 3 stages`,
  `… · deploying Staging`, `… · deployed to Staging` (a stage named `Deploy…`, or one whose `Deploy…` job succeeded),
  `… · deployed to Staging · pipeline running` (a later stage still waits, such as production at an approval),
  `… · pipeline passed`, `… · pipeline failed`. An optional build validation (not blocking) counts here until it is
  through, though it never holds the merge up. `details` says the same (`azure.prPipeline`), with `azure.draft`.
- **Shown** on the row (`PR#42`, green when its policies pass, red when one fails), by its link
  (`https://dev.azure.com/<org>/<project>/_git/<repo>/pullrequest/42`). The task panel adds a line of its reviewers'
  votes and its policies (`Azure  Reviewers: Ann approved · Policies: CI build running`); `details` has the pull
  request as Azure DevOps says it now (`azure`: its status, reviewers and policies) and, after the merge, the main
  pipeline's runs.
- **Merged** on approve: only at the commit that was reviewed, made ready if it is a draft (a pull request that is
  ready already is left as it is), its policies passed, then completed by
  `merge.method` (`squash`; `merge` is a merge with no fast-forward; `rebase`), its source branch deleted. The strategy
  is named on the completion, whatever one the pull request already had (an auto-complete set in Azure DevOps). It is
  Azure DevOps' REST call through `az rest` (`az repos pr update` can say neither the strategy nor the commit), so it
  needs `az login`: a token from `az devops login` alone does not do for it.
- **Your yes in Azure DevOps too**: see [below](#your-yes-in-azure-devops).
- **Completion accepted, not carried out**: Azure DevOps may accept the completion and still be merging it. The task
  waits, watching the pull request's merge status: a conflict, a policy that rejects the merge or a failed merge sends
  it back to you with the reason (a conflict is the agent's to fix), and a pull request still active after
  `check.timeout` waits for you too, saying so. Completed in Azure DevOps meanwhile, it goes on by itself.
- **Completed by you in Azure DevOps**: the task is merged without the board's own merge, and needs no clean checkout.
  **Abandoned** while its merge was awaited: it waits for you, as a closed pull request does on GitHub.
- **Merged by the agent itself**: `Already merged in !1234`, done once the main pipeline of its merge commit passed.
  Only a pull request whose last commit holds the work counts: one of the branch merged before the work was done is not
  what brought it to main.
- **Pushed to main by the agent itself**: `Already on main`, and not done yet either: the task waits for the main
  pipeline of the commit that brought the work there (`On main: bdb73e8. Waiting for the main pipeline to start`): the
  agent's own commit when it was pushed as it is, else the first commit of main's line that holds it; when main holds
  its changes as another commit (cherry-picked, or squashed there while the branch keeps its own), main's tip.
- **Feedback**: what you write on it (as the user `az` is logged in as) wakes the agent, as on GitHub. The board's own
  review comment is a thread closed at once, so it never holds up a policy of resolved comments.

The pull requests are looked at on the same cadence as GitHub's, every two minutes.

## Your yes in Azure DevOps

Approving a task (`approve <id>`, or yes on the board) whose pull request is at Azure DevOps gives that yes there as
well, at once, before its policies are looked at, so also while its build still runs:

- **Your vote**: the Approve vote of the user `az` is logged in as (`az repos pr set-vote --id <pr> --vote approve`).
- **Its auto-complete**: set by `merge.method` with its source branch deleted and the board's commit message (`az repos
  pr update --auto-complete true --squash true|false --delete-source-branch true --merge-commit-message …`), so Azure
  DevOps completes it the moment its policies are met. Azure DevOps' auto-complete completes whatever was pushed last,
  never only the reviewed commit, so it is set only where a push cannot be completed before the board looks again: the
  pull request has a blocking build validation (a new commit needs a new build) or a blocking policy of reviewers that
  resets the votes on a push. Elsewhere (only a number of reviewers whose votes stay) no auto-complete is set
  (`azure.vote.autoSkipped` says so) and the board completes it at the reviewed commit. az cannot set a rebase either:
  with `merge.method: rebase` the board completes it itself, as it does whenever the auto-complete could not be set.
- **The board's own completion stays**: once its policies pass the board completes it at the reviewed commit, as
  before; completed by Azure DevOps first, the task is taken for merged and goes on to the main pipeline.

A task that merges by itself (`merge.by itself`) gives them the same way when the board takes it to the merge. Both
are given once per approval and reviewed commit (`azureVote` on the task, in `details` as `azure.vote`). A vote
Azure DevOps refused is said, with its words: `Approved here; Azure DevOps did not take your vote (TF401289: …)`. The
auto-complete is called off (`--auto-complete false`) and your vote taken back (`set-vote --vote reset`) as soon as
that approval no longer stands: the merge failed, its checks failed and went back to the agent, the task is no longer
merging, or someone pushed to the pull request a commit that was not reviewed here. Your next yes gives both again.

When your vote cannot meet a policy of people by itself, the row, `details` (`azure.approved`, `azure.waits`) and the
panel say what it still waits for, with the policy's name and who still has to vote, and the row follows each vote;
while its build still runs the row says it after the checks (`Waiting for Azure DevOps checks · 0 of 1 passed ·
Approved here and in Azure · waits for 1 more reviewer (…)`):

- `Approved here and in Azure · waits for 1 more reviewer: Bob (policy: Minimum number of reviewers)`
- `… · waits for 1 more reviewer (policy: Minimum number of reviewers; the creator's own vote does not count)`: you
  opened the pull request, and the policy does not let the creator approve their own changes
- `… · waits for Shop Team (policy: Required reviewers)`
- `… · waits for its comments to be resolved (policy: Comment requirements)`, `… · waits for a linked work item`

The panel's `Azure` line lists the pull request's reviewers with their votes, the required ones (those a policy adds)
marked so: `Reviewers: Me approved, Shop Team no vote (required) · Waits for Shop Team (policy: Required reviewers)`.
A yes to a task held at an approval only Azure DevOps can give (a build at its environment's check) says where to give
it, and what your vote, given already, still waits for.

## Where it stands while it waits for your yes

A task whose work is finished and whose review passed is **ready to approve** (the green `◆`, counted as `N ready` in
the list's header), exactly as on GitHub, while its pull request's pipeline runs, passes or deploys to staging: the row
says the review's result and the pipeline (`Pass · 1 minor · Pull request open · pipeline running`), `herdr-projects
list` says `ready to approve`. A policy of people still to be met (`a review is still required`) does not change that:
your yes then waits for it in Azure DevOps.

It **needs you** only for a real hold: a failed review, a failed merge or sync, a policy that failed, a reviewer who
rejected it or waits for the author, or a build or pipeline held at an approval that only Azure DevOps can give (the row
says which, and yes says to approve it there).

Its pull request's own pipeline held at an approval before a stage (the run's timeline has a `Checkpoint.Approval`
under way) needs you too, though nothing failed: the row says `Pass · Pull request open · waits for your approval of
stage Staging in Azure DevOps`, the panel's `Approve` line names the run with its URL, the approval question stays and
your yes is still yours to give (it then waits for that approval there, and for the policies). A failure of an earlier
run of its checks is cleared then, and nothing is sent to the agent. A skipped stage is said as such (`pipeline running
· 3 of 5 stages · Lint skipped`); a stage that failed is still a failed policy, with its job and the end of its log. An
earlier wait for its builds that ran out (`Azure DevOps' checks did not finish`) is cleared once the build passed. A turn of its agent that changes nothing (you typed in its pane, it
compacted its conversation) leaves it where it was: never "stopped without a summary" with the agent's screen line.

## Its policies are its checks

`merge.checks` is `auto` by default: `github` on GitHub, `azure` on Azure DevOps. With `azure`:

| Policy | The board |
|---|---|
| a build validation still running | waits for it as long as it runs (a build is never cut off by `check.timeout`): `Waiting for Azure DevOps checks · 0 of 1 passed` |
| a status check still running, or none started | waits, at most `check.timeout` |
| a build validation held at an approval (its environment's check in Azure Pipelines) | waits for you, with no timeout: `Pipeline reports waits for your approval of stage Staging in Azure DevOps: <run>`. Once you approve it there, it waits for the build again |
| one that failed | merges nothing; the failure goes back to the agent with the policy's name, the failing job and step, and the end of its log, within `review.rounds`, then waits for you (`approve <id> --anyway` merges all the same) |
| a policy of people not met (reviewers, required reviewers, resolved comments, linked work items) | waits for it, as a review still required on GitHub, saying who still has to vote ([your yes in Azure DevOps](#your-yes-in-azure-devops)); a reviewer who rejects, or waits for the author, is a change asked for |
| one that is not blocking | never holds the merge up |

`merge.checks: none` does not look at them.

## The main pipeline

After the merge, the task stays at "merging" while the base branch's pipeline (`azure-pipelines.yml`, or whichever the
repo has) runs for the merge commit, its row saying how far: `Main pipeline running · 2 of 3 stages`, or
`Deploying staging` while a stage named `Deploy…` runs. Only the runs of that very commit count: an older run of the same
pipeline that passed already (of the commit before) is not the task's, and no run of the merge commit yet means waiting
for it to start. With no run found, the pull request's merge commit is read again, in case Azure DevOps named another
one at first. The task is done only once its run is through, its last stage too.

- **Passed**: the task is done and cleaned up. A run that partly succeeded counts as passed, and says so.
- **Failed or cancelled**: the task waits for you, with the run, the failing job and step and the end of its log.
  `fix` sends that to the agent; it fixes it on its branch (which has the merge in it) and finishes again, and a new
  pull request brings the fix in. `retry` looks at the runs again, after you ran the pipeline again in Azure DevOps (the
  newest run of a pipeline counts).
- **Held at an approval** in Azure DevOps (a deployment stage's environment check): the task waits for you,
  `Main pipeline reports waits for your approval of stage prodDE in Azure DevOps: <run>`, with no timeout; once
  approved there it goes on by itself, through each stage's approval in turn (`Staging`, then `Production_DE`).
- **An approval rejected** there (or one that ran out): the task waits for you, `Main pipeline stopped: The approval of
  stage Production_DE was rejected or ran out in Azure DevOps (run 104499)`, with nothing for the agent to fix. Run the
  stage again there and `retry` to wait for it again, or archive the task.
- **Running**: once a job of it has an agent, waited for as long as it runs, never cut off.
- **Not started after `merge.pipelineTimeout`** (60 minutes by default, counted from your last approval there), or
  queued all that time with no job on an agent: it waits for you with that reason.

`merge.pipeline` says which runs it waits for:

| `merge.pipeline` | Waits for |
|---|---|
| `all` (built in) | every pipeline run of the base branch for the merge commit. A repo with no pipeline is done at once; one with none that ran for the commit after 10 minutes is done, saying so |
| a name or id, or several (`CI, deploy`) | those, each until it starts and passes |
| `none` | nothing: done at the merge |

A pull request completed in Azure DevOps (by its auto-complete, or by you) is merged work. Main then moves, with the
task's own squash in it, but the board never merges main into that branch and never compares the branch's head with the
pull request's. The land step and the sync with main both ask Azure DevOps first. Completed, the task is taken for
merged at the pull request's merge commit (`lastMergeCommit`) and waits for its main pipeline as above. While it waits it
has no runner, and the board does not count that as a lost runner. A task that an earlier version left with you this way
(`Pull request has commits that were not reviewed`, after the board merged main into its merged branch) is finished on
the next poll: that merge of the board's own is undone (only a merge of main on top of what merged, in a clean
worktree), and the task waits for its main pipeline again. A push to the pull request after its review is still caught
while the pull request is open.

The gate is Azure DevOps' only: on GitHub a task is done at the merge, as before.
