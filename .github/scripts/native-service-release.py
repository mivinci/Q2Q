"""Resolve an immutable native service release from the main push and gate publishing."""
import json
import os
import re
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
import tomllib

SERVICE = os.environ.get('RELEASE_SERVICE', 'punch')
if SERVICE not in ('punch', 'track'):
    raise ValueError('Unknown native service')
PACKAGE = f'q2q-{SERVICE}-server'
TAG_PREFIX = f'{SERVICE}/v'
VERSION = re.compile(r'^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$')


def git(*args):
    return subprocess.check_output(['git', *args], text=True).strip()


def version_at(ref):
    manifest = tomllib.loads(git('show', f'{ref}:crates/{PACKAGE}/Cargo.toml'))
    return manifest['package']['version']


def parse(value):
    match = VERSION.fullmatch(value)
    if not match:
        raise ValueError(f'Expected stable semantic version X.Y.Z, got {value!r}')
    return tuple(map(int, match.groups()))


def resolve():
    before, sha = os.environ['BEFORE_SHA'], os.environ['GITHUB_SHA']
    if os.environ.get('GITHUB_EVENT_NAME') == 'pull_request':
        before = git('merge-base', before, sha)
    if not before or set(before) == {'0'}:
        raise ValueError('Missing push base; cannot safely determine a version change')
    old, new = version_at(before), version_at(sha)
    if old == new:
        print(f'{SERVICE} version unchanged; no release.')
        with open(os.environ['GITHUB_OUTPUT'], 'a') as out:
            out.write('release=false\n')
        return
    if parse(new) <= parse(old):
        raise ValueError(f'{SERVICE} version must increase: {old} -> {new}')
    lock = tomllib.loads(git('show', f'{sha}:Cargo.lock'))
    matches = [p for p in lock['package'] if p['name'] == PACKAGE]
    if len(matches) != 1 or matches[0]['version'] != new:
        raise ValueError(f'Cargo.lock does not match the {SERVICE} package version')
    tags = git('tag', '--list', f'{TAG_PREFIX}*').splitlines()
    for tag in tags:
        found = VERSION.fullmatch(tag.removeprefix(TAG_PREFIX))
        if not found:
            continue
        number = parse(tag.removeprefix(TAG_PREFIX))
        if number > parse(new):
            raise ValueError(f'{tag} is newer than v{new}')
        if number == parse(new) and git('rev-parse', f'{tag}^{{commit}}') != sha:
            raise ValueError(f'{tag} already points to a different commit')
    with open(os.environ['GITHUB_OUTPUT'], 'a') as out:
        out.write(f'release=true\nversion={new}\ntag=v{new}\n')
    print(f'{SERVICE} version {old} -> {new}; verifying release prerequisites.')


def wait_rust():
    repo, sha = os.environ['GITHUB_REPOSITORY'], os.environ['GITHUB_SHA']
    endpoint = f'repos/{repo}/actions/workflows/rust.yml/runs?head_sha={sha}&event=push&per_page=100'
    deadline = time.monotonic() + 1800
    while time.monotonic() < deadline:
        data = json.loads(subprocess.check_output(['gh', 'api', endpoint], text=True))
        matches = [r for r in data['workflow_runs'] if r['head_sha'] == sha and r['head_branch'] == 'main' and r['event'] == 'push']
        if matches:
            run = max(matches, key=lambda r: r['id'])
            if run['status'] == 'completed':
                if run['conclusion'] != 'success':
                    raise RuntimeError(f"Rust CI {run['id']} ended with {run['conclusion']}")
                print('Rust CI succeeded for the release commit.')
                return
        time.sleep(10)
    raise TimeoutError('Rust CI did not succeed within 30 minutes')


def validate_image_tags(tags, target):
    if not target.startswith('v'):
        raise ValueError('Expected image tag vX.Y.Z')
    target_version = parse(target[1:])
    for tag in tags:
        if not isinstance(tag, str) or not tag.startswith('v') or not VERSION.fullmatch(tag[1:]):
            continue
        existing = parse(tag[1:])
        if existing == target_version:
            raise ValueError(f'Image tag {target} already exists; refusing to overwrite it')
        if existing > target_version:
            raise ValueError(f'Image tag {tag} is newer than {target}; refusing to move latest backwards')


def check_image():
    owner = os.environ['GITHUB_REPOSITORY_OWNER'].lower()
    image = f'{owner}/{PACKAGE}'
    scope = urllib.parse.urlencode({'scope': f'repository:{image}:pull'})
    with urllib.request.urlopen(f'https://ghcr.io/token?{scope}', timeout=20) as response:
        token = json.load(response)['token']
    url = f'https://ghcr.io/v2/{image}/tags/list?n=100'
    target = os.environ['RELEASE_TAG']
    # Fail closed if the public registry cannot be read or pagination is malformed.
    for _ in range(100):
        parsed = urllib.parse.urlsplit(url)
        if parsed.scheme != 'https' or parsed.netloc != 'ghcr.io' or parsed.path != f'/v2/{image}/tags/list':
            raise RuntimeError('Unexpected registry pagination URL')
        request = urllib.request.Request(url, headers={'Authorization': f'Bearer {token}'})
        with urllib.request.urlopen(request, timeout=20) as response:
            payload = json.load(response)
            if payload.get('name') != image or not isinstance(payload.get('tags'), list):
                raise RuntimeError('Registry did not return an image tag list')
            validate_image_tags(payload['tags'], target)
            link = response.headers.get('Link')
        if not link:
            print(f'{target} is newer than all existing stable image tags.')
            return
        match = re.fullmatch(r'\s*<([^>]+)>\s*;\s*rel="?next"?\s*', link)
        if not match:
            raise RuntimeError('Cannot parse registry pagination')
        url = urllib.parse.urljoin(url, match.group(1))
    raise RuntimeError('Registry tag pagination limit exceeded')


if __name__ == '__main__':
    import sys
    {'resolve': resolve, 'wait-rust': wait_rust, 'check-image': check_image}[sys.argv[1]]()
