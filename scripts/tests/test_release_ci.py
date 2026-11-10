import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("release_ci", Path(__file__).parents[1] / "verify-release-ci.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ReleaseValidationTests(unittest.TestCase):
    def test_combines_targeted_checks_only_for_the_exact_revision(self):
        jobs = {index: [{"name": target, "conclusion": "success" if target == name else "skipped"} for target in module.REQUIRED] for index, name in enumerate(sorted(module.REQUIRED), 1)}
        runs = [{"id": index, "head_sha": "a" * 40} for index in jobs]
        self.assertEqual(module.verify("a" * 40, runs, jobs.__getitem__), sorted(module.REQUIRED))
        with self.assertRaises(ValueError):
            module.verify("b" * 40, runs, jobs.__getitem__)

    def test_partial_or_skipped_checks_do_not_authorize_packaging(self):
        runs = [{"id": 1, "head_sha": "a" * 40}]
        for conclusion in ["skipped", "cancelled", "failure", None]:
            with self.subTest(conclusion=conclusion), self.assertRaises(ValueError):
                module.verify("a" * 40, runs, lambda _: [{"name": name, "conclusion": conclusion} for name in module.REQUIRED])
        with self.assertRaises(ValueError):
            module.verify("a" * 40, runs, lambda _: [{"name": "rust", "conclusion": "success"}])

    def test_newer_failure_overrides_an_older_success(self):
        runs = [{"id": 1, "head_sha": "a" * 40}, {"id": 2, "head_sha": "a" * 40}]
        def jobs(run_id):
            return ([{"name": name, "conclusion": "success"} for name in module.REQUIRED] if run_id == 1 else [{"name": "rust", "conclusion": "failure"}])
        with self.assertRaises(ValueError):
            module.verify("a" * 40, runs, jobs)

    def test_candidate_requires_rust_ui_and_selected_native_platform(self):
        for target, native in module.NATIVE_TARGETS.items():
            with self.subTest(target=target):
                required = module.candidate_checks(target)
                self.assertEqual(required, {"rust", "desktop-ui"} | native)
                runs = [{"id": 1, "head_sha": "a" * 40}]
                jobs = [{"name": name, "conclusion": "success"} for name in required]
                self.assertEqual(module.verify("a" * 40, runs, lambda _: jobs, required), sorted(required))
                for missing in required:
                    with self.assertRaises(ValueError):
                        module.verify("a" * 40, runs, lambda _: [job for job in jobs if job["name"] != missing], required)
                with self.assertRaises(ValueError):
                    module.verify("b" * 40, runs, lambda _: jobs, required)

    def test_candidate_selection_does_not_weaken_stable_release_requirements(self):
        runs = [{"id": 1, "head_sha": "a" * 40}]
        jobs = [{"name": name, "conclusion": "success"} for name in module.candidate_checks("linux")]
        with self.assertRaises(ValueError):
            module.verify("a" * 40, runs, lambda _: jobs)
        self.assertEqual(module.candidate_checks("all"), module.REQUIRED)


if __name__ == "__main__":
    unittest.main()
