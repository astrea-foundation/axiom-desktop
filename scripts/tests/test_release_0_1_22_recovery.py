import copy
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("recovery_0_1_22", Path(__file__).parents[1] / "release-0.1.22-recovery.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        self.run = {"id": module.SOURCE_RUN, "head_sha": module.REVISION,
                    "head_branch": "v0.1.22", "event": "push", "workflow_id": 42,
                    "repository": {"full_name": module.REPOSITORY}, "run_attempt": 2,
                    "status": "completed", "conclusion": "failure"}
        self.jobs = [{"name": name, "conclusion": "success"} for name in module.REQUIRED]
        self.mac = {"name": "Sign combined macOS installers", "conclusion": "failure", "steps": [
            {"name": "Sign and notarize both PKG installers", "conclusion": "success"},
            {"name": "Generate and view an authenticated image from both signed installations", "conclusion": "failure"},
        ]}
        self.jobs += [self.mac, {"name": "publish-draft", "conclusion": "skipped"}]

    def test_accepts_only_the_retained_qualified_release(self):
        module.validate_source(self.run, 42, self.jobs)

    def test_rejects_other_revisions_attempts_events_or_workflows(self):
        for key, value in [("id", 1), ("head_sha", "b" * 40), ("head_branch", "dev"),
                           ("event", "workflow_dispatch"), ("workflow_id", 13), ("run_attempt", 1),
                           ("status", "in_progress"), ("conclusion", "success"),
                           ("repository", {"full_name": "other/repo"})]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                module.validate_source(dict(self.run, **{key: value}), 42, self.jobs)

    def test_rejects_unqualified_native_inputs_and_windows_acceptance(self):
        for name in module.REQUIRED:
            for conclusion in [None, "cancelled", "failure", "skipped"]:
                jobs = copy.deepcopy(self.jobs)
                next(job for job in jobs if job["name"] == name)["conclusion"] = conclusion
                with self.subTest(name=name, conclusion=conclusion), self.assertRaises(ValueError):
                    module.validate_source(self.run, 42, jobs)

    def test_rejects_a_different_mac_failure_or_active_publisher(self):
        for step in self.mac["steps"]:
            previous = step["conclusion"]
            step["conclusion"] = "skipped"
            with self.subTest(step=step["name"]), self.assertRaises(ValueError):
                module.validate_source(self.run, 42, self.jobs)
            step["conclusion"] = previous
        for name, conclusion in [(self.mac["name"], "success"), ("publish-draft", "success"), ("publish-draft", None)]:
            jobs = copy.deepcopy(self.jobs)
            next(job for job in jobs if job["name"] == name)["conclusion"] = conclusion
            with self.subTest(name=name, conclusion=conclusion), self.assertRaises(ValueError):
                module.validate_source(self.run, 42, jobs)
