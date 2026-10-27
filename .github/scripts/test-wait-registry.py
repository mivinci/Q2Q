"""Deployment ordering regressions; no network or actual waits."""
import importlib.util
from pathlib import Path
import sys
import unittest

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('wait_registry', Path(__file__).with_name('wait-registry.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def run(id=1, **values):
    return dict(id=id, head_sha='current', head_branch='main', event='push',
                status='completed', conclusion='success') | values


class RegistryGateTests(unittest.TestCase):
    def test_missing_and_running_wait(self):
        self.assertEqual(module.registry_state([], 'current'), 'waiting')
        self.assertEqual(module.registry_state([run(status='in_progress', conclusion=None)], 'current'), 'waiting')

    def test_only_matching_main_push_can_unlock(self):
        for values in [dict(head_sha='old'), dict(head_branch='feature'), dict(event='pull_request'), dict(event='workflow_dispatch')]:
            self.assertEqual(module.registry_state([run(**values)], 'current'), 'waiting')
        self.assertEqual(module.registry_state([run()], 'current'), 'success')

    def test_failures_do_not_unlock(self):
        for conclusion in ['failure', 'cancelled', 'skipped', 'timed_out', None]:
            with self.assertRaises(RuntimeError):
                module.registry_state([run(conclusion=conclusion)], 'current')

    def test_newer_run_wins_over_older_success(self):
        self.assertEqual(module.registry_state([run(1), run(2, status='queued', conclusion=None)], 'current'), 'waiting')
        with self.assertRaises(RuntimeError):
            module.registry_state([run(1), run(2, conclusion='failure')], 'current')


if __name__ == '__main__':
    unittest.main()
