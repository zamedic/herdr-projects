"""Azure DevOps, the second host beside GitHub: a repo whose origin is at dev.azure.com (or the
older visualstudio.com) has its pull requests, their policies, votes and completion run through
the Azure CLI (az with its azure-devops extension), as gh is used for GitHub. A pull request is
read into the shape gh gives (`gh pr view --json ...`: state, mergeable, headRefOid,
statusCheckRollup, mergeCommit), so github.py's rules (pr_problem, pr_pending) and the Tasks
panel read both hosts the same way. More: docs/azure-devops.md."""

import json
import os
import re
import subprocess
import tempfile
import time
import urllib.parse

from .base import git

AZ = os.environ.get("HERDR_TASK_AZ") or "az"
LOGIN = ("Install the Azure CLI and its azure-devops extension (az extension add --name azure-devops) "
         "where herdr-task runs, and log in (az login), then try again.")
# Azure DevOps' own application: the resource `az rest` asks a token for.
DEVOPS = "499b84ac-1321-427f-aa17-267ca6975798"
TIMEOUT = 120


# ---- where the repo is ------------------------------------------------------------------------

def parse(url):
    """The Azure DevOps repo an origin URL names, {org, project, repo}; None for another host.
    Every form Azure DevOps gives a clone: git@ssh.dev.azure.com:v3/<org>/<project>/<repo>,
    https://<org>@dev.azure.com/<org>/<project>/_git/<repo>, and the older visualstudio.com ones."""
    u = (url or "").strip().rstrip("/")
    rest = u.split("://", 1)[1] if "://" in u else u
    slash = rest.find("/") if "/" in rest else len(rest)
    at = rest[:slash].rfind("@")
    if at >= 0:
        rest = rest[at + 1:]
    cut = re.search(r"[:/]", rest)
    cut = cut.start() if cut else len(rest)
    host = rest[:cut].lower()
    p = [urllib.parse.unquote(x) for x in re.split(r"[/:]", rest[cut:]) if x]

    def made(org, project, repo):
        repo = repo[:-4] if repo.endswith(".git") else repo
        return {"org": org, "project": project, "repo": repo} if org and project and repo else None
    if host in ("ssh.dev.azure.com", "vs-ssh.visualstudio.com") and len(p) == 4 and p[0] == "v3":
        return made(p[1], p[2], p[3])
    if host == "dev.azure.com":
        if len(p) == 4 and p[2] == "_git":
            return made(p[0], p[1], p[3])
        if len(p) == 3 and p[1] == "_git":
            return made(p[0], p[2], p[2])
    if host.endswith(".visualstudio.com") and host != "vs-ssh.visualstudio.com":
        org = host[: -len(".visualstudio.com")]
        if p and p[0] == "DefaultCollection":
            p = p[1:]
        if len(p) == 2 and p[0] == "_git":
            return made(org, p[1], p[1])
        if len(p) == 3 and p[1] == "_git":
            return made(org, p[0], p[2])
    return None


def enc(s):
    return urllib.parse.quote(s, safe="-._~")


def org_url(a):
    return f"https://dev.azure.com/{enc(a['org'])}"


def web(a):
    return f"{org_url(a)}/{enc(a['project'])}/_git/{enc(a['repo'])}"


def pr_url(a, n):
    return f"{web(a)}/pullrequest/{n}"


def build_url(a, build):
    return f"{org_url(a)}/{enc(a['project'])}/_build/results?buildId={build}"


def pr_api(a, n):
    """A pull request in the REST API, on the repository's route (the one that takes a PATCH)."""
    return f"{org_url(a)}/{enc(a['project'])}/_apis/git/repositories/{enc(a['repo'])}/pullRequests/{n}?api-version=7.1"


def of_folder(folder):
    """The Azure DevOps repo a folder's remote is at (origin, else its first), or None."""
    names = git(folder, "remote")[1].split()
    remote = "origin" if "origin" in names else (names[0] if names else "origin")
    ok, url, _ = git(folder, "config", "--get", f"remote.{remote}.url")
    if not ok or ("azure.com" not in url and "visualstudio.com" not in url):
        return None
    return parse(url)


def is_azure(pr):
    """A task's pull request (its record) is at Azure DevOps."""
    return bool(pr) and pr.get("host") == "azure"


def numbered(pr):
    """How the host writes its number: !12 on Azure DevOps, #12 on GitHub."""
    return f"{'!' if is_azure(pr) else '#'}{(pr or {}).get('number')}"


# ---- the az program ---------------------------------------------------------------------------

def explained(err):
    """az's own words for no login, no extension or no az, as a line that says what to do."""
    e = (err or "").lower()
    if any(x in e for x in ("az login", "az devops login", "refresh token has expired", "tf400813")):
        why = "not logged in to Azure DevOps here"
    elif any(x in e for x in ("is not in the 'az' command group", "misspelled or not recognized", "extension add")):
        why = "az has no azure-devops extension here"
    else:
        return err
    return f"Azure DevOps: {why}. {LOGIN}\n{err}"


def az(args, cwd=None):
    """Runs az, never prompting: (ok, stdout, what went wrong). Tests replace this function."""
    env = {**os.environ, "AZURE_CORE_ONLY_SHOW_ERRORS": "true", "AZURE_CORE_NO_COLOR": "true",
           "AZURE_CORE_COLLECT_TELEMETRY": "false", "GIT_TERMINAL_PROMPT": "0"}
    try:
        r = subprocess.run([AZ, *args], cwd=cwd, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, universal_newlines=True, timeout=TIMEOUT)
    except FileNotFoundError:
        return False, "", f"Azure DevOps: az is not installed here. {LOGIN}"
    except subprocess.TimeoutExpired:
        return False, "", f"az {' '.join(args[:3])} gave no answer in {TIMEOUT}s."
    return r.returncode == 0, r.stdout.strip(), "" if r.returncode == 0 else explained((r.stderr or r.stdout).strip())


def on(a):
    """The organization every call names: never one az would guess from the folder."""
    return ["--org", org_url(a), "--detect", "false", "--output", "json"]


def call(a, *args):
    """az <args> --org ... --output json: (ok, what it said, error)."""
    return az([*args, *on(a)])


def json_of(a, *args):
    """az <args> read as JSON; None when az failed (call() says why)."""
    ok, out, _ = call(a, *args)
    if not ok:
        return None
    try:
        return json.loads(out) if out else {}
    except ValueError:
        return None


def body_file(body):
    fd, path = tempfile.mkstemp(prefix="herdr-task-az-", suffix=".json")
    with os.fdopen(fd, "w") as f:
        json.dump(body, f)
    return path


def invoke(a, area, resource, route, method="GET", body=None):
    """One REST call through `az devops invoke` (a build's timeline and log, a pull request's
    threads). None when it failed."""
    args = ["devops", "invoke", "--area", area, "--resource", resource, "--route-parameters",
            f"project={a['project']}", *[f"{k}={v}" for k, v in route.items()],
            "--http-method", method, "--api-version", "7.1"]
    path = body_file(body) if body is not None else None
    if path:
        args += ["--in-file", path]
    try:
        return json_of(a, *args)
    finally:
        if path:
            os.unlink(path)


def rest(method, url, body):
    """One REST call invoke can't make (a PATCH on a pull request), through `az rest`."""
    path = body_file(body)
    try:
        return az(["rest", "--method", method, "--url", url, "--resource", DEVOPS,
                   "--headers", "Content-Type=application/json", "--body", f"@{path}", "--output", "json"])
    finally:
        os.unlink(path)


_USER = {}


def user():
    """Who az is logged in as (the person), asked once an hour; "" when az can't say."""
    if _USER and time.time() - _USER["at"] < 3600:
        return _USER["login"]
    ok, out, _ = az(["account", "show", "--query", "user.name", "--output", "tsv"])
    _USER.update(login=out.splitlines()[0].strip() if ok and out else "", at=time.time())
    return _USER["login"]


def first_line(err):
    line = next((x.strip() for x in (err or "").splitlines() if x.strip()), "no answer")
    return line.removeprefix("ERROR: ")[:160]


# ---- the pull request -------------------------------------------------------------------------

def branch(ref):
    return ref[len("refs/heads/"):] if (ref or "").startswith("refs/heads/") else (ref or "")


def pr_record(a, n):
    """What a task keeps of its pull request (as github.pr_open's, with its host)."""
    return {"number": int(n), "url": pr_url(a, n), "repo": f"{a['org']}/{a['project']}/{a['repo']}",
            "host": "azure", "azure": a}


def create(a, br, base, title, body):
    """Opens the pull request of `br` into `base`, ready for review, never a draft: its pipeline
    (build validation, a deployment to staging) runs from the moment it exists. Returns
    (record, None) or (None, why)."""
    if len(body) > 4000:  # Azure DevOps takes at most 4000 characters of description
        body = body[:3990] + "…"
    ok, out, err = call(a, "repos", "pr", "create", "--project", a["project"], "--repository", a["repo"],
                        "--source-branch", br, "--target-branch", base, "--title", title,
                        "--description", body, "--draft", "false")
    try:
        n = json.loads(out).get("pullRequestId") if ok else None
    except ValueError:
        n = None
    if not n:
        return None, err or f"az repos pr create gave no pull request: {out[:200]}"
    return pr_record(a, n), None


def show(a, n):
    return json_of(a, "repos", "pr", "show", "--id", str(n))


def mergeable(status):
    return {"conflicts": "CONFLICTING", "succeeded": "MERGEABLE"}.get((status or "").lower(), "UNKNOWN")


def shaped(a, p):
    """One pull request as az gives it, in gh's shape (without its checks)."""
    n = p.get("pullRequestId")
    state = {"completed": "MERGED", "abandoned": "CLOSED"}.get(str(p.get("status") or "").lower(), "OPEN")
    merge = (p.get("lastMergeCommit") or {}).get("commitId")
    return {"number": n, "url": pr_url(a, n), "title": p.get("title"), "state": state, "isDraft": bool(p.get("isDraft")),
            "headRefName": branch(p.get("sourceRefName")), "baseRefName": branch(p.get("targetRefName")),
            "headRefOid": (p.get("lastMergeSourceCommit") or {}).get("commitId") or "",
            # On an open one its last merge commit is only Azure DevOps' trial merge.
            "mergeCommit": {"oid": merge} if state == "MERGED" and merge else None,
            "mergeStatus": p.get("mergeStatus") or "", "mergeable": mergeable(p.get("mergeStatus")),
            "author": (p.get("createdBy") or {}).get("uniqueName") or "",
            "reviewers": [{"name": r.get("displayName"), "login": r.get("uniqueName"), "vote": r.get("vote") or 0,
                           "required": bool(r.get("isRequired")), "group": bool(r.get("isContainer"))}
                          for r in p.get("reviewers") or []]}


def vote_word(v):
    return {10: "approved", 5: "approved with suggestions", -5: "waiting for the author", -10: "rejected"}.get(v, "no vote")


# ---- its policies, read as checks -------------------------------------------------------------

def kind(p):
    """What a policy evaluation is of: "build", "status" or "people"."""
    c = p.get("configuration") or {}
    ty = str((c.get("type") or {}).get("displayName") or "").lower()
    if (c.get("settings") or {}).get("buildDefinitionId") or ty == "build":
        return "build"
    return "status" if ty == "status" else "people"


def policy_name(p):
    c = p.get("configuration") or {}
    s = c.get("settings") or {}
    if s.get("displayName"):
        return s["displayName"]
    if (p.get("context") or {}).get("buildDefinitionName"):
        return p["context"]["buildDefinitionName"]
    if kind(p) == "status" and s.get("statusName"):
        return "/".join(x for x in (s.get("statusGenre"), s.get("statusName")) if x)
    return (c.get("type") or {}).get("displayName") or "a policy"


def check_words(status, blocking):
    """An evaluation's status as gh's (status, conclusion) of a check run. One not blocking
    counts as neutral: it never fails the merge and is never waited for."""
    st = (status or "").lower()
    if st == "approved":
        return "COMPLETED", "SUCCESS"
    if st == "notapplicable":
        return "COMPLETED", "SKIPPED"
    if not blocking:
        return "COMPLETED", "NEUTRAL"
    return {"rejected": ("COMPLETED", "FAILURE"), "broken": ("COMPLETED", "ERROR"),
            "running": ("IN_PROGRESS", ""), "queued": ("QUEUED", "")}.get(st, ("PENDING", ""))


def live(evaluations):
    return [p for p in evaluations if (p.get("configuration") or {}).get("isEnabled") is not False]


def blocking(p):
    return (p.get("configuration") or {}).get("isBlocking") is not False


def rollup(a, evaluations):
    """The evaluations as gh's checks, and whether a blocking policy of people isn't met yet."""
    checks, people = [], False
    for p in live(evaluations):
        st = str(p.get("status") or "").lower()
        if kind(p) == "people":
            people = people or (blocking(p) and st not in ("approved", "notapplicable"))
            continue
        status, conclusion = check_words(st, blocking(p))
        build = str((p.get("context") or {}).get("buildId") or "")
        c = {"__typename": "CheckRun", "name": policy_name(p), "status": status, "conclusion": conclusion,
             "detailsUrl": build_url(a, build) if build else (p.get("context") or {}).get("targetUrl") or "",
             "workflowName": "", "policyStatus": st}
        if build and kind(p) == "build":
            c["buildId"] = build
        checks.append(c)
    return checks, people


def policies(a, n):
    v = json_of(a, "repos", "pr", "policy", "list", "--id", str(n))
    return v if isinstance(v, list) else None


def either(names):
    if len(names) <= 1:
        return "".join(names)
    if len(names) <= 3:
        return f"{', '.join(names[:-1])} or {names[-1]}"
    return f"{', '.join(names[:2])} or {len(names) - 2} more"


def waits(evaluations, reviewers, creator, me):
    """What each blocking policy of people not met still waits for, in words:
    "waits for 1 more reviewer: Bob (policy: Minimum number of reviewers)"."""
    out = []
    approves = lambda r: (r.get("vote") or 0) >= 5
    for p in live(evaluations):
        if kind(p) != "people" or not blocking(p) or str(p.get("status") or "").lower() in ("approved", "notapplicable"):
            continue
        s = (p.get("configuration") or {}).get("settings") or {}
        ty = str(((p.get("configuration") or {}).get("type") or {}).get("displayName") or "").lower()
        name = policy_name(p)
        own = s.get("creatorVoteCounts") is True
        mine = lambda r: bool(creator) and str(r.get("login") or "").lower() == creator.lower()
        counts = lambda r: not r.get("group") and (own or not mine(r))
        void = not own and me and me.lower() == (creator or "").lower() and any(mine(r) and approves(r) for r in reviewers)
        note = "; the creator's own vote does not count" if void else ""
        if "minimum number of reviewers" in ty:
            left = max(1, int(s.get("minimumApproverCount") or 1) - sum(1 for r in reviewers if approves(r) and counts(r)))
            who = [r["name"] for r in reviewers if not approves(r) and counts(r) and r.get("name")]
            out.append(f"waits for {left} more reviewer{'' if left == 1 else 's'}" + (f": {either(who)}" if who else "")
                       + f" (policy: {name}{note})")
        elif "required reviewers" in ty:
            named = [r for r in reviewers if r.get("required")]
            who = [r["name"] for r in named if not approves(r) and (own or not mine(r)) and r.get("name")]
            out.append(f"waits for {either(who) or 'a required reviewer'} (policy: {name}{note})")
        elif "comment" in ty:
            out.append(f"waits for its comments to be resolved (policy: {name})")
        elif "work item" in ty:
            out.append(f"waits for a linked work item (policy: {name})")
        else:
            out.append(f"waits for it to be met (policy: {name})")
    return out


def view(pr):
    """A task's pull request (its record) as `gh pr view --json` gives it, with its policies as
    statusCheckRollup, `waits` (who still has to vote, by policy), `evaluations` (as az gives
    them) and `problem` (a reviewer who rejected it or waits for the author). None: az failed."""
    a = pr["azure"]
    p = show(a, pr["number"])
    if not isinstance(p, dict) or not p.get("pullRequestId"):
        return None
    v = shaped(a, p)
    v["statusCheckRollup"], v["waits"], v["evaluations"] = [], [], []
    if v["state"] == "OPEN":
        ev = policies(a, pr["number"])
        if ev is None:
            return None
        checks, people = rollup(a, ev)
        v["evaluations"] = ev
        v["waits"] = waits(ev, v["reviewers"], v["author"], user() if people else "")
        # A policy of people not met is still waited for, as a check that runs: never a problem.
        v["statusCheckRollup"] = checks + [{"__typename": "CheckRun", "name": w, "status": "PENDING", "conclusion": "",
                                            "people": True} for w in v["waits"]]
    no = [f"{r['name']} {vote_word(r['vote'])}" for r in v["reviewers"] if (r.get("vote") or 0) < 0]
    if no:
        v["problem"] = "A reviewer asked for changes in Azure DevOps: " + ", ".join(no) + "."
    return v


def summary(v):
    """The panel's line: "Reviewers: Ann approved, Shop Team no vote (required) · Policies: CI
    build passed · Waits for Shop Team (policy: Required reviewers)"."""
    def result(c):
        end = (c.get("conclusion") or "").upper()
        return "passed" if end in ("SUCCESS", "NEUTRAL", "SKIPPED") else "failed" if end else "running"
    parts = []
    revs = [f"{r['name']} {vote_word(r['vote'])}" + (" (required)" if r.get("required") else "") for r in v.get("reviewers") or []]
    if revs:
        parts.append("Reviewers: " + ", ".join(revs))
    pol = [f"{c['name']} {result(c)}" for c in v.get("statusCheckRollup") or [] if not c.get("people")]
    if pol:
        parts.append("Policies: " + ", ".join(pol))
    parts += [w.replace("waits for", "Waits for", 1) for w in v.get("waits") or []]
    return " · ".join(parts)


def comments(pr):
    """What people wrote on it (not Azure DevOps' own: votes, pushes), as ghnews' comments, oldest
    first: {id, at, who, body, url}. None when az didn't answer."""
    a = pr["azure"]
    v = invoke(a, "git", "pullRequestThreads", {"repositoryId": a["repo"], "pullRequestId": pr["number"]})
    if not isinstance(v, dict):
        return None
    out = []
    for th in v.get("value") or []:
        where = ((th.get("threadContext") or {}).get("filePath") or "").lstrip("/")
        for c in th.get("comments") or []:
            if c.get("commentType") == "system" or c.get("isDeleted"):
                continue
            at = when(c.get("lastUpdatedDate") or c.get("publishedDate"))
            if at is None:
                continue
            out.append({"id": f"a{th.get('id')}/{c.get('id')}", "at": at, "body": (c.get("content") or "").strip(),
                        "who": ((c.get("author") or {}).get("uniqueName") or "someone")[:64],
                        "url": f"{pr['url']}?discussionId={th.get('id')}", **({"where": where} if where else {})})
    return sorted(out, key=lambda c: (c["at"], c["id"]))


def when(text):
    """Azure DevOps' 2026-11-03T10:00:00.123Z as ms (rounded down to the second), or None."""
    m = re.match(r"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)", str(text or ""))
    if not m:
        return None
    import calendar
    return calendar.timegm(time.strptime(m.group(1), "%Y-%m-%dT%H:%M:%S")) * 1000


def comment(pr, body):
    """A comment of herdr-task's own (the summary), as a thread closed at once: it never holds the
    merge up where comments must be resolved."""
    a = pr["azure"]
    thread = {"comments": [{"parentCommentId": 0, "content": body, "commentType": "text"}], "status": "closed"}
    return invoke(a, "git", "pullRequestThreads", {"repositoryId": a["repo"], "pullRequestId": pr["number"]}, "POST", thread) is not None


# ---- the completion ---------------------------------------------------------------------------

STRATEGIES = {"squash": "squash", "merge": "noFastForward", "rebase": "rebase", "rebase-merge": "rebaseMerge"}


def strategy(method):
    """Settings' method as Azure DevOps names it; None for one it doesn't know (never a guess)."""
    return STRATEGIES.get(str(method or "").strip().lower())


def complete(pr, head, method, message):
    """Completes it at `head` only (Azure DevOps refuses it once the branch moved on), by the merge
    strategy named, its source branch deleted. `az repos pr update` can do neither (it keeps a
    strategy already set and completes whatever was pushed last): the REST call itself, through
    `az rest`, which needs `az login` (a token from `az devops login` alone doesn't do)."""
    body = {"status": "completed", "completionOptions": {"mergeStrategy": strategy(method), "deleteSourceBranch": True,
                                                         "mergeCommitMessage": message}}
    if head:
        body["lastMergeSourceCommit"] = {"commitId": head}
    return rest("patch", pr_api(pr["azure"], pr["number"]), body)


# ---- the person's yes, given in Azure DevOps too ----------------------------------------------

def cast_vote(pr):
    """The Approve vote of the user az is logged in as."""
    return call(pr["azure"], "repos", "pr", "set-vote", "--id", str(pr["number"]), "--vote", "approve")


def reset_vote(pr):
    return call(pr["azure"], "repos", "pr", "set-vote", "--id", str(pr["number"]), "--vote", "reset")


def push_holds(evaluations):
    """Whether a push to the pull request keeps an auto-complete from completing it until it is
    looked at again: a blocking build validation (a new commit needs a new build) or a blocking
    policy of reviewers that resets their votes on a push. Azure DevOps' auto-complete can't be
    pinned to a commit: without such a policy it could complete an unreviewed push, so it isn't
    set then (herdr-task completes it at the reviewed commit itself)."""
    for p in live(evaluations):
        if not blocking(p):
            continue
        if kind(p) == "build" or (kind(p) == "people" and ((p.get("configuration") or {}).get("settings") or {}).get("resetOnSourcePush") is True):
            return True
    return False


def auto_complete(pr, method, message):
    """Its auto-complete, squashed or merged by `method`, its source branch deleted. az can't set a
    rebase: callers don't ask for one then."""
    return call(pr["azure"], "repos", "pr", "update", "--id", str(pr["number"]), "--auto-complete", "true",
                "--squash", "true" if method == "squash" else "false", "--delete-source-branch", "true",
                "--merge-commit-message", message)


def call_off(pr):
    return call(pr["azure"], "repos", "pr", "update", "--id", str(pr["number"]), "--auto-complete", "false")

