"""Commit the current measured boot after a valid operator admission lease.

The first boot restores the previous boot links for recovery. This helper
re-arms the exact current candidate only; it accepts no paths or code from an
HTTP request. The Rust worker invokes it after full local verification.
"""
import os
from pathlib import Path
import subprocess

from .strict import decode_json, require


def write_boot_links(host, kernel, initrd):
    """Select files inside the separate boot filesystem, without root aliases.

    Verda reads this partition outside the running guest. Relative targets
    keep the links valid when that partition is mounted by itself.
    """
    host=Path(host)
    require(host.is_dir() and (host/'boot').is_dir() and not (host/'boot').is_symlink(),
        'fixed host boot directory required')
    targets={'vmlinuz':Path(kernel),'initrd.img':Path(initrd)}
    for target in targets.values():
        require(target.parent==Path('/boot') and target.name.startswith('axiom-')
            and '..' not in target.parts,'fixed candidate boot paths required')
        require((host/'boot'/target.name).is_file() and not (host/'boot'/target.name).is_symlink(),
            'ordinary candidate boot artifact required')
    for directory in (host,host/'boot'):
        for name in targets:
            path=directory/name
            require(not path.exists() or path.is_symlink(),'boot alias must be absent or a symlink')
    for name,target in targets.items():
        directory=host/'boot'
        temporary=directory/(name+'.axiom-new')
        temporary.unlink(missing_ok=True)
        temporary.symlink_to(target.name)
        temporary.replace(directory/name)
        (host/name).unlink(missing_ok=True)


def main():
    control=decode_json(Path('/run/axiom-release/boot-control.json').read_bytes(),8192)
    require(set(control)=={'kernel_path','initrd_path','boot_partition'},'measured boot control required')
    for name in ('kernel_path','initrd_path'):
        p=Path(control[name])
        require(p.parent==Path('/boot') and p.name.startswith('axiom-') and '..' not in p.parts,
            'fixed candidate boot paths required')
    host=Path('/run/axiom-host')
    require(host.is_dir() and not (host/'boot').is_symlink(),'fixed host boot mount required')
    subprocess.run(['/usr/bin/mount','-o','remount,rw',str(host)],check=True)
    mounted=False
    try:
        device=control['boot_partition']
        if device:
            import re
            require(re.fullmatch(r'/dev/vd[a-z][0-9]+',device),'fixed boot partition required')
            subprocess.run(['/usr/bin/mount','-t','ext4','-o','rw,nosuid,nodev,noexec',device,str(host/'boot')],check=True)
            mounted=True
        write_boot_links(host,control['kernel_path'],control['initrd_path'])
        # The loader can retry this admitted candidate up to three times even
        # if the following boot dies before any client renews admission.
        import re
        suffix=Path(control['kernel_path']).name.removeprefix('axiom-vmlinuz-')
        require(re.fullmatch(r'[0-9a-f]{16}',suffix),'fixed candidate suffix required')
        marker=host/'var/lib/axiom-release'/('boot-committed-'+suffix)
        require(not marker.is_symlink(),'ordinary recovery marker required')
        temporary=marker.with_suffix('.new')
        temporary.unlink(missing_ok=True)
        with temporary.open('x') as output: output.write('0\n')
        temporary.replace(marker)
        os.sync()
    finally:
        if mounted: subprocess.run(['/usr/bin/umount',str(host/'boot')],check=True)
        subprocess.run(['/usr/bin/mount','-o','remount,ro',str(host)],check=True)


if __name__=='__main__': main()
