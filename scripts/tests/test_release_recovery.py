import copy
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("recovery", Path(__file__).parents[1] / "release-packaging-recovery.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        self.run = {"id": module.SOURCE_RUN, "head_sha": module.REVISION,
                    "head_branch": "v" + module.VERSION, "event": "push", "workflow_id": 42,
                    "repository": {"full_name": module.REPOSITORY}, "run_attempt": 2,
                    "status": "completed", "conclusion": "failure"}
        self.jobs = [{"name": name, "conclusion": "success"} for name in module.REQUIRED]
        self.jobs += [{"name": "Sign combined macOS installers", "conclusion": "failure"},
                      {"name": "publish-draft", "conclusion": "skipped"}]

    def test_accepts_only_the_retained_qualified_release(self):
        module.validate_source(self.run, 42, self.jobs)

    def test_rejects_other_revisions_runs_events_and_workflows(self):
        for key, value in [("id", 1), ("head_sha", "b" * 40), ("head_branch", "dev"),
                           ("event", "pull_request"), ("workflow_id", 13), ("run_attempt", 3),
                           ("status", "in_progress"), ("repository", {"full_name": "other/repo"})]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                module.validate_source(dict(self.run, **{key: value}), 42, self.jobs)

    def test_rejects_incomplete_cancelled_or_failed_native_acceptance(self):
        for name in module.REQUIRED:
            for conclusion in [None, "cancelled", "failure", "skipped"]:
                jobs = copy.deepcopy(self.jobs)
                next(job for job in jobs if job["name"] == name)["conclusion"] = conclusion
                with self.subTest(name=name, conclusion=conclusion), self.assertRaises(ValueError):
                    module.validate_source(self.run, 42, jobs)

    def test_cannot_replace_a_completed_signer_or_active_publisher(self):
        for name, conclusion in [("Sign combined macOS installers", "success"),
                                 ("publish-draft", "success"), ("publish-draft", None)]:
            jobs = copy.deepcopy(self.jobs)
            next(job for job in jobs if job["name"] == name)["conclusion"] = conclusion
            with self.subTest(name=name), self.assertRaises(ValueError):
                module.validate_source(self.run, 42, jobs)


if __name__ == "__main__":
    unittest.main()
