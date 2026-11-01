"""Native process locking and access-control checks for persistent trust state."""
import json
import os
from pathlib import Path
import subprocess
import sys

import pytest

from axiom_chutes.private_state import atomic_state, check_private, private_directory, state_lock


def test_concurrent_processes_preserve_every_atomic_state_update(tmp_path):
    state = tmp_path / "private/counter.json"
    private_directory(state.parent)
    atomic_state(state, b"0")
    program = """import json,sys
from pathlib import Path
from axiom_chutes.private_state import state_lock,atomic_state
p=Path(sys.argv[1])
for _ in range(12):
 with state_lock(p.with_suffix('.lock')):
  value=json.loads(p.read_bytes())
  atomic_state(p,json.dumps(value+1).encode())
"""
    env = dict(os.environ, PYTHONPATH=str(Path(__file__).resolve().parents[1]))
    children = [subprocess.Popen([sys.executable, "-c", program, str(state)], env=env,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE) for _ in range(4)]
    for child in children:
        _, error = child.communicate(timeout=45)
        assert child.returncode == 0, error.decode()
    assert json.loads(state.read_bytes()) == 48
    check_private(state)


@pytest.mark.skipif(os.name == "nt", reason="POSIX permissions; Windows DACL test below")
def test_posix_state_rejects_shared_paths_and_linked_lock_files(tmp_path):
    directory = tmp_path / "private"
    private_directory(directory)
    directory.chmod(0o755)
    with pytest.raises(ValueError, match="permissions"):
        private_directory(directory)
    directory.chmod(0o700)
    outside = tmp_path / "outside"; outside.write_text("unchanged")
    lock = directory / "state.lock"; lock.symlink_to(outside)
    with pytest.raises(OSError):
        with state_lock(lock):
            pytest.fail("linked lock was accepted")
    assert outside.read_text() == "unchanged"


@pytest.mark.skipif(os.name != "nt", reason="requires real Windows access control")
def test_windows_state_rejects_other_users_and_directory_junctions(tmp_path):
    directory = tmp_path / "private"
    private_directory(directory)
    command = ["icacls", str(directory), "/grant", "*S-1-1-0:(RX)"]
    subprocess.run(command, check=True, capture_output=True)
    with pytest.raises(ValueError, match="another identity"):
        check_private(directory, directory=True)
    subprocess.run(["icacls", str(directory), "/remove:g", "*S-1-1-0"], check=True, capture_output=True)
    check_private(directory, directory=True)
    junction = tmp_path / "junction"
    subprocess.run(["cmd", "/c", "mklink", "/J", str(junction), str(directory)], check=True, capture_output=True)
    try:
        with pytest.raises(ValueError, match="reparse"):
            private_directory(junction)
    finally:
        junction.rmdir()
