#!/usr/bin/env python3
"""Combine two qualified Mac payloads behind a pinned universal launcher.

Each frozen Python runtime remains intact. The launcher verifies the selected
child's executable hash before exec, and the installer seals the full inventory.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'providers/chutes'))
from axiom_chutes.release import require, sha256
from axiom_chutes.wire import canonical

LAUNCHER = r'''
#include <CommonCrypto/CommonDigest.h>
#include <mach-o/dyld.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
int main(int argc, char **argv) {
  char source[PATH_MAX], resolved[PATH_MAX], child[PATH_MAX];
  uint32_t size = sizeof(source);
  if (_NSGetExecutablePath(source, &size) || !realpath(source, resolved)) return 126;
  char *slash = strrchr(resolved, '/');
  if (!slash) return 126;
  *slash = 0;
#if defined(__arm64__)
  const char *architecture = "arm64", *expected = "@ARM_HASH@";
#elif defined(__x86_64__)
  const char *architecture = "x64", *expected = "@X64_HASH@";
#else
#error Unsupported Mac helper architecture
#endif
  int count = snprintf(child, sizeof(child), "%s/%s/axiom-chutes-helper", resolved, architecture);
  if (count < 0 || count >= sizeof(child)) return 126;
  FILE *file = fopen(child, "rb");
  if (!file) return 126;
  CC_SHA256_CTX context; CC_SHA256_Init(&context);
  unsigned char buffer[65536], hash[CC_SHA256_DIGEST_LENGTH];
  size_t bytes;
  while ((bytes = fread(buffer, 1, sizeof(buffer), file))) CC_SHA256_Update(&context, buffer, (CC_LONG)bytes);
  int failed = ferror(file); fclose(file);
  if (failed) return 126;
  CC_SHA256_Final(hash, &context);
  char actual[65];
  for (size_t index = 0; index < sizeof(hash); index++) snprintf(actual + 2 * index, 3, "%02x", hash[index]);
  if (strcmp(actual, expected)) return 126;
  argv[0] = child;
  execv(child, argv);
  return 126;
}
'''


def inventory(directory):
    rows = []
    for path in sorted(directory.rglob('*')):
        name = path.relative_to(directory).as_posix()
        if name == 'bundle.json':
            continue
        if path.is_symlink():
            target = os.readlink(path)
            require(not Path(target).is_absolute() and path.resolve().is_relative_to(directory.resolve()), 'runtime symlink escapes bundle')
            rows.append({'path': name, 'symlink': target})
        elif path.is_file():
            rows.append({'path': name, 'bytes': path.stat().st_size, 'sha256': sha256(path.read_bytes())})
    return rows


def inspect_bundle(directory, architecture):
    require(not (directory / 'bundle.json').is_symlink(), 'regular bundle inventory required')
    manifest = json.loads((directory / 'bundle.json').read_bytes())
    require(manifest['schema'] == 'axiom-chutes-helper-bundle-v1' and manifest['platform'] == 'darwin'
        and manifest['architecture'] == architecture and manifest['self_test']['passed'] is True
        and manifest['self_test']['frozen'] is True, 'qualified native Mac payload required')
    require(manifest['files'] == inventory(directory), 'native payload inventory differs')
    native = json.loads((directory / 'native.json').read_bytes())
    require(native['helper'] == 'runtime/axiom-chutes-helper'
        and native['helper_sha256'] == sha256((directory / native['helper']).read_bytes()), 'pinned native child required')
    return manifest, native


def combine(args):
    require(sys.platform == 'darwin', 'assemble and sign the universal launcher on macOS')
    require(not args.output.exists(), 'bundle output already exists')
    arm, arm_native = inspect_bundle(args.arm64, 'arm64')
    intel, intel_native = inspect_bundle(args.x64, 'x64')
    for field in ['repository', 'channel', 'authority_fingerprint']:
        require(arm[field] == intel[field], 'native bundle authorities differ')
    require({k: v for k, v in arm_native.items() if k != 'helper_sha256'} ==
        {k: v for k, v in intel_native.items() if k != 'helper_sha256'}, 'native public settings differ')
    with tempfile.TemporaryDirectory(prefix='axiom-helper-universal-') as temporary:
        temporary = Path(temporary)
        payload = temporary / 'payload'; payload.mkdir()
        (payload / 'runtime').mkdir()
        for architecture, source in [('arm64', args.arm64), ('x64', args.x64)]:
            shutil.copytree(source / 'runtime', payload / 'runtime' / architecture, symlinks=True)
        common_arm = {row['path']: row for row in arm['files'] if not row['path'].startswith('runtime/') and row['path'] != 'native.json'}
        common_intel = {row['path']: row for row in intel['files'] if not row['path'].startswith('runtime/') and row['path'] != 'native.json'}
        require(common_arm == common_intel, 'public configuration or dependency notices differ')
        for name in common_arm:
            destination = payload / name; destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(args.arm64 / name, destination)
        source = temporary / 'launcher.c'
        source.write_text(LAUNCHER.replace('@ARM_HASH@', arm_native['helper_sha256']).replace('@X64_HASH@', intel_native['helper_sha256']))
        executable = payload / 'runtime/axiom-chutes-helper'
        subprocess.run(['xcrun', 'clang', '-O2', '-arch', 'arm64', '-arch', 'x86_64',
            '-mmacosx-version-min=11.0', '-Wno-deprecated-declarations', str(source), '-o', str(executable)], check=True)
        if args.codesign_identity:
            subprocess.run(['codesign', '--force', '--options', 'runtime', '--timestamp', '--sign', args.codesign_identity, str(executable)], check=True)
        native = {**arm_native, 'helper_sha256': sha256(executable.read_bytes())}
        (payload / 'native.json').write_bytes(canonical(native))
        result = subprocess.run([str(executable), '--self-test'], cwd=temporary,
            env={'PATH': '/usr/bin:/bin:/usr/sbin:/sbin'}, capture_output=True, text=True, check=True, timeout=30)
        report = json.loads(result.stdout)
        require(report['passed'] is True and report['frozen'] is True, 'universal launcher self test failed')
        manifest = {k: arm[k] for k in ['schema', 'platform', 'repository', 'channel', 'authority_fingerprint']}
        manifest.update(architecture='universal', self_test=report, files=inventory(payload),
            native_children={'arm64': arm_native['helper_sha256'], 'x64': intel_native['helper_sha256']})
        (payload / 'bundle.json').write_bytes(canonical(manifest))
        args.output.parent.mkdir(parents=True, exist_ok=True)
        shutil.move(str(payload), args.output)
    return {'output': str(args.output), 'architecture': 'universal', 'self_test_passed': True,
        'native_child_hashes_verified': True, 'signed_installer_qualified': False}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--arm64', type=Path, required=True); parser.add_argument('--x64', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True); parser.add_argument('--codesign-identity')
    print(json.dumps(combine(parser.parse_args())))
