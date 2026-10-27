"""Wait for this main push's Registry workflow before deploying its RPC consumer."""
import json
import os
import subprocess
import time


def registry_state(runs, sha):
    matches = [run for run in runs if run['head_sha'] == sha
               and run['head_branch'] == 'main' and run['event'] == 'push']
    if not matches:
        return 'waiting'
    latest = max(matches, key=lambda run: run['id'])
    if latest['status'] != 'completed':
        return 'waiting'
    if latest['conclusion'] != 'success':
        raise RuntimeError(f"Registry run {latest['id']} ended with {latest['conclusion']}")
    return 'success'


def main():
    repo, sha = os.environ['GITHUB_REPOSITORY'], os.environ['GITHUB_SHA']
    endpoint = f'repos/{repo}/actions/workflows/registry.yml/runs?head_sha={sha}&event=push&per_page=100'
    deadline = time.monotonic() + 900
    while time.monotonic() < deadline:
        data = json.loads(subprocess.check_output(['gh', 'api', endpoint]))
        if registry_state(data['workflow_runs'], sha) == 'success':
            print('Registry workflow succeeded for this main commit; deploying the RPC consumer.')
            return
        time.sleep(10)
    raise TimeoutError('Registry workflow did not succeed within 15 minutes; RPC consumer was not deployed')


if __name__ == '__main__':
    main()
