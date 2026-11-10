#!/usr/bin/env python3
"""Require successful CI jobs for the exact promoted release revision."""
import json
import os
import re
import subprocess
import sys

REQUIRED = {
    "rust",
    "desktop-ui",
    "desktop portability (macOS Intel)",
    "desktop portability (macOS Apple Silicon)",
    "desktop portability (Windows x64)",
    "desktop portability (Windows ARM64)",
}


def api(route):
    return json.loads(subprocess.check_output(["gh", "api", "--paginate", "--slurp", route]))


def verify(revision, runs, jobs_for_run):
    # Newest results take precedence, including failures and cancellations.
    results = {}
    for run in sorted(runs, key=lambda item: item["id"], reverse=True):
        if run["head_sha"] != revision:
            continue
        for job in jobs_for_run(run["id"]):
            if job["name"] in REQUIRED and job["conclusion"] != "skipped":
                results.setdefault(job["name"], job["conclusion"])
    missing = sorted(name for name in REQUIRED if results.get(name) != "success")
    if missing:
        raise ValueError("Release CI is incomplete for " + revision + ": " + ", ".join(missing))
    return sorted(results)


def main():
    revision = sys.argv[1] if len(sys.argv) == 2 else ""
    repository = os.environ.get("GITHUB_REPOSITORY", "")
    if not re.fullmatch(r"[0-9a-f]{40}", revision) or not re.fullmatch(r"[\w.-]+/[\w.-]+", repository):
        raise SystemExit("usage: GITHUB_REPOSITORY=owner/repo verify-release-ci.py COMMIT_SHA")
    base = "repos/" + repository + "/actions"
    runs = [run for page in api(base + "/workflows/ci.yml/runs?head_sha=" + revision + "&per_page=100") for run in page["workflow_runs"]]
    def jobs(run_id):
        return [job for page in api(base + "/runs/" + str(run_id) + "/jobs?per_page=100") for job in page["jobs"]]
    for name in verify(revision, runs, jobs):
        print("Validated: " + name)


if __name__ == "__main__":
    main()
