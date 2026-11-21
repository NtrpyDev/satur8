#!/usr/bin/env python3
"""Autopilot scan: decide what needs doing this run. No AI, read-only.

Looks at open issues and autopilot pull requests and prints, as GitHub Actions
outputs, the short lists each later job works through:

  triage    issues nobody has sorted yet
  fix       sorted issues (minor or major) that have no fix in progress
  followup  autopilot pull requests whose tests failed or that got review findings
  land      autopilot pull requests that passed everything and can merge
  release   true when merged fixes are waiting for a new version

Everything an outside person wrote (titles, bodies, comments) is treated as
data. This script only reads labels, authors, states and timestamps to decide.
"""

import datetime
import json
import os
import re
import sys
import urllib.error
import urllib.request

REPO = os.environ["GITHUB_REPOSITORY"]
TOKEN = os.environ["GH_TOKEN"]
API = "https://api.github.com"

CODEX_BOT = "chatgpt-codex-connector[bot]"
PR_LABEL = "autopilot"
MAX_PER_RUN = {"triage": 5, "fix": 2, "followup": 2, "land": 2}
MAX_ROUNDS = 2
# How long to wait for the Codex review before going on without it.
CODEX_WAIT_MINUTES = 60


def api(path, params=""):
    url = f"{API}/repos/{REPO}/{path}{params}"
    request = urllib.request.Request(
        url,
        headers={
            "Authorization": f"Bearer {TOKEN}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        print(f"::warning::GitHub API {path} returned {error.code}", file=sys.stderr)
        return None


def api_all(path, params=""):
    """Every page of a list endpoint, 100 items at a time."""
    items = []
    joiner = "&" if params else "?"
    for page in range(1, 51):
        batch = api(path, f"{params}{joiner}per_page=100&page={page}")
        if not batch:
            break
        items.extend(batch)
        if len(batch) < 100:
            break
    return items


def labels_of(item):
    return {label["name"] for label in item.get("labels", [])}


def minutes_since(timestamp):
    then = datetime.datetime.fromisoformat(timestamp.replace("Z", "+00:00"))
    return (datetime.datetime.now(datetime.timezone.utc) - then).total_seconds() / 60


def ci_state(sha):
    """'success', 'failure', or 'pending' for the ci workflow on this commit."""
    runs = api("actions/runs", f"?head_sha={sha}&per_page=20") or {}
    ci_runs = [run for run in runs.get("workflow_runs", []) if run.get("name") == "ci"]
    if not ci_runs:
        return "pending", None
    latest = max(ci_runs, key=lambda run: run["created_at"])
    if latest["status"] != "completed":
        return "pending", latest
    return ("success" if latest["conclusion"] == "success" else "failure"), latest


def codex_state(number, sha, head_pushed_at):
    """What the Codex review bot said about the current head commit.

    'approved'  it reviewed this commit and has no open findings
    'findings'  it reviewed this commit and left findings
    'waiting'   no verdict on this commit yet, still inside the wait window
    'absent'    no verdict on this commit and the wait window has passed

    Codex keeps a machine-readable state in its summary comment on the pull
    request, naming the commit it reviewed. It reviews when a pull request is
    opened and when asked with an "@codex review" comment, not on every push,
    so a verdict about an older commit says nothing about this one.
    """
    for comment in api_all(f"issues/{number}/comments"):
        if comment["user"]["login"] != CODEX_BOT:
            continue
        match = re.search(r"codex-review-state:v1 (\{.*?\})\s*-->", comment.get("body") or "", re.S)
        if not match:
            continue
        try:
            review = json.loads(match.group(1)).get("code_review") or {}
        except ValueError:
            continue
        if review.get("commit_sha") == sha:
            if review.get("status") != "completed":
                return "waiting" if minutes_since(head_pushed_at) < CODEX_WAIT_MINUTES else "absent"
            return "findings" if review.get("has_unresolved_findings") else "approved"

    findings = [
        comment
        for comment in api_all(f"pulls/{number}/comments")
        if comment["user"]["login"] == CODEX_BOT and comment.get("commit_id") == sha
    ]
    if findings:
        return "findings"
    thumbs = [
        reaction
        for reaction in api_all(f"issues/{number}/reactions")
        if reaction["user"]["login"] == CODEX_BOT
        and reaction["content"] == "+1"
        and reaction["created_at"] >= head_pushed_at
    ]
    if thumbs:
        return "approved"
    return "waiting" if minutes_since(head_pushed_at) < CODEX_WAIT_MINUTES else "absent"


def main():
    work = {"triage": [], "fix": [], "followup": [], "land": []}

    # Oldest first, so nothing waits forever behind newer reports.
    items = api_all("issues", "?state=open&sort=created&direction=asc")
    issues = [item for item in items if "pull_request" not in item]
    pulls = api_all("pulls", "?state=open&sort=created&direction=asc")

    open_fix_branches = {pull["head"]["ref"] for pull in pulls}

    for issue in issues:
        labels = labels_of(issue)
        number = issue["number"]
        if not any(label.startswith("autopilot:") for label in labels):
            work["triage"].append({"number": number})
            continue
        severity = next(
            (name for name in ("minor", "major") if f"autopilot:{name}" in labels), None
        )
        blocked = labels & {"autopilot:needs-noah", "autopilot:skip", "autopilot:working"}
        if severity and not blocked and f"autopilot/issue-{number}" not in open_fix_branches:
            work["fix"].append({"number": number, "severity": severity})

    for pull in pulls:
        labels = labels_of(pull)
        number = pull["number"]
        branch = pull["head"]["ref"]
        same_repo = pull["head"]["repo"] and pull["head"]["repo"]["full_name"] == REPO
        if PR_LABEL not in labels or not same_repo or not branch.startswith("autopilot/"):
            continue
        if labels & {"autopilot:needs-noah", "autopilot:skip"}:
            continue
        sha = pull["head"]["sha"]
        severity = "major" if "autopilot:major" in labels else "minor"
        rounds = sum(1 for label in labels if re.fullmatch(r"autopilot:round-\d", label))
        commit = api(f"commits/{sha}") or {}
        pushed_at = commit.get("commit", {}).get("committer", {}).get("date", pull["updated_at"])

        ci, _ = ci_state(sha)
        entry = {"number": number, "branch": branch, "severity": severity, "rounds": rounds}
        if ci == "pending":
            continue
        if ci == "failure":
            work["followup"].append({**entry, "reason": "tests"})
            continue
        codex = codex_state(number, sha, pushed_at)
        if codex == "waiting":
            continue
        if codex == "findings":
            work["followup"].append({**entry, "reason": "review"})
            continue
        work["land"].append({**entry, "codex": codex})

    # A pull request that has used up its rounds goes to a human instead.
    exhausted = [entry for entry in work["followup"] if entry["rounds"] >= MAX_ROUNDS]
    work["followup"] = [entry for entry in work["followup"] if entry["rounds"] < MAX_ROUNDS]

    changelog = ""
    try:
        with open("CHANGELOG.md", encoding="utf-8") as handle:
            changelog = handle.read()
    except OSError:
        pass
    unreleased = re.search(r"^## Unreleased\n(.*?)(?=^## )", changelog, re.S | re.M)
    release = bool(unreleased and re.search(r"^- ", unreleased.group(1), re.M))

    lines = []
    for name, entries in work.items():
        entries = entries[: MAX_PER_RUN[name]]
        lines.append(f"{name}={json.dumps(entries)}")
        lines.append(f"has_{name}={'true' if entries else 'false'}")
    lines.append(f"exhausted={json.dumps(exhausted)}")
    lines.append(f"has_exhausted={'true' if exhausted else 'false'}")
    lines.append(f"release={'true' if release else 'false'}")

    with open(os.environ.get("GITHUB_OUTPUT", "/dev/stdout"), "a", encoding="utf-8") as out:
        out.write("\n".join(lines) + "\n")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
