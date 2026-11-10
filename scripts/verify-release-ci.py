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

NATIVE_TARGETS = {
    "linux": set(),
    "macos-intel": {"desktop portability (macOS Intel)"},
    "macos-arm64": {"desktop portability (macOS Apple Silicon)"},
    "windows-x64": {"desktop portability (Windows x64)"},
    "windows-arm64": {"desktop portability (Windows ARM64)"},
    "all": REQUIRED - {"rust", "desktop-ui"},
}


def candidate_checks(target):
    return {"rust", "desktop-ui"} | NATIVE_TARGETS[target]


def api(route):
    return json.loads(subprocess.check_output(["gh", "api", "--paginate", "--slurp", route]))


def verify(revision, runs, jobs_for_run, required=REQUIRED):
    # Newest results take precedence, including failures and cancellations.
    results = {}
    for run in sorted(runs, key=lambda item: item["id"], reverse=True):
        if run["head_sha"] != revision:
            continue
        for job in jobs_for_run(run["id"]):
            if job["name"] in required and job["conclusion"] != "skipped":
                results.setdefault(job["name"], job["conclusion"])
    missing = sorted(name for name in required if results.get(name) != "success")
    if missing:
        raise ValueError("Release CI is incomplete for " + revision + ": " + ", ".join(missing))
    return sorted(results)


def main():
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("revision")
    parser.add_argument("--candidate", choices=sorted(NATIVE_TARGETS), help="Require CI for an unsigned candidate's selected platforms")
    args = parser.parse_args()
    revision = args.revision
    repository = os.environ.get("GITHUB_REPOSITORY", "")
    if not re.fullmatch(r"[0-9a-f]{40}", revision) or not re.fullmatch(r"[\w.-]+/[\w.-]+", repository):
        raise SystemExit("usage: GITHUB_REPOSITORY=owner/repo verify-release-ci.py COMMIT_SHA")
    base = "repos/" + repository + "/actions"
    runs = [run for page in api(base + "/workflows/ci.yml/runs?head_sha=" + revision + "&per_page=100") for run in page["workflow_runs"]]
    def jobs(run_id):
        return [job for page in api(base + "/runs/" + str(run_id) + "/jobs?per_page=100") for job in page["jobs"]]
    required = candidate_checks(args.candidate) if args.candidate else REQUIRED
    for name in verify(revision, runs, jobs, required):
        print("Validated: " + name)


if __name__ == "__main__":
    main()
