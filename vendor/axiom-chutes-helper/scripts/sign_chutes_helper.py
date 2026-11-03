#!/usr/bin/env python3
"""Sign a verified native Mac payload before generating its final inventory."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'providers/chutes'))
from axiom_chutes.release import require, sha256
from axiom_chutes.wire import canonical
from combine_chutes_helpers import inspect_bundle, inventory


def macho(path):
    return path.is_file() and not path.is_symlink() and path.read_bytes()[:4].hex() in {
        'cffaedfe', 'cefaedfe', 'cafebabe', 'cafebabf'}


def sign(args):
    require(sys.platform == 'darwin', 'sign on a native macOS runner')
    require(not args.output.exists(), 'signed output already exists')
    original = json.loads((args.input / 'bundle.json').read_bytes())
    require(original.get('architecture') in {'x64', 'arm64'}, 'sign individual native inputs before combining')
    manifest, native = inspect_bundle(args.input, original['architecture'])
    with tempfile.TemporaryDirectory(prefix='axiom-helper-sign-') as temporary:
        payload = Path(temporary) / 'payload'
        shutil.copytree(args.input, payload, symlinks=True)
        files = [path for path in sorted((payload / 'runtime').rglob('*')) if macho(path)]
        command = ['codesign', '--force', '--options', 'runtime', '--timestamp', '--sign', args.codesign_identity]
        for file in files:
            subprocess.run([*command, str(file)], check=True)
        frameworks = [path for path in (payload / 'runtime').rglob('*.framework') if path.is_dir() and not path.is_symlink()]
        for framework in sorted(frameworks, key=lambda path: len(path.parts), reverse=True):
            subprocess.run([*command, str(framework)], check=True)
        for file in files:
            subprocess.run(['codesign', '--verify', '--strict', str(file)], check=True)
        native['helper_sha256'] = sha256((payload / native['helper']).read_bytes())
        (payload / 'native.json').write_bytes(canonical(native))
        manifest['files'] = inventory(payload)
        (payload / 'bundle.json').write_bytes(canonical(manifest))
        args.output.parent.mkdir(parents=True, exist_ok=True)
        shutil.move(str(payload), args.output)
    return {'signed': True, 'architecture': manifest['architecture'], 'inventory_regenerated': True,
        'output': str(args.output), 'signed_installer_qualified': False}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True); parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--codesign-identity', required=True)
    print(json.dumps(sign(parser.parse_args())))
