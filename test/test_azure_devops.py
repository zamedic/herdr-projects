"""herdr-task on Azure DevOps (the fork's port of its Azure DevOps support onto Shepherdr): the
origin's URL, a pull request opened ready and read in gh's shape (policies as checks, who still has
to vote), the Accept casting the person's vote and completing at the reviewed commit by its
strategy, an accepted completion that isn't carried out, a completed pull request as merged work,
stage approvals held there, and the main pipeline's gate before done. In-process, with a fake az:
no program is started, never hc."""
import atexit
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, "mods", "tasks", "bin")
SCRATCH = tempfile.mkdtemp(prefix="herdr-task-azure-test-")
atexit.register(shutil.rmtree, SCRATCH, True)

# herdr-task reads its settings and store paths when it is imported: a test's own, then put back.
_saved = {k: os.environ.get(k) for k in ("HERDR_TASK_SETTINGS", "HERDR_TASK_STORE")}
os.environ["HERDR_TASK_SETTINGS"] = os.path.join(SCRATCH, "shepherdr.json")
os.environ["HERDR_TASK_STORE"] = os.path.join(SCRATCH, "tasks")
sys.dont_write_bytecode = True
sys.path.insert(0, BIN)
from herdr_task import azure, azure_flow, azure_pipeline, github, ghnews  # noqa: E402
from herdr_task.agents import task_status  # noqa: E402
from herdr_task.base import now  # noqa: E402
for _k, _v in _saved.items():
    if _v is None:
        os.environ.pop(_k, None)
    else:
        os.environ[_k] = _v

A = {"org": "acme", "project": "Shop", "repo": "web"}
HEAD = "a" * 40
MERGE = "m" * 40


def policy(kind, status, name=None, blocking=True, build=None, **settings):
    ty = {"build": "Build", "status": "Status", "min": "Minimum number of reviewers", "required": "Required reviewers",
          "comments": "Comment requirements"}[kind]
    s = dict(settings)
    if kind == "build":
        s["buildDefinitionId"] = 3
    if name:
        s["displayName"] = name
    return {"status": status, "configuration": {"isEnabled": True, "isBlocking": blocking, "type": {"displayName": ty}, "settings": s},
            "context": {"buildId": build} if build else {}}


def record(id, type, name, state, result="", parent=None, log=None, order=0):
    return {"id": id, "parentId": parent, "type": type, "name": name, "state": state, "result": result,
            "log": {"id": log} if log else None, "order": order}


class FakeAz:
    """Answers az as Azure DevOps would, from what the test set; every call is kept."""

    def __init__(self):
        self.calls, self.bodies = [], []
        self.pr = {"pullRequestId": 7, "status": "active", "isDraft": False, "title": "PRJ-1 Do it", "mergeStatus": "succeeded",
                   "sourceRefName": "refs/heads/work/1", "targetRefName": "refs/heads/main", "lastMergeSourceCommit": {"commitId": HEAD},
                   "createdBy": {"uniqueName": "me@acme.com"}, "reviewers": []}
        self.policies = [policy("build", "approved", name="CI", build="101")]
        self.listed = []
        self.completes = True        # a completion is carried out at once
        self.complete_fails = None   # az rest's error
        self.pipelines = [{"id": 3}]
        self.runs = []
        self.timelines = {}
        self.logs = {}
        self.threads = []

    def flag(self, args, name):
        return args[args.index(name) + 1] if name in args else None

    def route(self, args):
        r = args[args.index("--route-parameters") + 1:]
        return dict(x.split("=", 1) for x in r[: next((i for i, x in enumerate(r) if x.startswith("--")), len(r))])

    def __call__(self, args, cwd=None):
        self.calls.append(list(args))
        j = lambda v: (True, json.dumps(v), "")
        if args[:3] == ["repos", "pr", "show"]:
            return j(self.pr)
        if args[:4] == ["repos", "pr", "policy", "list"]:
            return j(self.policies)
        if args[:3] == ["repos", "pr", "create"]:
            return j({"pullRequestId": 7})
        if args[:3] == ["repos", "pr", "list"]:
            return j(self.listed)
        if args[:3] in (["repos", "pr", "set-vote"], ["repos", "pr", "update"]):
            return j({})
        if args[0] == "rest":
            with open(args[args.index("--body") + 1][1:]) as f:
                body = json.load(f)
            self.bodies.append(body)
            if self.complete_fails:
                return False, "", self.complete_fails
            if self.completes:
                self.pr.update(status="completed", lastMergeCommit={"commitId": MERGE})
            return j({})
        if args[:2] == ["devops", "invoke"]:
            res, route = self.flag(args, "--resource"), self.route(args)
            if res == "timeline":
                return j({"records": self.timelines.get(route["buildId"], [])})
            if res == "logs":
                return j({"value": self.logs.get(route["logId"], [])})
            if res == "pullRequestThreads":
                return j({"value": self.threads})
        if args[:2] == ["pipelines", "list"]:
            return j(self.pipelines)
        if args[:3] == ["pipelines", "runs", "list"]:
            return j(self.runs)
        if args[:2] == ["account", "show"]:
            return True, "me@acme.com", ""
        return False, "", f"fake az has no answer for {args}"

    def did(self, *words):
        return [c for c in self.calls if c[: len(words)] == list(words)]


class Store:
    """What azure_flow reads of a folder's locked list."""

    def __init__(self, folder):
        self.loc, self.data, self.dirty = {"folder": folder, "kind": "repo", "name": "web"}, {"tasks": []}, False

    def ref(self, t):
        return f"PRJ-{t['id']}"

    def key(self, t):
        return f"web#{t['id']}"


def report():
    return {"done": [], "attention": []}


class Base(unittest.TestCase):
    def setUp(self):
        self.az = FakeAz()
        self.patch(azure, "az", self.az)
        azure._USER.clear()
        self.finished = []

        def finish(s, t, even_if_busy=False):
            t["state"] = "done"
            t.pop("attention", None)
            self.finished.append(t["id"])
        self.patch(azure_flow, "finish", finish)
        self.patch(azure_flow, "update_main", lambda s, t: None)
        self.write_settings({})
        self.s = Store(SCRATCH)
        self.t = {"id": 1, "title": "Do it", "state": "ready", "mode": "worktree", "worktree": SCRATCH, "branch": "work/1",
                  "base": "main", "head": HEAD, "ready_at": 1000, "agents": [], "pr": azure.pr_record(A, 7)}

    def patch(self, mod, name, value):
        old = getattr(mod, name)
        setattr(mod, name, value)
        self.addCleanup(setattr, mod, name, old)

    def write_settings(self, azure_part):
        with open(os.path.join(SCRATCH, "shepherdr.json"), "w") as f:
            json.dump({"everywhere": {"azure": azure_part}}, f)

    def accept(self):
        self.t["accepted_at"] = now()
        return azure_flow.merge_now(self.s, self.t)

    def tick(self):
        self.t["pr"]["checked"] = 0
        if self.t.get("pipeline_wait"):
            self.t["pipeline_wait"]["checked"] = 0
        r = report()
        azure_flow.facts(self.s, self.t, r)
        return r


class Origin(unittest.TestCase):
    def test_every_form_of_clone_url(self):
        for url in ("git@ssh.dev.azure.com:v3/acme/Shop/web", "https://acme@dev.azure.com/acme/Shop/_git/web",
                    "https://dev.azure.com/acme/Shop/_git/web.git", "https://acme.visualstudio.com/Shop/_git/web",
                    "https://acme.visualstudio.com/DefaultCollection/Shop/_git/web", "acme@vs-ssh.visualstudio.com:v3/acme/Shop/web"):
            self.assertEqual(azure.parse(url), A, url)
        self.assertEqual(azure.parse("https://dev.azure.com/acme/My%20Shop/_git/web")["project"], "My Shop")
        self.assertEqual(azure.parse("https://dev.azure.com/acme/_git/web"), {"org": "acme", "project": "web", "repo": "web"})
        for url in ("git@github.com:acme/web.git", "https://github.com/acme/web", "/srv/git/web"):
            self.assertIsNone(azure.parse(url), url)

    def test_links(self):
        self.assertEqual(azure.pr_url({"org": "acme", "project": "My Shop", "repo": "web"}, 7),
                         "https://dev.azure.com/acme/My%20Shop/_git/web/pullrequest/7")
        self.assertEqual(azure.numbered(azure.pr_record(A, 7)), "!7")
        self.assertEqual(azure.numbered({"number": 7, "repo": "acme/web"}), "#7")

    def test_folder_with_azure_origin(self):
        repo = tempfile.mkdtemp(dir=SCRATCH)
        subprocess.run(["git", "init", "-q", repo], check=True)
        subprocess.run(["git", "-C", repo, "remote", "add", "origin", "git@ssh.dev.azure.com:v3/acme/Shop/web"], check=True)
        self.assertEqual(azure.of_folder(repo), A)
        self.assertEqual(github.github(Store(repo)), A)
        subprocess.run(["git", "-C", repo, "remote", "set-url", "origin", "git@github.com:acme/web.git"], check=True)
        self.assertIsNone(azure.of_folder(repo))


class PullRequest(Base):
    def test_opened_ready_not_a_draft(self):
        t = dict(self.t, pr=None, prompt="Make it so")
        rec = azure_flow.pr_open(self.s, t, A, "Done: it is so")
        self.assertEqual(rec["number"], 7)
        self.assertEqual(rec["host"], "azure")
        made = self.az.did("repos", "pr", "create")[0]
        self.assertEqual(made[made.index("--draft") + 1], "false")
        self.assertEqual(made[made.index("--target-branch") + 1], "main")
        self.assertIn("## Summary", made[made.index("--description") + 1])

    def test_one_the_agent_opened_is_taken_on(self):
        self.az.listed = [{"pullRequestId": 9}]
        rec = azure_flow.pr_open(self.s, dict(self.t, pr=None), A)
        self.assertEqual(rec["number"], 9)
        self.assertFalse(self.az.did("repos", "pr", "create"))

    def test_read_in_gh_shape_policies_as_checks(self):
        self.az.policies = [policy("build", "running", name="CI", build="101"), policy("status", "approved", statusGenre="sonar", statusName="gate"),
                            policy("build", "rejected", name="Optional lint", blocking=False, build="102")]
        v = github.pr_view(self.t)
        self.assertEqual(v["state"], "OPEN")
        self.assertEqual(v["headRefOid"], HEAD)
        names = {c["name"]: (c["status"], c["conclusion"]) for c in v["statusCheckRollup"]}
        self.assertEqual(names, {"CI": ("IN_PROGRESS", ""), "sonar/gate": ("COMPLETED", "SUCCESS"), "Optional lint": ("COMPLETED", "NEUTRAL")})
        self.assertTrue(github.pr_pending(v))
        self.assertIsNone(github.pr_problem(v))
        self.az.policies[0]["status"] = "rejected"
        self.assertEqual(github.pr_problem(github.pr_view(self.t)), "Checks failing: CI.")

    def test_a_policy_of_people_is_waited_for_and_stays_ready(self):
        self.az.policies.append(policy("min", "queued", minimumApproverCount=2))
        self.az.pr["reviewers"] = [{"displayName": "Me", "uniqueName": "me@acme.com", "vote": 10}, {"displayName": "Bob", "uniqueName": "bob@acme.com", "vote": 0}]
        v = github.pr_view(self.t)
        self.assertEqual(v["waits"], ["waits for 2 more reviewers: Bob (policy: Minimum number of reviewers; the creator's own vote does not count)"])
        self.assertTrue(github.pr_pending(v))
        self.assertIsNone(github.pr_problem(v))
        self.tick()
        self.assertNotIn("attention", self.t)
        self.assertEqual(task_status(self.t, [])[0], "ready")
        self.assertIn("Waits for 2 more reviewers", self.t["azure_summary"])

    def test_required_reviewers_and_comments_in_words(self):
        self.az.policies += [policy("required", "queued"), policy("comments", "rejected")]
        self.az.pr["reviewers"] = [{"displayName": "Shop Team", "uniqueName": "team", "vote": 0, "isRequired": True, "isContainer": True}]
        w = github.pr_view(self.t)["waits"]
        self.assertEqual(w, ["waits for Shop Team (policy: Required reviewers)", "waits for its comments to be resolved (policy: Comment requirements)"])

    def test_a_reviewer_who_rejects_needs_the_person(self):
        self.az.pr["reviewers"] = [{"displayName": "Ann", "uniqueName": "ann", "vote": -10}]
        self.tick()
        self.assertEqual(self.t["attention"], "A reviewer asked for changes in Azure DevOps: Ann rejected.")
        self.az.pr["reviewers"][0]["vote"] = 10
        self.tick()
        self.assertNotIn("attention", self.t)


class Accept(Base):
    def test_vote_cast_and_completed_at_the_reviewed_commit_by_strategy(self):
        self.write_settings({"method": "merge", "pipeline": "none"})
        self.assertIsNone(self.accept())
        vote = self.az.did("repos", "pr", "set-vote")[0]
        self.assertEqual(vote[vote.index("--vote") + 1], "approve")
        body = self.az.bodies[0]
        self.assertEqual(body["status"], "completed")
        self.assertEqual(body["lastMergeSourceCommit"], {"commitId": HEAD})
        self.assertEqual(body["completionOptions"]["mergeStrategy"], "noFastForward")
        self.assertTrue(body["completionOptions"]["deleteSourceBranch"])
        rest = next(c for c in self.az.calls if c[0] == "rest")
        self.assertEqual(rest[rest.index("--method") + 1], "patch")
        self.assertTrue(rest[rest.index("--url") + 1].endswith("/Shop/_apis/git/repositories/web/pullRequests/7?api-version=7.1"))
        self.assertEqual(self.t["state"], "done")
        self.assertTrue(self.t["merged"])

    def test_a_refused_vote_is_said_and_the_merge_goes_on(self):
        self.write_settings({"pipeline": "none"})
        real = self.az.__call__

        def refuse(args, cwd=None):
            if args[:3] == ["repos", "pr", "set-vote"]:
                return False, "", "ERROR: TF401027: You need the Git 'PullRequestContribute' permission."
            return real(args, cwd)
        self.patch(azure, "az", refuse)
        self.assertIsNone(self.accept())
        self.assertEqual(self.t["state"], "done")
        from herdr_task.commands import landed_words
        self.assertIn("Azure DevOps did not take your vote (TF401027", landed_words(self.t))

    def test_an_unknown_method_is_refused_not_guessed(self):
        self.write_settings({"method": "Merge", "pipeline": "none"})
        self.assertIsNone(self.accept())
        self.assertEqual(self.az.bodies[0]["completionOptions"]["mergeStrategy"], "noFastForward")
        self.setUp()
        self.write_settings({"method": "fast-forward", "pipeline": "none"})
        err = self.accept()
        self.assertIn("azure.method is 'fast-forward'", err)
        self.assertEqual(self.t["attention"], err)
        self.assertFalse(self.az.bodies)
        self.assertFalse(self.az.did("repos", "pr", "set-vote"))

    def test_auto_complete_only_where_a_push_cannot_slip_through(self):
        self.write_settings({"pipeline": "none"})
        self.az.policies[0]["status"] = "running"
        self.assertEqual(self.accept(), azure_flow.WAIT)
        auto = self.az.did("repos", "pr", "update")[0]
        self.assertEqual(auto[auto.index("--auto-complete") + 1], "true")
        self.assertEqual(auto[auto.index("--squash") + 1], "true")
        self.assertTrue(self.t["azure_vote"]["auto_complete"])
        self.assertIn("Waiting for Azure DevOps checks · 0 of 1 passed · Approved here and in Azure DevOps", self.t["merging_why"])
        # Only reviewers whose votes stay on a push: no auto-complete, herdr-task completes it itself.
        self.setUp()
        self.write_settings({"pipeline": "none"})
        self.az.policies = [policy("min", "queued")]
        self.assertEqual(self.accept(), azure_flow.WAIT)
        self.assertFalse(self.az.did("repos", "pr", "update"))
        self.assertIn("auto_skipped", self.t["azure_vote"])
        # A rebase az can't set.
        self.setUp()
        self.write_settings({"method": "rebase", "pipeline": "none"})
        self.az.policies[0]["status"] = "running"
        self.accept()
        self.assertFalse(self.az.did("repos", "pr", "update"))

    def test_vote_once_per_accept_and_withdrawn_when_it_no_longer_stands(self):
        self.write_settings({"pipeline": "none"})
        self.az.policies[0]["status"] = "running"
        self.accept()
        self.t["auto_merge"] = "checks"
        azure_flow.merge_now(self.s, self.t)
        self.assertEqual(len(self.az.did("repos", "pr", "set-vote")), 1)
        # Someone pushed a commit that wasn't reviewed here.
        self.az.pr["lastMergeSourceCommit"] = {"commitId": "b" * 40}
        self.tick()
        votes = [c[c.index("--vote") + 1] for c in self.az.did("repos", "pr", "set-vote")]
        self.assertEqual(votes, ["approve", "reset"])
        offs = [c[c.index("--auto-complete") + 1] for c in self.az.did("repos", "pr", "update")]
        self.assertEqual(offs, ["true", "false"])
        self.assertTrue(self.t["azure_vote"]["reset"])

    def test_unreviewed_commits_never_merge(self):
        self.az.pr["lastMergeSourceCommit"] = {"commitId": "b" * 40}
        err = self.accept()
        self.assertIn("commits that were not reviewed here", err)
        self.assertEqual(self.t["attention"], err)
        self.assertFalse(self.az.bodies)

    def test_a_draft_is_made_ready(self):
        self.write_settings({"pipeline": "none"})
        self.az.pr["isDraft"] = True
        self.accept()
        ready = self.az.did("repos", "pr", "update")[0]
        self.assertEqual(ready[ready.index("--draft") + 1], "false")

    def test_accepted_completion_not_carried_out_goes_back_to_the_person(self):
        self.write_settings({"pipeline": "none", "complete_timeout": 30})
        self.az.completes = False
        self.assertEqual(self.accept(), azure_flow.WAIT)
        self.t["auto_merge"] = "checks"
        self.assertIn("azure_completing", self.t)
        self.tick()
        self.assertNotIn("attention", self.t)
        self.assertFalse(self.t["_pr_fresh"])
        self.az.pr["mergeStatus"] = "rejectedByPolicy"
        self.tick()
        self.assertEqual(self.t["attention"], "Azure DevOps accepted the completion, but a policy rejected the merge.")
        self.assertNotIn("azure_completing", self.t)

    def test_accepted_completion_that_takes_too_long(self):
        self.write_settings({"pipeline": "none", "complete_timeout": 30})
        self.az.completes = False
        self.accept()
        self.t["auto_merge"] = "checks"
        self.az.pr["mergeStatus"] = "queued"
        self.t["azure_completing"]["since"] = now() - 31 * 60000
        self.tick()
        self.assertIn("hasn't carried it out", self.t["attention"])

    def test_completed_meanwhile_goes_on_by_itself(self):
        self.write_settings({"pipeline": "none"})
        self.az.completes = False
        self.accept()
        self.az.pr.update(status="completed", lastMergeCommit={"commitId": MERGE})
        r = self.tick()
        self.assertEqual(r["done"], [1])

    def test_a_refused_completion_is_said(self):
        self.write_settings({"pipeline": "none"})
        self.az.completes, self.az.complete_fails = False, "ERROR: TF401181: The pull request cannot be completed."
        err = self.accept()
        self.assertEqual(err, "Azure DevOps didn't complete it: TF401181: The pull request cannot be completed.")


class MergedWork(Base):
    def test_completed_in_azure_devops_is_merged_whatever_its_head(self):
        self.write_settings({"pipeline": "none"})
        self.az.pr.update(status="completed", lastMergeCommit={"commitId": MERGE}, lastMergeSourceCommit={"commitId": "c" * 40})
        self.assertIsNone(self.accept())
        self.assertFalse(self.az.bodies)
        self.assertNotIn("attention", self.t)
        self.assertEqual(self.t["state"], "done")

    def test_merged_by_tick_waits_for_its_main_pipeline(self):
        self.az.pr.update(status="completed", lastMergeCommit={"commitId": MERGE})
        self.t["state"] = "running"
        r = self.tick()
        self.assertEqual(r["done"], [])
        self.assertEqual(self.t["state"], "ready")
        self.assertEqual(self.t["pipeline_wait"]["commit"], MERGE)
        self.assertEqual(task_status(self.t, [])[:3:2], ("merging", "Merged: !7. Waiting for the main pipeline to start"))

    def test_pr_view_for_the_panel(self):
        from types import SimpleNamespace
        import io
        import contextlib
        import herdr_task.store as store

        class Open:
            def __init__(self, s):
                self.s = s

            def __enter__(self):
                return self.s

            def __exit__(self, *a):
                return False
        self.s.find = lambda ref: self.t
        self.patch(store, "open_store", lambda args, write=True: Open(self.s))
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            azure_flow.cmd_pr_view(SimpleNamespace(id="1", folder=None, json=True))
        v = json.loads(out.getvalue())
        self.assertEqual((v["host"], v["numbered"], v["state"]), ("azure", "!7", "OPEN"))
        self.assertNotIn("evaluations", v)


class MainPipeline(Base):
    def setUp(self):
        super().setUp()
        self.az.pr.update(status="completed", lastMergeCommit={"commitId": MERGE})
        self.accept()
        self.assertIn("pipeline_wait", self.t)

    def run_(self, id, status, result="", pipeline="CI", commit=MERGE):
        return {"id": id, "definition": {"name": pipeline, "id": 3}, "status": status, "result": result, "sourceVersion": commit}

    def test_passed_is_done(self):
        self.az.runs = [self.run_(200, "completed", "succeeded"), self.run_(199, "completed", "failed", commit="o" * 40)]
        r = self.tick()
        self.assertEqual(r["done"], [1])
        self.assertEqual(self.t["pipeline"], "Main pipeline passed: CI passed")

    def test_running_says_how_far(self):
        self.az.runs = [self.run_(200, "inProgress")]
        self.az.timelines["200"] = [record("s1", "Stage", "Build", "completed", "succeeded", order=1), record("s2", "Stage", "Test", "inProgress", order=2),
                                    record("s3", "Stage", "Lint", "completed", "skipped", order=3), record("j", "Job", "Unit", "inProgress", parent="s2")]
        self.tick()
        self.assertEqual(self.t["merging_why"], "Main pipeline running · 1 of 2 stages · Lint skipped")
        self.az.timelines["200"].append(record("s4", "Stage", "Deploy_Staging", "inProgress", order=4))
        self.tick()
        self.assertEqual(self.t["merging_why"], "Deploying Staging")

    def test_failed_needs_the_person_with_the_end_of_its_log(self):
        self.az.runs = [self.run_(200, "completed", "failed")]
        self.az.timelines["200"] = [record("j", "Job", "Unit", "completed", "failed"),
                                    record("k", "Task", "npm test", "completed", "failed", parent="j", log=12)]
        self.az.logs["12"] = ["2026-11-04T10:00:00.1Z ok", "2026-11-04T10:00:01.1Z \x1b[31mFAIL\x1b[0m one test"]
        self.tick()
        self.assertIn("Main pipeline failed: The pipeline CI failed (run 200)", self.t["attention"])
        self.assertIn("Failed in job Unit, step npm test", self.t["pipeline_failed"])
        self.assertIn("FAIL one test", self.t["pipeline_failed"])
        # Run again in Azure DevOps, and passed: done.
        self.az.runs.insert(0, self.run_(201, "completed", "succeeded"))
        self.assertEqual(self.tick()["done"], [1])

    def test_held_at_a_stage_approval_waits_for_the_person(self):
        self.az.runs = [self.run_(200, "inProgress")]
        self.az.timelines["200"] = [record("s1", "Stage", "Staging", "completed", "succeeded", order=1), record("s2", "Stage", "prodDE", "pending", order=2),
                                    record("c", "Checkpoint", "Checkpoint", "inProgress", parent="s2"),
                                    record("ap", "Checkpoint.Approval", "Approval", "inProgress", parent="c")]
        self.t["pipeline_wait"]["since"] = now() - 5 * 3600000  # no timeout while it waits for a person
        r = self.tick()
        self.assertEqual(r["attention"], [1])
        self.assertTrue(self.t["attention"].startswith("Main pipeline CI waits for your approval of stage prodDE in Azure DevOps: https://dev.azure.com/acme/Shop/_build/results?buildId=200"))
        self.assertEqual(task_status(self.t, [])[0], "needs")
        # Approved there: on by itself, the timeout counted from then.
        self.az.timelines["200"][-1].update(state="completed", result="succeeded")
        self.az.timelines["200"][1]["state"] = "inProgress"
        self.az.timelines["200"].append(record("j", "Job", "Deploy", "inProgress", parent="s2"))
        self.tick()
        self.assertNotIn("attention", self.t)
        self.assertEqual(self.t["merging_why"], "Deploying prodDE")

    def test_a_rejected_approval_stops_it(self):
        self.az.runs = [self.run_(200, "completed", "failed")]
        self.az.timelines["200"] = [record("s2", "Stage", "Production_DE", "completed", "failed"), record("c", "Checkpoint", "Checkpoint", "completed", parent="s2"),
                                    record("ap", "Checkpoint.Approval", "Approval", "completed", "rejected", parent="c")]
        self.tick()
        self.assertTrue(self.t["attention"].startswith("Main pipeline stopped: The approval of stage Production_DE was rejected or ran out in Azure DevOps (run 200)"))

    def test_the_merge_commit_is_read_again(self):
        self.az.pr["lastMergeCommit"] = {"commitId": "n" * 40}
        self.tick()
        self.assertEqual(self.t["pipeline_wait"]["commit"], "n" * 40)

    def test_no_pipeline_in_the_repo(self):
        self.az.pipelines = []
        self.assertEqual(self.tick()["done"], [1])
        self.assertEqual(self.t["pipeline"], "No pipeline in this repo to wait for.")

    def test_a_named_pipeline_not_started_in_time(self):
        self.write_settings({"pipeline": "Release", "pipeline_timeout": 60})
        self.az.runs = [self.run_(200, "completed", "succeeded")]
        self.tick()
        self.assertEqual(self.t["merging_why"], "Merged: !7. Waiting for the pipeline Release to start")
        self.t["pipeline_wait"]["since"] = now() - 61 * 60000
        self.tick()
        self.assertEqual(self.t["attention"], "The main pipeline did not start: Release not started after 60 minutes.")


class PullRequestPipeline(Base):
    def test_its_own_pipeline_in_words(self):
        self.az.policies = [policy("build", "running", name="CI", build="101")]
        self.az.timelines["101"] = [record("s1", "Stage", "Build", "completed", "succeeded", order=1),
                                    record("s2", "Stage", "Deploy_Staging", "completed", "succeeded", order=2),
                                    record("s3", "Stage", "Smoke", "inProgress", order=3)]
        self.tick()
        self.assertEqual(self.t["pr_pipeline"]["words"], "Pull request open · deployed to Staging · pipeline running")

    def test_held_at_an_approval_before_the_accept(self):
        self.az.policies = [policy("build", "running", name="CI", build="101")]
        self.az.timelines["101"] = [record("s2", "Stage", "Staging", "pending"), record("c", "Checkpoint", "Checkpoint", "inProgress", parent="s2"),
                                    record("ap", "Checkpoint.Approval", "Approval", "inProgress", parent="c")]
        r = self.tick()
        self.assertEqual(r["attention"], [1])
        self.assertTrue(self.t["azure_hold"].startswith("Pipeline CI waits for your approval of stage Staging in Azure DevOps"))
        status, needs, why = task_status(self.t, [])
        self.assertEqual((status, needs), ("needs", "attention"))
        self.assertNotIn("attention", self.t)  # the Accept is still the person's to give
        # After the Accept it says so while it merges.
        self.write_settings({"pipeline": "none"})
        self.assertEqual(self.accept(), azure_flow.WAIT)
        self.t["auto_merge"] = "checks"
        self.tick()
        self.assertNotIn("azure_hold", self.t)
        self.assertTrue(self.t["merging_why"].startswith("Pipeline CI waits for your approval"))


class PushedToMain(Base):
    def test_waits_for_the_run_of_the_commit_that_brought_it(self):
        repo = tempfile.mkdtemp(dir=SCRATCH)
        g = lambda *a: subprocess.run(["git", "-C", repo, *a], check=True, capture_output=True, text=True).stdout.strip()
        g("init", "-q", "-b", "main")
        g("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "start")
        g("remote", "add", "origin", "https://acme@dev.azure.com/acme/Shop/_git/web")
        g("checkout", "-q", "-b", "work/1")
        g("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "work")
        tip = g("rev-parse", "HEAD")
        g("checkout", "-q", "main")
        g("merge", "-q", "--ff-only", "work/1")
        s, t = Store(repo), dict(self.t, pr=None, state="running")
        self.assertTrue(azure_flow.direct(s, t, report()))
        self.assertEqual(t["pipeline_wait"]["commit"], tip)
        self.assertEqual(t["merging_why"], f"On main: {tip[:7]}. Waiting for the main pipeline to start")
        self.assertEqual(task_status(t, [])[0], "merging")


class News(Base):
    def test_comments_people_wrote_reach_the_agent(self):
        self.az.threads = [{"id": 4, "threadContext": {"filePath": "/src/a.py"}, "comments": [
            {"id": 1, "content": "Rename this", "author": {"uniqueName": "ann@acme.com"}, "publishedDate": "2026-11-04T10:00:00.5Z"},
            {"id": 2, "content": "Ann voted 10", "commentType": "system", "publishedDate": "2026-11-04T10:00:01Z"}]}]
        c = azure.comments(self.t["pr"])
        self.assertEqual([(x["who"], x["body"], x.get("where")) for x in c], [("ann@acme.com", "Rename this", "src/a.py")])
        self.patch(ghnews, "resting", lambda s, t: {"address": "t1"})
        v = github.pr_view(self.t)
        ghnews.pr_news(self.s, self.t, v, report())
        text = self.t["pr"]["news"]["text"]
        self.assertIn("Azure DevOps news on pull request !7", text)
        self.assertIn("Comment by ann@acme.com on src/a.py: Rename this", text)

    def test_the_summary_is_a_closed_thread(self):
        self.assertTrue(azure.comment(self.t["pr"], "Done"))
        call = self.az.did("devops", "invoke")[0]
        self.assertEqual(call[call.index("--http-method") + 1], "POST")


class GitHubUnchanged(unittest.TestCase):
    def test_github_pr_is_still_read_through_gh(self):
        seen = []

        def gh(*args, cwd=None):
            seen.append(args)
            return True, json.dumps({"state": "OPEN", "mergeable": "MERGEABLE", "statusCheckRollup": []}), ""
        old = github.gh
        github.gh = gh
        try:
            v = github.pr_view({"pr": {"number": 3, "repo": "acme/web"}})
        finally:
            github.gh = old
        self.assertEqual(seen[0][:3], ("pr", "view", "3"))
        self.assertIsNone(github.pr_problem(v))
        self.assertEqual(azure.numbered({"number": 3, "repo": "acme/web"}), "#3")


if __name__ == "__main__":
    unittest.main()
