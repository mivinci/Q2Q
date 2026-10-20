"""CLI release preparation and validation. No credentials are printed."""
import json
import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path


def command(*args):
    return subprocess.check_output(args, text=True).strip()


def api(path):
    return json.loads(command('gh', 'api', f'repos/{os.environ["GITHUB_REPOSITORY"]}/{path}'))


def version(text):
    if not re.fullmatch(r'\d+\.\d+\.\d+', text) or any(str(int(x)) != x for x in text.split('.')):
        raise ValueError('version must be a stable X.Y.Z without v or leading zeros')
    return tuple(map(int, text.split('.')))


def output(**values):
    with open(os.environ['GITHUB_OUTPUT'], 'a') as stream:
        for key, value in values.items():
            stream.write(f'{key}={value}\n')


def prepare():
    base = command('git', 'rev-parse', 'HEAD')
    requested = os.environ['RELEASE_VERSION']
    manifest = Path('crates/q2q/Cargo.toml')
    current = tomllib.loads(manifest.read_text())['package']['version']
    if version(requested) <= version(current):
        raise ValueError('new CLI version must exceed the current version')
    if subprocess.run(['git', 'show-ref', '--verify', '--quiet', f'refs/tags/v{requested}']).returncode == 0:
        raise ValueError('release tag already exists')
    manifest.write_text(manifest.read_text().replace(f'version = "{current}"', f'version = "{requested}"', 1))
    lock = Path('Cargo.lock')
    old = f'name = "q2q"\nversion = "{current}"'
    if lock.read_text().count(old) != 1:
        raise ValueError('cannot uniquely locate CLI package in Cargo.lock')
    lock.write_text(lock.read_text().replace(old, f'name = "q2q"\nversion = "{requested}"', 1))
    command('git', 'config', 'user.name', 'github-actions[bot]')
    command('git', 'config', 'user.email', '41898282+github-actions[bot]@users.noreply.github.com')
    branch = f'release/cli-v{requested}'
    command('git', 'switch', '-c', branch)
    command('git', 'add', 'crates/q2q/Cargo.toml', 'Cargo.lock')
    command('git', 'commit', '-m', f'chore(cli): release v{requested}')
    # Never force-push a previous version proposal.
    command('git', 'push', 'origin', branch)
    Path('/tmp/cli-release-pr.md').write_text(
        f'Prepare Q2Q CLI v{requested}. Only the CLI manifest and lockfile package version change.\n\n'
        'Review and merge after CI succeeds. Then run **CLI release** on main; '
        'the cli-release Environment approval is required before tagging and publishing.\n')
    url = command('gh', 'pr', 'create', '--base', 'main', '--head', branch,
                  '--title', f'chore(cli): release v{requested}', '--body-file', '/tmp/cli-release-pr.md')
    print(url)
    with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as summary:
        summary.write(f'Version PR: {url}\n')
    # GITHUB_TOKEN-created PRs do not trigger pull_request workflows. Dispatch
    # Rust explicitly on the exact version branch; workflow_dispatch does trigger.
    command('gh', 'workflow', 'run', 'rust.yml', '--ref', branch)
    # Publish the stable app check names without rebuilding unaffected apps.
    for workflow in ('platform.yml', 'seed-processing.yml'):
        command('gh', 'workflow', 'run', workflow, '--ref', branch, '-f', f'base_sha={base}')


def resolve():
    event = os.environ['GITHUB_EVENT_NAME']
    ref = os.environ['GITHUB_REF']
    if event == 'workflow_dispatch' and ref != 'refs/heads/main':
        raise ValueError('manual release must run on main')
    sha = command('git', 'rev-parse', 'HEAD')
    command('git', 'merge-base', '--is-ancestor', sha, 'origin/main')
    current = tomllib.loads(Path('crates/q2q/Cargo.toml').read_text())['package']['version']
    version(current)
    tag = f'v{current}'
    if event == 'push' and ref != f'refs/tags/{tag}':
        raise ValueError('tag must match CLI version')
    locked = tomllib.loads(Path('Cargo.lock').read_text())['package']
    if [p['version'] for p in locked if p['name'] == 'q2q'] != [current]:
        raise ValueError('CLI manifest and Cargo.lock versions differ')
    # Fail closed: referencing a nonexistent Environment would otherwise create
    # an unprotected one automatically. Approval is configured in repo settings.
    environment = api('environments/cli-release')
    reviewers = [r for r in environment.get('protection_rules', []) if r['type'] == 'required_reviewers']
    owner = os.environ['GITHUB_REPOSITORY'].split('/')[0]
    if not reviewers or not any(r['reviewer']['login'] == owner for r in reviewers[0].get('reviewers', [])):
        raise ValueError('cli-release must require approval from the repository owner')
    if reviewers[0].get('prevent_self_review', False):
        raise ValueError('single-maintainer release requires self-review to be allowed')
    if environment.get('can_admins_bypass', True):
        raise ValueError('disable administrator bypass for cli-release')
    runs = api(f'actions/workflows/rust.yml/runs?head_sha={sha}&per_page=100')['workflow_runs']
    trusted = [r for r in runs if r['head_sha'] == sha and r['event'] in ('push', 'workflow_dispatch')]
    if not trusted or max(trusted, key=lambda r: r['id'])['conclusion'] != 'success':
        raise ValueError('latest Rust CI on this exact commit must succeed before releasing; retry after CI completes')
    existing = subprocess.run(['git', 'rev-parse', '--verify', f'refs/tags/{tag}^{{commit}}'], capture_output=True, text=True)
    if existing.returncode == 0 and existing.stdout.strip() != sha:
        raise ValueError('existing tag points to a different commit; tags must never move')
    output(sha=sha, tag=tag, version=current)
    with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as summary:
        summary.write(f'## Release approval requested\nVersion: **{tag}**\nCommit: `{sha}`\n')


if __name__ == '__main__':
    {'prepare': prepare, 'resolve': resolve}[sys.argv[1]]()
