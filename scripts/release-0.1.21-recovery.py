#!/usr/bin/env python3
"""Complete unpublished 0.1.21 from qualified inputs after fixing QA authentication."""
import json
import os
import subprocess

REPOSITORY = 'astrea-foundation/axiom-desktop'
REVISION = '0cea3d47db3a75c1cab7495af930bc864604a6fc'
SOURCE_RUN = 42696233901
RECOVERY_TAG = 'v0.1.21-packaging.1'
REQUIRED = {
    'live-evaluation', 'linux', 'Native macOS build (x64)', 'Native macOS build (arm64)',
    'windows-desktop / Native Windows build (x64)', 'windows-desktop / Native Windows build (arm64)',
    'windows-desktop / Sign combined Windows installers',
    'windows-desktop / Install combined packages on Windows ARM64',
}


def validate_source(run, workflow_id, jobs):
    if (run.get('id') != SOURCE_RUN or run.get('head_sha') != REVISION
            or run.get('head_branch') != 'v0.1.21' or run.get('event') != 'push'
            or run.get('workflow_id') != workflow_id or run.get('run_attempt') != 1
            or run.get('repository', {}).get('full_name') != REPOSITORY
            or run.get('status') != 'completed' or run.get('conclusion') != 'failure'):
        raise ValueError('Recovery requires the exact completed 0.1.21 source attempt')
    results = {job['name']: job for job in jobs}
    if any(results.get(name, {}).get('conclusion') != 'success' for name in REQUIRED):
        raise ValueError('Native builds, live evaluation and signed Windows acceptance must pass')
    mac = results.get('Sign combined macOS installers', {})
    steps = {step['name']: step.get('conclusion') for step in mac.get('steps', [])}
    if (mac.get('conclusion') != 'failure' or steps.get('Sign and notarize both PKG installers') != 'success'
            or steps.get('Generate and view an authenticated image from both signed installations') != 'failure'):
        raise ValueError('Only the native-session acceptance harness failure may be recovered')
    if results.get('publish-draft', {}).get('conclusion') != 'skipped':
        raise ValueError('The original publisher must not be active or completed')


def main():
    if (os.environ.get('GITHUB_REPOSITORY') != REPOSITORY
            or os.environ.get('GITHUB_REF_TYPE') != 'tag'
            or os.environ.get('GITHUB_REF_NAME') != RECOVERY_TAG):
        raise SystemExit('Dispatch the reviewed 0.1.21 recovery tag')
    for revision in ['HEAD', REVISION]:
        subprocess.run(['git', 'merge-base', '--is-ancestor', revision, 'origin/main'], check=True)
    source = subprocess.check_output(['git', 'rev-parse', 'v0.1.21^{commit}'], text=True).strip()
    if source != REVISION:
        raise SystemExit('The immutable native source tag changed')
    def api(route):
        return json.loads(subprocess.check_output(['gh', 'api', 'repos/' + REPOSITORY + route]))
    validate_source(api('/actions/runs/' + str(SOURCE_RUN)), api('/actions/workflows/release.yml')['id'],
                    api('/actions/runs/' + str(SOURCE_RUN) + '/jobs?per_page=100')['jobs'])
    artifacts = api('/actions/runs/' + str(SOURCE_RUN) + '/artifacts?per_page=100')['artifacts']
    available = {artifact['name'] for artifact in artifacts if not artifact['expired']}
    required = {'macos-native-x64', 'macos-native-arm64', 'macos-helper-x64', 'macos-helper-arm64',
                'axiom-release-linux-x64', 'axiom-release-win-universal', 'axiom-windows-setup'}
    if not required <= available:
        raise SystemExit('Qualified retained inputs are missing or expired')
    print('Qualified retained 0.1.21 inputs at ' + REVISION)


if __name__ == '__main__':
    main()
