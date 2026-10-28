"""Release guard checks against actual Git commits and tags."""
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('punch_release', Path(__file__).with_name('native-service-release.py'))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseGuardTests(unittest.TestCase):
    def setUp(self):
        self.service_patches = [patch.object(release, 'SERVICE', 'punch'), patch.object(release, 'PACKAGE', 'q2q-punch-server'), patch.object(release, 'TAG_PREFIX', 'punch/v')]
        for item in self.service_patches:
            item.start()
            self.addCleanup(item.stop)
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.cwd = os.getcwd()
        os.chdir(self.tmp.name)
        self.addCleanup(os.chdir, self.cwd)
        subprocess.run(['git', 'init', '-q'], check=True)
        subprocess.run(['git', 'config', 'user.name', 'Test'], check=True)
        subprocess.run(['git', 'config', 'user.email', 'test@example.invalid'], check=True)
        self.manifest = Path('crates/q2q-punch-server/Cargo.toml')
        self.manifest.parent.mkdir(parents=True)
        self.write_version('0.1.0')
        self.old = self.commit('initial')
        self.output = Path('output')
        self.patch = patch.dict(os.environ, {'BEFORE_SHA': self.old, 'GITHUB_OUTPUT': str(self.output), 'GITHUB_EVENT_NAME': 'push'})
        self.patch.start()
        self.addCleanup(self.patch.stop)

    def write_version(self, manifest, lock=None):
        self.manifest.write_text(f'[package]\nname = "q2q-punch-server"\nversion = "{manifest}"\n')
        Path('Cargo.lock').write_text(f'[[package]]\nname = "q2q-punch-server"\nversion = "{lock or manifest}"\n')

    def commit(self, message):
        subprocess.run(['git', 'add', 'Cargo.lock', str(self.manifest)], check=True)
        subprocess.run(['git', 'commit', '-qm', message], check=True)
        return subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()

    def resolve(self, sha):
        with patch.dict(os.environ, {'GITHUB_SHA': sha}):
            release.resolve()
        return self.output.read_text()

    def test_unchanged_version_does_not_release(self):
        self.assertEqual(self.resolve(self.old), 'release=false\n')

    def test_version_increase_creates_expected_tag(self):
        self.write_version('0.6.1')
        self.assertIn('tag=v0.6.1\n', self.resolve(self.commit('release')))

    def test_lock_mismatch_and_non_increasing_versions_are_rejected(self):
        self.write_version('0.6.1', '0.1.0')
        with self.assertRaisesRegex(ValueError, 'Cargo.lock'):
            self.resolve(self.commit('bad lock'))
        self.write_version('0.0.9')
        with self.assertRaisesRegex(ValueError, 'must increase'):
            self.resolve(self.commit('downgrade'))

    def test_existing_and_newer_git_tags_are_not_overwritten(self):
        subprocess.run(['git', 'tag', 'punch/v0.6.1', self.old], check=True)
        self.write_version('0.6.1')
        sha = self.commit('release')
        with self.assertRaisesRegex(ValueError, 'different commit'):
            self.resolve(sha)
        subprocess.run(['git', 'tag', '-d', 'punch/v0.6.1'], check=True, capture_output=True)
        subprocess.run(['git', 'tag', 'punch/v0.7.0', self.old], check=True)
        with self.assertRaisesRegex(ValueError, 'newer'):
            self.resolve(sha)

    def test_retry_accepts_same_commit_tag_but_still_checks_registry(self):
        self.write_version('0.6.1')
        sha = self.commit('release')
        subprocess.run(['git', 'tag', 'punch/v0.6.1', sha], check=True)
        self.assertIn('release=true', self.resolve(sha))

    def test_track_version_and_tags_are_independent(self):
        track = Path('crates/q2q-track-server/Cargo.toml')
        track.parent.mkdir(parents=True)
        track.write_text('[package]\nname = "q2q-track-server"\nversion = "0.1.0"\n')
        Path('Cargo.lock').write_text(Path('Cargo.lock').read_text() + '\n[[package]]\nname = "q2q-track-server"\nversion = "0.1.0"\n')
        subprocess.run(['git', 'add', str(track), 'Cargo.lock'], check=True)
        subprocess.run(['git', 'commit', '-qm', 'add track'], check=True)
        before = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
        track.write_text(track.read_text().replace('0.1.0', '0.6.1'))
        Path('Cargo.lock').write_text(Path('Cargo.lock').read_text().replace('name = "q2q-track-server"\nversion = "0.1.0"', 'name = "q2q-track-server"\nversion = "0.6.1"'))
        subprocess.run(['git', 'add', str(track), 'Cargo.lock'], check=True)
        subprocess.run(['git', 'commit', '-qm', 'release track'], check=True)
        sha = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
        subprocess.run(['git', 'tag', 'punch/v0.7.0', before], check=True)
        with patch.object(release, 'SERVICE', 'track'), patch.object(release, 'PACKAGE', 'q2q-track-server'), patch.object(release, 'TAG_PREFIX', 'track/v'), patch.dict(os.environ, {'BEFORE_SHA': before, 'GITHUB_SHA': sha}):
            release.resolve()
        self.assertIn('tag=v0.6.1', self.output.read_text())

    def test_image_tags_must_be_unused_and_newer_than_existing(self):
        release.validate_image_tags(['latest', 'dev', 'v0.6.0'], 'v0.6.1')
        with self.assertRaisesRegex(ValueError, 'already exists'):
            release.validate_image_tags(['v0.6.1'], 'v0.6.1')
        with self.assertRaisesRegex(ValueError, 'newer'):
            release.validate_image_tags(['v0.7.0'], 'v0.6.1')

    def test_only_stable_versions_are_accepted(self):
        for value in ('v1.0.0', '1.0.0-rc1', '01.2.3', '1.2'):
            with self.subTest(version=value), self.assertRaises(ValueError):
                release.parse(value)


if __name__ == '__main__':
    unittest.main()
