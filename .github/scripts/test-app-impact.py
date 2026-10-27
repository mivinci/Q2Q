"""Regression tests for source and dependency gates without modifying Git history."""
import contextlib
import importlib.util
import io
import json
import os
import sys
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location('impact', Path(__file__).with_name('app-impact.py'))
impact = importlib.util.module_from_spec(spec)
spec.loader.exec_module(impact)


class ImpactTests(unittest.TestCase):
    def decision(self, app, paths, npm_changed=False, cargo_changed=False):
        def npm(ref, name):
            workspaces = [f'apps/{name}']
            if name in ('admin', 'platform'):
                workspaces += ['packages/ui','packages/plans','packages/accounts']
            if name in ('admin','registry','track'):
                workspaces.append('packages/punch-management')
            if name in ('track', 'registry', 'platform', 'admin'):
                workspaces.append('packages/punch-discovery')
            if name == 'track':
                workspaces.append('apps/registry')  # Real RPC integration test dependency.
            return ('changed' if ref == 'HEAD' and npm_changed else 'same', workspaces)

        def cargo(ref, roots=('q2q-wasm',)):
            return json.dumps([{'name': 'q2q-core', 'version': 'changed' if ref == 'HEAD' and cargo_changed else 'same'}])

        with tempfile.TemporaryDirectory() as directory:
            output, summary = Path(directory) / 'output', Path(directory) / 'summary'
            with patch.dict(os.environ, {'BASE_SHA': 'base', 'EVENT_NAME': 'push', 'GITHUB_OUTPUT': str(output), 'GITHUB_STEP_SUMMARY': str(summary)}), \
                 patch.object(impact.sys, 'argv', ['app-impact.py', app]), \
                 patch.object(impact, 'git', return_value='\n'.join(paths).encode()), \
                 patch.object(impact, 'npm_closure', side_effect=npm), \
                 patch.object(impact, 'closure', side_effect=cargo), contextlib.redirect_stdout(io.StringIO()):
                impact.main()
            return output.read_text().strip() == 'run=true'

    def test_track_sources_do_not_trigger_other_apps(self):
        for app in ('platform', 'seed-processing', 'registry', 'track'):
            self.assertEqual(self.decision(app, ['apps/track/src/directory.ts']), app == 'track')

    def test_shared_discovery_triggers_both_consumers(self):
        self.assertTrue(self.decision('track', ['packages/punch-discovery/index.ts']))
        self.assertTrue(self.decision('registry', ['packages/punch-discovery/index.ts']))
        self.assertTrue(self.decision('platform', ['packages/punch-discovery/index.ts']))
        self.assertFalse(self.decision('seed-processing', ['packages/punch-discovery/index.ts']))

    def test_registry_sources_retest_real_binding_consumer(self):
        self.assertTrue(self.decision('track', ['apps/registry/src/worker.ts']))
        self.assertTrue(self.decision('registry', ['apps/registry/src/worker.ts']))

    def test_native_control_sources_and_shared_fixtures(self):
        self.assertTrue(self.decision('track', ['crates/q2q-core/src/control.rs']))
        self.assertTrue(self.decision('track', ['tests/fixtures/track/v1.json']))
        self.assertFalse(self.decision('track', ['crates/q2q-cli/src/main.rs']))

    def test_track_deployment_gate_changes_trigger_track(self):
        self.assertTrue(self.decision('track', ['.github/scripts/wait-registry.py']))
        self.assertTrue(self.decision('track', ['.github/scripts/test-wait-registry.py']))
        self.assertTrue(self.decision('track', ['.github/scripts/test-app-impact.py']))

    def test_admin_inputs(self):
        self.assertTrue(self.decision('admin',['packages/ui/src/button/button.svelte']))
        self.assertTrue(self.decision('admin',['packages/punch-management/index.ts']))
        self.assertTrue(self.decision('admin',['apps/platform/migrations/0020.sql']))
        self.assertFalse(self.decision('platform',['apps/admin/src/routes/+page.svelte']))

    def test_lock_changes_compare_resolved_closures(self):
        self.assertFalse(self.decision('track', ['package-lock.json']))
        self.assertTrue(self.decision('track', ['package-lock.json'], npm_changed=True))
        self.assertFalse(self.decision('track', ['Cargo.lock']))
        self.assertTrue(self.decision('track', ['Cargo.lock'], cargo_changed=True))

    def test_existing_platform_and_seed_specific_inputs(self):
        self.assertTrue(self.decision('platform', ['packages/ui/src/assets/logo-light.svg']))
        self.assertTrue(self.decision('seed-processing', ['apps/platform/migrations/0001.sql']))
        self.assertFalse(self.decision('registry', ['docs/en/guide/track.md']))


if __name__ == '__main__':
    unittest.main()
