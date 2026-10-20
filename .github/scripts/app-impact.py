"""Conservative app impact gate; lock-only changes compare the WASM closure."""
import fnmatch
import json
import os
import subprocess
import sys
import tomllib


def git(*args):
    return subprocess.check_output(['git', *args])


def closure(ref):
    lock = tomllib.loads(git('show', f'{ref}:Cargo.lock').decode())
    packages = lock['package']
    roots = [p for p in packages if p['name'] == 'q2q-wasm']
    if len(roots) != 1:
        raise ValueError('ambiguous WASM root')
    pending, found = roots[:], {}
    while pending:
        package = pending.pop()
        key = (package['name'], package['version'], package.get('source', ''))
        if key in found:
            continue
        found[key] = package
        for dep in package.get('dependencies', []):
            parts = dep.split(' ', 2)
            matches = [p for p in packages if p['name'] == parts[0]
                       and (len(parts) < 2 or p['version'] == parts[1])
                       and (len(parts) < 3 or p.get('source') == parts[2].strip('()'))]
            if len(matches) != 1:
                raise ValueError(f'ambiguous dependency: {dep}')
            pending.extend(matches)
    # Include sources/checksums and all locked edges, even optional/dev edges.
    return json.dumps([found[k] for k in sorted(found)], sort_keys=True)


def main():
    app = sys.argv[1]
    if app not in ('platform', 'seed-processing'):
        raise ValueError('unknown app')
    reason = 'manual run or missing base: run conservatively'
    run = True
    base = os.environ.get('BASE_SHA', '')
    if base and set(base) != {'0'}:
        try:
            paths = git('diff', '--name-only', '--no-renames', base, 'HEAD').decode().splitlines()
            shared = ['packages/s3-storage/**', 'packages/seed-processing-contract/**',
                      'crates/q2q-primitives/**', 'crates/q2q-wasm/**', 'Cargo.toml',
                      'package.json', 'package-lock.json', 'tsconfig.base.json', '.node-version',
                      '.github/scripts/app-impact.py', f'.github/workflows/{app}.yml']
            own = [f'apps/{app}/**']
            if app == 'platform':
                own += ['docs/public/logo-light.svg', 'docs/public/logo-dark.svg', 'docs/public/favicon.svg']
            else:
                own += ['apps/platform/migrations/**', 'apps/platform/scripts/check-processing.mjs']
            run = any(fnmatch.fnmatchcase(path, pattern) for path in paths for pattern in shared + own)
            reason = 'application/shared source changed' if run else 'no application impact'
            if not run and 'Cargo.lock' in paths:
                run = closure(base) != closure('HEAD')
                reason = 'WASM dependency closure changed' if run else 'WASM dependency closure unchanged'
        except (subprocess.CalledProcessError, ValueError, KeyError, tomllib.TOMLDecodeError) as error:
            run, reason = True, f'cannot determine impact: {type(error).__name__}'
    print(f'{app}: run={str(run).lower()} ({reason})')
    with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
        output.write(f'run={str(run).lower()}\n')
    with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as summary:
        summary.write(f'### {app}\nRun: **{run}** — {reason}\n')


if __name__ == '__main__':
    main()
