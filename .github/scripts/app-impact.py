"""Gate app CI on its sources and resolved npm/WASM dependency closures."""
import fnmatch
import json
import os
import posixpath
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


def npm_closure(ref, app):
    lock = json.loads(git('show', f'{ref}:package-lock.json'))
    if lock['lockfileVersion'] != 3:
        raise ValueError('unsupported npm lock format')
    packages = lock['packages']

    def resolve(directory, name):
        # Match Node's ancestor lookup, including nested/scoped installations.
        while True:
            candidate = posixpath.join(directory, 'node_modules', name)
            if candidate in packages:
                return candidate
            if not directory:
                return None
            directory = posixpath.dirname(directory)
            if posixpath.basename(directory) == 'node_modules':
                directory = posixpath.dirname(directory)

    pending, found = [f'apps/{app}'], {}
    while pending:
        path = pending.pop()
        if path in found:
            continue
        package = packages[path]
        found[path] = package
        if package.get('link'):
            pending.append(package['resolved'])
            continue
        dependencies = dict(package.get('dependencies', {}))
        dependencies.update(package.get('optionalDependencies', {}))
        dependencies.update(package.get('peerDependencies', {}))
        # Workspace build tools are installed too; registry packages' dev tools aren't.
        if 'node_modules/' not in path:
            dependencies.update(package.get('devDependencies', {}))
        for name in dependencies:
            # Bundled modules live inside the tarball and are covered by its integrity.
            bundled = package.get('bundleDependencies', package.get('bundledDependencies', []))
            if bundled is True or (isinstance(bundled, list) and name in bundled):
                continue
            target = resolve(path, name)
            if target is None:
                if (name in package.get('optionalDependencies', {})
                        or package.get('peerDependenciesMeta', {}).get(name, {}).get('optional')):
                    continue
                raise ValueError(f'unresolved dependency: {path}: {name}')
            pending.append(target)
    workspaces = [path for path in found if 'node_modules/' not in path]
    return json.dumps(found, sort_keys=True), workspaces


def main():
    app = sys.argv[1]
    if app not in ('platform', 'seed-processing', 'registry'):
        raise ValueError('unknown app')
    reason = 'manual run or missing base: run conservatively'
    run = True
    base = os.environ.get('BASE_SHA', '')
    if base and set(base) != {'0'}:
        try:
            if os.environ.get('EVENT_NAME') == 'pull_request':
                base = git('merge-base', base, 'HEAD').decode().strip()
            paths = git('diff', '--name-only', '--no-renames', base, 'HEAD').decode().splitlines()
            before_npm, before_workspaces = npm_closure(base, app)
            after_npm, after_workspaces = npm_closure('HEAD', app)
            shared = ['package.json', 'tsconfig.base.json', '.node-version',
                      '.github/scripts/app-impact.py', f'.github/workflows/{app}.yml']
            shared += [f'{path}/**' for path in set(before_workspaces + after_workspaces)]
            uses_wasm = 'crates/q2q-wasm' in before_workspaces + after_workspaces
            if uses_wasm:
                shared += ['crates/q2q-primitives/**', 'Cargo.toml']
            own = [f'apps/{app}/**']
            if app == 'platform':
                own += ['docs/public/logo-light.svg', 'docs/public/logo-dark.svg', 'docs/public/favicon.svg']
            elif app == 'seed-processing':
                own += ['apps/platform/migrations/**', 'apps/platform/scripts/check-processing.mjs']
            run = any(fnmatch.fnmatchcase(path, pattern) for path in paths for pattern in shared + own)
            reason = 'application/shared source changed' if run else 'no application impact'
            if not run and 'package-lock.json' in paths:
                run = before_npm != after_npm
                reason = 'npm dependency closure changed' if run else 'npm dependency closure unchanged'
            if not run and uses_wasm and 'Cargo.lock' in paths:
                run = closure(base) != closure('HEAD')
                reason = 'WASM dependency closure changed' if run else 'WASM dependency closure unchanged'
        except (subprocess.CalledProcessError, ValueError, KeyError, TypeError) as error:
            run, reason = True, f'cannot determine impact: {type(error).__name__}'
    print(f'{app}: run={str(run).lower()} ({reason})')
    with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
        output.write(f'run={str(run).lower()}\n')
    with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as summary:
        summary.write(f'### {app}\nRun: **{run}** — {reason}\n')


if __name__ == '__main__':
    main()
