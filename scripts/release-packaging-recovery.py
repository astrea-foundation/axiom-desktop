#!/usr/bin/env python3
"""Gate the one-time completion of unpublished 0.1.17 packaging."""
import json
import os
import subprocess

REPOSITORY = "astrea-foundation/axiom-desktop"
REVISION = "7c41a12b31d42aefe4f21896864a166f0d087b3a"
VERSION = "0.1.17"
SOURCE_RUN = 42116622051
RECOVERY_TAG = "v0.1.17-packaging.1"
REQUIRED = {
    "live-evaluation", "linux",
    "Native macOS build (x64)", "Native macOS build (arm64)",
    "windows-desktop / Native Windows build (x64)",
    "windows-desktop / Native Windows build (arm64)",
    "windows-desktop / Sign combined Windows installers",
    "windows-desktop / Install combined packages on Windows ARM64",
}


def validate_source(run, workflow_id, jobs):
    if (run.get("id") != SOURCE_RUN or run.get("head_sha") != REVISION
            or run.get("head_branch") != "v" + VERSION or run.get("event") != "push"
            or run.get("workflow_id") != workflow_id
            or run.get("repository", {}).get("full_name") != REPOSITORY
            or run.get("run_attempt") != 2 or run.get("status") != "completed"
            or run.get("conclusion") != "failure"):
        raise ValueError("Recovery requires the exact completed source release attempt")
    results = {job["name"]: job.get("conclusion") for job in jobs}
    if any(results.get(name) != "success" for name in REQUIRED):
        raise ValueError("Native builds, live evaluation and Windows installation must pass")
    if results.get("Sign combined macOS installers") != "failure":
        raise ValueError("Only the failed unpublished macOS packaging may be recovered")
    if results.get("publish-draft") != "skipped":
        raise ValueError("The original publisher must not be active or completed")


def main():
    if (os.environ.get("GITHUB_REPOSITORY") != REPOSITORY
            or os.environ.get("GITHUB_REF_TYPE") != "tag"
            or os.environ.get("GITHUB_REF_NAME") != RECOVERY_TAG):
        raise SystemExit("Dispatch the reviewed recovery tag in the native repository")
    def git(*args):
        return subprocess.check_output(["git", *args], text=True).strip()
    subprocess.run(["git", "merge-base", "--is-ancestor", "HEAD", "origin/main"], check=True)
    subprocess.run(["git", "merge-base", "--is-ancestor", REVISION, "origin/main"], check=True)
    if git("rev-parse", "v" + VERSION + "^{commit}") != REVISION:
        raise SystemExit("The immutable native source tag changed")
    def api(route):
        return json.loads(subprocess.check_output(["gh", "api", "repos/" + REPOSITORY + route]))
    run = api("/actions/runs/" + str(SOURCE_RUN))
    jobs = api("/actions/runs/" + str(SOURCE_RUN) + "/jobs?per_page=100")["jobs"]
    validate_source(run, api("/actions/workflows/release.yml")["id"], jobs)
    print("Qualified retained native inputs for " + VERSION + " at " + REVISION)


if __name__ == "__main__":
    main()
