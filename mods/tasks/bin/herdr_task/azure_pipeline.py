"""Azure Pipelines: a build's timeline (its stages, jobs and steps), the end of the log of what
failed in it, stages held at an approval for the person (an environment's check), the pull
request's own pipeline in words ("Pull request open · deploying Staging") and the base branch's
runs for a task's merge commit (the main pipeline's gate)."""

import re

from .azure import build_url, invoke, json_of

LOG_LINES = 40


# ---- a build's timeline and log ---------------------------------------------------------------

def records_of(v):
    recs = [{"id": r.get("id"), "parent": r.get("parentId"), "type": r.get("type") or "", "name": r.get("name") or "",
             "state": r.get("state") or "", "result": r.get("result") or "", "log": str((r.get("log") or {}).get("id") or ""),
             "order": r.get("order") or 0} for r in (v or {}).get("records") or []]
    return sorted(recs, key=lambda r: r["order"])


def timeline(a, build):
    """The build's timeline; [] when it can't be read (yet)."""
    v = invoke(a, "build", "timeline", {"buildId": build})
    return records_of(v) if isinstance(v, dict) else []


def failed_steps(recs):
    """What failed: each failed step with a log, with its job; else a failed job itself."""
    bad = lambda r: r["result"] in ("failed", "canceled", "abandoned")
    job = lambda r: next((j["name"] for j in recs if j["id"] == r["parent"] and j["type"] == "Job"), "")
    steps = [(job(r), r) for r in recs if bad(r) and r["type"] == "Task" and r["log"]]
    return steps or [(r["name"], r) for r in recs if bad(r) and r["type"] == "Job"]


def clean(lines):
    out = []
    for line in lines:
        line = re.sub(r"\x1b\[[0-9;]*[A-Za-z]", "", line)
        line = re.sub(r"^\d{4}-\d\d-\d\dT[0-9:.]+Z ?", "", line).rstrip()
        if line.strip():
            out.append(line)
    return out


def tail(a, build):
    """The failing job and step of build `build`, and the end of its log."""
    url = build_url(a, build)
    recs = timeline(a, build)
    found = failed_steps(recs)
    if not found:
        return f"Azure Pipelines named no failed job in its timeline: {url}"
    job, step = found[0]
    at = f"job {step['name']}" if not job or job == step["name"] else f"job {job}, step {step['name']}"
    lines = []
    if step["log"]:
        v = invoke(a, "build", "logs", {"buildId": build, "logId": step["log"]})
        lines = clean(v.splitlines() if isinstance(v, str) else [str(x) for x in (v or {}).get("value") or []] if isinstance(v, dict) else [])
    if not lines:
        return f"Failed in {at}. Its log could not be read here: {url}"
    return f"Failed in {at}. The end of its log ({url}):\n" + "\n".join(lines[-LOG_LINES:])


# ---- stages -----------------------------------------------------------------------------------

def stage_of(recs, r):
    at = r["parent"]
    for _ in range(5):
        x = next((x for x in recs if x["id"] == at), None)
        if not x:
            break
        if x["type"] == "Stage":
            return x["id"]
        at = x["parent"]
    return ""


def deploy_target(name):
    """Where a stage named for a deployment deploys to: Deploy_Staging is Staging, Deployment says
    nowhere (""). None: not named so."""
    low = name.lower()
    if not low.startswith("deploy"):
        return None
    return name[10 if low.startswith("deployment") else 6:].lstrip("_- :").strip()


def stages_of(recs):
    """Each stage: its name, whether it is through, under way, deploying (a job named Deploy...
    runs) or deployed, held at an approval (Checkpoint.Approval under way) or skipped."""
    deploy_job = lambda r: r["type"] == "Job" and r["name"].lower().startswith("deploy")
    deploying = {stage_of(recs, r) for r in recs if deploy_job(r) and r["state"] == "inProgress"}
    deployed = {stage_of(recs, r) for r in recs if deploy_job(r) and r["state"] == "completed" and r["result"] == "succeeded"}
    held = {stage_of(recs, r) for r in recs if r["type"] == "Checkpoint.Approval" and r["state"] in ("inProgress", "pending")}
    return [{"name": r["name"], "done": r["state"] == "completed", "running": r["state"] == "inProgress",
             "deploying": r["id"] in deploying, "held": r["id"] in held, "skipped": r["result"] == "skipped",
             "deployed": r["id"] in deployed or (r["result"] == "succeeded" and deploy_target(r["name"]) is not None)}
            for r in recs if r["type"] == "Stage"]


def refused_of(recs):
    """Stages whose approval was rejected there, or ran out before anyone gave it."""
    out = []
    for r in recs:
        if r["type"] == "Checkpoint.Approval" and r["state"] == "completed" and r["result"] not in ("", "succeeded", "succeededWithIssues", "skipped"):
            s = next((x for x in recs if x["id"] == stage_of(recs, r)), None)
            if s and s["name"] not in out:
                out.append(s["name"])
    return out


def held_words(run, stages, url):
    return f"{run} waits for your approval of stage{'s' if len(stages) > 1 else ''} {', '.join(stages)} in Azure DevOps: {url}"


def awaiting(read):
    """What waits for the person's approval in Azure DevOps, of timelines read as (name, url,
    records); "" when nothing does."""
    return "; ".join(held_words(name, at, url) for name, url, recs in read
                     for at in [[s["name"] for s in stages_of(recs) if s["held"]]] if at)


def skipped_words(stages):
    names = [s["name"].replace("_", " ") for s in stages if s["skipped"]]
    return f" · {', '.join(names)} skipped" if names else ""


def through(stages):
    run = [s for s in stages if not s["skipped"]]
    return f"{sum(1 for s in run if s['done'])} of {len(run)} stages"


# ---- the pull request's own pipeline ----------------------------------------------------------

def build_state(c):
    st = c.get("policyStatus") or ""
    if st in ("approved", "notapplicable"):
        return "passed"
    if st in ("rejected", "broken"):
        return "failed"
    return "running"


def pr_state(checks):
    states = [build_state(c) for c in checks if c.get("buildId")]
    if not states:
        return ""
    return "failed" if "failed" in states else "passed" if all(s == "passed" for s in states) else "running"


def pr_words(state, stages):
    named = lambda s: deploy_target(s["name"]) or s["name"].replace("_", " ")
    deploying = next((s for s in stages if s["running"] and (s["deploying"] or deploy_target(s["name"]) is not None)), None)
    deployed = [named(s) for s in stages if s["deployed"]]
    held = [s["name"] for s in stages if s["held"]]
    said = ""
    if state == "failed":
        said = "pipeline failed"
    elif state == "running" and held:
        said = f"waits for your approval of stage{'s' if len(held) > 1 else ''} {', '.join(held)} in Azure DevOps"
    elif state == "running" and deploying:
        said = "deploying" if deploy_target(deploying["name"]) == "" else f"deploying {named(deploying)}"
    elif state == "running" and deployed:
        said = f"deployed to {', '.join(deployed)} · pipeline running"
    elif state == "running":
        said = "pipeline running" + (f" · {through(stages)}{skipped_words(stages)}" if stages else "")
    elif state == "passed":
        said = (f"deployed to {', '.join(deployed)}" if deployed else "pipeline passed") + skipped_words(stages)
    return f"Pull request open · {said}" if said else "Pull request open"


def pr_pipeline(a, checks, was=None):
    """What the pull request's own pipeline says now: {state, words, key, approval (what waits for
    the person, with the run's URL), builds}. The timelines are read while a build runs, and once
    when they passed; `was` (the last look) is kept while nothing changed."""
    builds = [c for c in checks if c.get("buildId")]
    state = pr_state(checks)
    key = ",".join(f"{c['buildId']}:{build_state(c)}" for c in builds)
    if was and state != "running" and was.get("key") == key and was.get("words"):
        return was
    read = [(f"Pipeline {c['name']}", build_url(a, c["buildId"]), timeline(a, c["buildId"])) for c in builds] if state in ("running", "passed") else []
    stages = [s for _, _, recs in read for s in stages_of(recs)]
    held = awaiting(read) if state == "running" else ""
    return {"state": state, "words": pr_words(state, stages), "key": key, "approval": held or None,
            "builds": [{"id": c["buildId"], "name": c["name"], "url": c.get("detailsUrl")} for c in builds]}


# ---- the main pipeline: the base branch's runs for a merge commit ------------------------------

def pipeline_ids(a):
    v = json_of(a, "pipelines", "list", "--project", a["project"], "--repository", a["repo"], "--repository-type", "tfsgit")
    return [str(p.get("id")) for p in v if p.get("id")] if isinstance(v, list) else None


def run_of(b):
    return {"id": str(b.get("id")), "pipeline": (b.get("definition") or {}).get("name") or "", "pipeline_id": str((b.get("definition") or {}).get("id") or ""),
            "status": str(b.get("status") or "").lower(), "result": str(b.get("result") or "").lower(), "commit": b.get("sourceVersion") or ""}


def runs_of_commit(a, base, commit):
    """The runs of `base` for `commit`, newest first, of this repo's pipelines only. None: az failed."""
    ids = pipeline_ids(a) or []
    args = ["pipelines", "runs", "list", "--project", a["project"], "--branch", f"refs/heads/{base}", "--top", "50",
            "--query-order", "QueueTimeDesc"]
    if ids:
        args += ["--pipeline-ids", *ids]
    v = json_of(a, *args)
    if not isinstance(v, list):
        return None
    return [r for r in map(run_of, v) if r["commit"].lower() == (commit or "").lower()]


def named(setting):
    return [x for x in re.split(r"[,\s]+", str(setting or "all")) if x]


def wanted(runs, setting):
    """The runs the setting waits for (all, or the pipelines it names), the newest of each."""
    names = [n.lower() for n in named(setting)]
    out = []
    for r in runs:
        if "all" not in names and r["pipeline"].lower() not in names and r["pipeline_id"] not in names:
            continue
        if not any(o["pipeline_id"] == r["pipeline_id"] for o in out):
            out.append(r)
    return out


def not_started(runs, setting):
    return [n for n in named(setting) if n.lower() != "all" and not any(n.lower() == r["pipeline"].lower() or n == r["pipeline_id"] for r in runs)]


def passed(r):
    return r["status"] == "completed" and r["result"] in ("succeeded", "partiallysucceeded")


def failed(r):
    return r["status"] == "completed" and not passed(r)


def progress(stages, runs):
    word = "Main pipelines" if runs > 1 else "Main pipeline"
    to = next((deploy_target(s["name"]) for s in stages if s["running"] and deploy_target(s["name"]) is not None), None)
    if to is not None:
        return f"Deploying {to}" if to else "Deploying"
    s = next((s for s in stages if s["running"] and s["deploying"]), None)
    if s:
        return f"Deploying {s['name'].replace('_', ' ')}"
    return f"{word} running" + (f" · {through(stages)}{skipped_words(stages)}" if stages else "")


def look(a, runs, missing):
    """What the runs say together: {"is": passed|running|approval|failed|refused|not_started,
    "words", "detail", "started" (a job has an agent), "names" (not started)}."""
    bad = next((r for r in runs if failed(r)), None)
    if bad:
        refused = refused_of(timeline(a, bad["id"]))
        if refused:
            why = f"The approval of stage{'s' if len(refused) > 1 else ''} {', '.join(refused)} was rejected or ran out in Azure DevOps (run {bad['id']})"
            return {"is": "refused", "words": why, "detail": f"{why}: {build_url(a, bad['id'])}"}
        why = f"The pipeline {bad['pipeline']} {'was cancelled' if bad['result'] == 'canceled' else 'failed'} (run {bad['id']})"
        return {"is": "failed", "words": why, "detail": f"{why}: {build_url(a, bad['id'])}\n\n{tail(a, bad['id'])}"}
    going = [r for r in runs if not passed(r)]
    if not runs or (not going and missing):
        return {"is": "not_started", "names": list(missing)}
    if not going:
        return {"is": "passed", "words": ", ".join(f"{r['pipeline']} {'partly succeeded' if r['result'] == 'partiallysucceeded' else 'passed'}" for r in runs)}
    read = [(f"Main pipeline {r['pipeline']}", build_url(a, r["id"]), timeline(a, r["id"])) for r in going]
    held = awaiting(read)
    if held:
        return {"is": "approval", "words": held}
    stages = [s for _, _, recs in read for s in stages_of(recs)]
    started = any(x["type"] == "Job" and x["state"] in ("inProgress", "completed") for _, _, recs in read for x in recs)
    return {"is": "running", "words": progress(stages, len(going)), "started": started}
