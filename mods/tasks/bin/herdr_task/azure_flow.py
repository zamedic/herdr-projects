"""Azure DevOps merging: a task's pull request there opened ready, merged on the person's Accept
(their vote cast there too, its auto-complete set where a push can't slip through) by its merge
strategy at the reviewed commit only, an accepted completion that isn't carried out watched until
it is (or sent back to the person), and the base branch's pipeline for the merge commit waited for
before the task is done (the main pipeline's gate), stage approvals held there shown as needing
the person. Called by github.py's callers (commands.merge_now, tick.facts) for a pull request whose
record says host "azure"."""

from . import azure as az
from . import azure_pipeline as pl
from .base import git, now, PR_EVERY
from .settings import settings_for
from .github import finish, pr_pending, pr_problem, update_main

WAIT = "Checks are still running."  # what commands.cmd_accept takes for "it merges once they pass"
START_GRACE_MS = 10 * 60000         # pipeline "all" and no run of the merge commit after this long: none runs for it
DEFAULTS = {"method": "squash", "pipeline": "all", "pipeline_timeout": 60, "complete_timeout": 30}


def options(s, t):
    """Settings' "azure" part (~/.herdr/shepherdr.json): method (squash, merge, rebase,
    rebase-merge), pipeline (all, none, or pipeline names or ids), pipeline_timeout and
    complete_timeout (minutes)."""
    o = dict(DEFAULTS)
    o.update({k: v for k, v in (settings_for(s, t.get("space")).get("azure") or {}).items() if v not in (None, "")})
    o["method"] = str(o["method"]).strip().lower()
    return o


def minutes(v, default):
    try:
        return max(0, float(v)) * 60000
    except (TypeError, ValueError):
        return default * 60000


def message(s, t):
    return f"{t['title']}\n\nTask: {s.ref(t)}"


def say_attention(s, t, text):
    """Needs the person, for a reason of Azure DevOps': kept so that it is cleared once it passed."""
    t["attention"] = t["azure_attention"] = text
    s.dirty = True


def clear_attention(s, t):
    if t.get("azure_attention") and t.get("attention") == t["azure_attention"]:
        t.pop("attention")
    if t.pop("azure_attention", None) is not None:
        s.dirty = True


def why(s, t, text):
    """The Merging row's second line."""
    if t.get("merging_why") != text:
        t["merging_why"] = text
        s.dirty = True


# ---- opening it -------------------------------------------------------------------------------

def pr_open(s, task, a, summary=None):
    """After the push (github.pr_open): keeps its open pull request (the summary as a comment),
    else takes on one the agent opened for the branch, else opens one, ready for review."""
    pr = task.get("pr") or {}
    if az.is_azure(pr):
        v = az.show(pr["azure"], pr["number"])
        if isinstance(v, dict) and str(v.get("status")).lower() == "active":
            if summary:
                az.comment(pr, summary)
            return pr
    found = az.json_of(a, "repos", "pr", "list", "--project", a["project"], "--repository", a["repo"],
                       "--source-branch", task["branch"], "--status", "active", "--top", "1")
    if isinstance(found, list) and found and found[0].get("pullRequestId"):
        task["pr"] = az.pr_record(a, found[0]["pullRequestId"])
        if summary:
            az.comment(task["pr"], summary)
        return task["pr"]
    body = (task.get("prompt") or "").strip() or task["title"]
    if summary:
        body += "\n\n## Summary\n\n" + summary.strip()
    if (task.get("details") or "").strip():
        body += "\n\n## Details\n\n" + task["details"].strip()
    rec, err = az.create(a, task["branch"], task.get("base") or "main", f"{s.ref(task)} {task['title']}", body)
    if not rec:
        from .base import Fail
        raise Fail(f"Couldn't open a pull request in Azure DevOps: {err}")
    task["pr"] = rec
    return rec


def reset(t):
    """A new ready: what an earlier round left of its merge no longer stands."""
    for k in ("pipeline_wait", "azure_completing", "azure_hold", "merging_why", "azure_attention", "pr_pipeline", "azure_summary"):
        t.pop(k, None)


# ---- the person's yes, given in Azure DevOps too ----------------------------------------------

def give(s, t, v, method):
    """Their Approve vote on the pull request, and its auto-complete where a push can't be completed
    before herdr-task looks again (azure.push_holds): once per Accept and reviewed commit."""
    pr, key, head = t["pr"], t.get("accepted_at") or t.get("ready_at"), t.get("head") or v.get("headRefOid")
    was = t.get("azure_vote") or {}
    if was.get("for") == key and was.get("head") == head and not was.get("reset"):
        return was
    ok, _, err = az.cast_vote(pr)
    holds = az.push_holds(v.get("evaluations") or [])
    auto = az.auto_complete(pr, method, message(s, t)) if holds and method in ("squash", "merge") else None
    vote = {"for": key, "head": head, "voted": ok, "auto_complete": bool(auto and auto[0]), "at": now()}
    if not ok:
        vote["said"] = az.first_line(err)
    if not holds:
        vote["auto_skipped"] = "no build validation and no policy that resets the votes on a push: herdr-task completes it at the reviewed commit"
    elif auto and not auto[0]:
        vote["auto_said"] = az.first_line(auto[2])
    t["azure_vote"] = vote
    s.dirty = True
    return vote


def vote_note(t):
    """Said after an Accept whose vote Azure DevOps refused: the merge went on, the vote didn't."""
    v = t.get("azure_vote") or {}
    return f" Azure DevOps did not take your vote ({v['said']})." if v.get("said") and not v.get("voted") else ""


def approved_words(t, v):
    vote = t.get("azure_vote") or {}
    if not vote:
        return ""
    given = "Approved here and in Azure DevOps" if vote.get("voted") else f"Approved here; Azure DevOps did not take your vote ({vote.get('said')})"
    return " · ".join([given] + list(v.get("waits") or []))


def stale(t, head):
    """The vote or auto-complete still stands, but the Accept it was given for doesn't: the task is
    no longer merging, needs the person, or the pull request names another commit."""
    v = t.get("azure_vote") or {}
    if not (v.get("voted") or v.get("auto_complete")):
        return False
    merging = t["state"] == "ready" and t.get("auto_merge") and not t.get("attention")
    return not merging or bool(head and v.get("head") and head != v["head"])


def withdraw(s, t):
    """Calls the auto-complete off and takes the vote back; what didn't go through is tried again."""
    v, pr = t["azure_vote"], t["pr"]
    if v.get("auto_complete") and az.call_off(pr)[0]:
        v["auto_complete"] = False
    if v.get("voted") and az.reset_vote(pr)[0]:
        v["voted"], v["reset"] = False, True
    s.dirty = True


# ---- merging ----------------------------------------------------------------------------------

def merge_now(s, t):
    """The Accept (or merge on its own): only the reviewed commit, made ready if it is a draft, the
    vote cast, its policies through, then completed by its strategy. Returns why it can't yet (WAIT
    while it waits for Azure DevOps), or None once it merged."""
    pr = t["pr"]
    s.dirty = True
    v = az.view(pr)
    if not v:
        return f"Couldn't reach pull request !{pr['number']} on Azure DevOps."
    if v["state"] == "MERGED":
        return merged(s, t, v)  # completed in Azure DevOps already: merged work, whatever its head
    o = options(s, t)
    if not az.strategy(o["method"]):
        say_attention(s, t, f"Settings' azure.method is {o['method']!r}: it is squash, merge, rebase or rebase-merge. Fix it, then accept again.")
        return t["attention"]
    reviewed = t.get("head")
    if reviewed and v["headRefOid"] and v["headRefOid"] != reviewed:
        say_attention(s, t, f"The pull request has commits that were not reviewed here (reviewed {reviewed[:7]}, on the pull "
                            f"request {v['headRefOid'][:7]}): bring them into the task's worktree, or push what was reviewed, then say ready again.")
        return t["attention"]
    if v.get("isDraft"):
        az.call(pr["azure"], "repos", "pr", "update", "--id", str(pr["number"]), "--draft", "false")
    give(s, t, v, o["method"])
    problem = pr_problem(v)
    if problem:
        say_attention(s, t, problem)
        return problem
    if pr_pending(v):
        why(s, t, waiting_words(t, v))
        return WAIT
    ok, _, err = az.complete(pr, reviewed or v["headRefOid"], o["method"], message(s, t))
    after = az.view(pr) or v
    if after["state"] == "MERGED":
        return merged(s, t, after)
    if not ok and (pr_problem(after) or not pr_pending(after)):
        say_attention(s, t, f"Azure DevOps didn't complete it: {az.first_line(err)}")
        return t["attention"]
    # Accepted is not completed: Azure DevOps may still be merging it (watched by tick).
    t["azure_completing"] = {"since": now()}
    why(s, t, f"Approved: Azure DevOps completes it ({pr['url']})")
    return WAIT


def waiting_words(t, v):
    checks = [c for c in v.get("statusCheckRollup") or [] if not c.get("people")]
    done = sum(1 for c in checks if (c.get("conclusion") or "").upper() in ("SUCCESS", "NEUTRAL", "SKIPPED"))
    parts = [f"Waiting for Azure DevOps checks · {done} of {len(checks)} passed" if checks else "Waiting for Azure DevOps"]
    if approved_words(t, v):
        parts.append(approved_words(t, v))
    return " · ".join(parts)


def merged(s, t, v):
    """Its pull request is completed (by herdr-task, its auto-complete or the person): the base
    branch here catches up, and the task waits for the main pipeline of the merge commit (unless
    Settings' azure.pipeline is "none"), else it is done. Returns None."""
    commit = (v.get("mergeCommit") or {}).get("oid") or ""
    for k in ("azure_completing", "azure_hold", "pr_pipeline"):
        t.pop(k, None)
    clear_attention(s, t)
    update_main(s, t)
    s.dirty = True
    if str(options(s, t)["pipeline"]).strip().lower() == "none":
        t["merged"] = True
        finish(s, t, even_if_busy=True)
        return None
    arm(s, t, commit, v.get("baseRefName") or t.get("base") or "main", f"Merged: {az.numbered(t['pr'])}")
    return None


def arm(s, t, commit, base, landed):
    """The task's work is on `base` at `commit`: it stays Merging until that commit's runs passed."""
    t["state"] = "ready"
    t["auto_merge"] = t.get("auto_merge") or "checks"
    t["pipeline_wait"] = {"commit": commit, "base": base, "since": now(), "landed": landed}
    why(s, t, f"{landed}. Waiting for the main pipeline to start")


def direct(s, t, report):
    """The agent put the work on the base branch itself (merged_here, no pull request): on Azure
    DevOps it waits for the main pipeline of the commit that brought it there. False: not so here."""
    a = az.of_folder(s.loc["folder"])
    if not a or t.get("pipeline_wait") or str(options(s, t)["pipeline"]).strip().lower() == "none":
        return False
    main, base, br = s.loc["folder"], t.get("base") or "main", t.get("branch")
    ok, tip, _ = git(main, "rev-parse", "--verify", "--quiet", f"refs/heads/{br}")
    tip = tip if ok and tip else t.get("head")
    # The first commit of the base's own line that holds it: the tip itself when it was pushed as it is.
    line = git(main, "rev-list", "--first-parent", "--reverse", "--ancestry-path", f"{tip}..{base}")[1].split()
    on_line = git(main, "rev-list", "--first-parent", base)[1].split()
    commit = tip if tip in on_line else (line[0] if line else tip)
    t["merged_direct"] = True
    arm(s, t, commit, base, f"On {base}: {commit[:7]}")
    s.dirty = True
    report.setdefault("azure", []).append({"task": s.ref(t), "waits": "main pipeline", "commit": commit})
    return True


# ---- tick -------------------------------------------------------------------------------------

def facts(s, t, report):
    """tick's look at a task whose pull request is at Azure DevOps."""
    pr = t["pr"]
    if t.get("pipeline_wait"):
        # Merged: only its main pipeline is left (while its agent fixes a failure, nothing).
        if t["state"] == "ready":
            gate(s, t, report)
        return
    if now() - pr.get("checked", 0) < PR_EVERY:
        return
    pr["checked"] = now()
    s.dirty = True
    v = az.view(pr)
    if not v:
        return
    if t.get("azure_vote") and stale(t, v.get("headRefOid")):
        withdraw(s, t)
    if v["state"] == "MERGED":
        merged(s, t, v)
        if t["state"] == "done":
            report["done"].append(t["id"])
        return
    problem = pr_problem(v)
    c = t.get("azure_completing")
    if c and v["state"] == "OPEN" and not problem:
        ms = str(v.get("mergeStatus") or "").lower()
        if ms == "rejectedbypolicy":
            problem = "Azure DevOps accepted the completion, but a policy rejected the merge."
        elif ms == "failure":
            problem = "Azure DevOps accepted the completion, but the merge failed there."
        elif now() - c.get("since", 0) >= minutes(options(s, t)["complete_timeout"], 30):
            problem = "Azure DevOps accepted the completion but hasn't carried it out: look at the pull request there."
    if problem and c:
        t.pop("azure_completing")
    t["_pr_fresh"] = not problem and not pr_pending(v) and not c
    t["azure_summary"] = az.summary(v)
    if v["state"] == "OPEN":
        t["pr_pipeline"] = pl.pr_pipeline(pr["azure"], v.get("statusCheckRollup") or [], t.get("pr_pipeline"))
    hold = (t.get("pr_pipeline") or {}).get("approval") if v["state"] == "OPEN" else None
    try:
        from .ghnews import pr_news
        pr_news(s, t, v, report)
    except Exception as e:  # never stops the tick
        report.setdefault("github_error", []).append(f"{s.ref(t)}: {e}")
    if t["state"] != "ready":
        return
    if problem:
        if t.get("attention") != problem:
            say_attention(s, t, problem)
            report["attention"].append(t["id"])
    else:
        clear_attention(s, t)
    # Its own pipeline held at a stage's approval: before the Accept it needs the person (the
    # Accept is still theirs to give); after it, it says so while it merges.
    if hold and not t.get("auto_merge"):
        if t.get("azure_hold") != hold:
            t["azure_hold"] = hold
            report["attention"].append(t["id"])
    elif t.pop("azure_hold", None):
        s.dirty = True
    if t.get("auto_merge") and not problem:
        why(s, t, hold or (f"Approved: Azure DevOps completes it ({pr['url']})" if c else waiting_words(t, v)))


def gate(s, t, report):
    """One look at the main pipeline's runs of the task's merge commit."""
    w = t["pipeline_wait"]
    if now() - w.get("checked", 0) < PR_EVERY:
        return
    w["checked"] = now()
    s.dirty = True
    a = (t.get("pr") or {}).get("azure") or az.of_folder(s.loc["folder"])
    o = options(s, t)
    if not a or str(o["pipeline"]).strip().lower() == "none":
        return done(s, t, report, "The main pipeline is not waited for.")
    commit, base, landed = w.get("commit") or "", w.get("base") or "main", w.get("landed") or "Merged"
    overdue = now() - w.get("since", 0) >= minutes(o["pipeline_timeout"], 60)
    runs = pl.runs_of_commit(a, base, commit)
    if runs is None:
        if overdue:
            say_attention(s, t, f"Couldn't read the main pipeline's runs for {commit[:7]} on {base}. {az.LOGIN}")
        else:
            why(s, t, f"{landed}. Waiting for the main pipeline · could not read its runs")
        return
    if not runs and recommit(s, t, w):
        return
    wanted, missing = pl.wanted(runs, o["pipeline"]), pl.not_started(runs, o["pipeline"])
    r = pl.look(a, wanted, missing)
    held = w.pop("approval", None)
    if r["is"] == "passed":
        return done(s, t, report, f"Main pipeline passed: {r['words']}")
    if r["is"] == "failed":
        t["pipeline_failed"] = r["detail"]
        return say_attention(s, t, f"Main pipeline failed: {r['words']}. The work is merged; fix it on the branch (rebase it onto "
                                   f"{base} first) and say ready again, or run the pipeline again in Azure DevOps.")
    if r["is"] == "refused":
        return say_attention(s, t, f"Main pipeline stopped: {r['words']}. Run the stage again there and approve it, or archive the task.")
    if r["is"] == "approval":
        # Waiting for a person is not the pipeline taking too long: no timeout meanwhile.
        w["approval"] = r["words"]
        if t.get("attention") != r["words"]:
            say_attention(s, t, r["words"])
            report["attention"].append(t["id"])
        return
    if held:
        w["since"] = now()  # the timeout counts from the person's approval there
    clear_attention(s, t)
    if r["is"] == "not_started" and not r["names"]:
        if pl.pipeline_ids(a) == []:
            return done(s, t, report, "No pipeline in this repo to wait for.")
        if now() - w.get("since", 0) >= START_GRACE_MS:
            return done(s, t, report, f"No pipeline ran for {commit[:7]} on {base}.")
        return why(s, t, f"{landed}. Waiting for the main pipeline to start")
    if r["is"] == "not_started":
        if overdue:
            return say_attention(s, t, f"The main pipeline did not start: {', '.join(r['names'])} not started after {o['pipeline_timeout']} minutes.")
        return why(s, t, f"{landed}. Waiting for the pipeline {', '.join(r['names'])} to start")
    if not r.get("started") and overdue:
        return say_attention(s, t, f"The main pipeline did not finish: {r['words']}, no job started after {o['pipeline_timeout']} minutes.")
    why(s, t, r["words"])


def recommit(s, t, w):
    """No run of the commit yet: the pull request may have named its trial merge at first. Its
    merge commit now, when that is another one, is waited for instead."""
    pr = t.get("pr")
    if t.get("merged_direct") or not az.is_azure(pr):
        return False
    v = az.view(pr)
    oid = ((v or {}).get("mergeCommit") or {}).get("oid") or ""
    if not v or v["state"] != "MERGED" or not oid or oid.lower() == (w.get("commit") or "").lower():
        return False
    w["commit"] = oid
    w["checked"] = 0
    return True


def done(s, t, report, words):
    t["pipeline"] = words
    t["merged"] = True
    for k in ("pipeline_wait", "merging_why", "pipeline_failed", "azure_attention"):
        t.pop(k, None)
    finish(s, t, even_if_busy=True)
    report["done"].append(t["id"])


# ---- the Pull request view --------------------------------------------------------------------

def cmd_pr_view(args):
    """`herdr-task pr-view <id> --json`: the task's pull request as `gh pr view --json` gives it, for
    the Tasks panel's Pull request view (mods/task-pr), which asks gh itself for GitHub's."""
    import json
    import sys
    from .base import Fail
    from .github import pr_view
    from .store import open_store
    with open_store(args, write=False) as s:
        t = s.find(args.id)
        if not t.get("pr"):
            raise Fail(f"{s.ref(t)} has no pull request.")
        v = pr_view(t)
    if not v:
        raise Fail(f"Couldn't reach pull request {az.numbered(t['pr'])}.")
    v.pop("evaluations", None)
    if az.is_azure(t["pr"]):
        v["host"], v["numbered"], v["summary"] = "azure", az.numbered(t["pr"]), az.summary(v)
        v["pipeline"] = (t.get("pr_pipeline") or {}).get("words")
        v["statusCheckRollup"] = [c for c in v.get("statusCheckRollup") or [] if not c.get("people")]
    json.dump(v, sys.stdout)
    print()
