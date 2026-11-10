#!/usr/bin/env python3
"""Deterministic executable archives, including all direct attestation code."""
import argparse
import hashlib
import json
from pathlib import Path
import zipfile


def build(source, destination, interpreter, module):
    if not interpreter.startswith('/') or '\n' in interpreter or ' ' in interpreter:
        raise ValueError('absolute interpreter path without spaces required')
    destination.parent.mkdir(parents=True,exist_ok=True)
    with destination.open('wb') as out:
        out.write(('#!'+interpreter+'\n').encode())
        with zipfile.ZipFile(out,'w',compression=zipfile.ZIP_DEFLATED,compresslevel=9) as archive:
            files={'__main__.py':f'from {module} import main\nmain()\n'.encode()}
            for path in sorted(source.glob('*.py')):
                if path.is_symlink(): raise ValueError('source symlinks forbidden')
                files['axiom_direct/'+path.name]=path.read_bytes()
            for name,raw in sorted(files.items()):
                info=zipfile.ZipInfo(name,(2026,1,1,0,0,0))
                info.compress_type=zipfile.ZIP_DEFLATED
                info.external_attr=0o100644<<16
                archive.writestr(info,raw)
    destination.chmod(0o755)
    return hashlib.sha256(destination.read_bytes()).hexdigest()


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--python',required=True)
    p.add_argument('--output',type=Path,required=True)
    a=p.parse_args()
    source=Path(__file__).resolve().parents[1]/'providers/direct/axiom_direct'
    print(json.dumps({name:build(source,a.output/name,a.python,module) for name,module in [
        ('axiom-direct-verify','axiom_direct.verify'),('axiom-direct-evidence','axiom_direct.collect'),
        ('axiom-direct-commit-boot','axiom_direct.boot_commit')]}))


if __name__=='__main__': main()
