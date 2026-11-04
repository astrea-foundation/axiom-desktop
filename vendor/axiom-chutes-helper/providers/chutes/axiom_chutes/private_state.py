"""Private, process-locked publisher state on POSIX and Windows.

Windows mode bits do not describe access control. Check the owner and DACL
there, create directories with an explicit protected DACL, and reject reparse
points. Only the current user, SYSTEM and Administrators may have allow ACEs.
"""
from contextlib import contextmanager
import os
from pathlib import Path
import secrets
import stat
import tempfile


def _require(value, message):
    if not value:
        raise ValueError(message)


if os.name == "nt":
    import ctypes
    from ctypes import wintypes as wt
    import msvcrt

    advapi = ctypes.WinDLL("advapi32", use_last_error=True)
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    P = ctypes.c_void_p
    advapi.OpenProcessToken.argtypes = [wt.HANDLE, wt.DWORD, ctypes.POINTER(wt.HANDLE)]
    advapi.OpenProcessToken.restype = wt.BOOL
    advapi.GetTokenInformation.argtypes = [wt.HANDLE, ctypes.c_int, P, wt.DWORD, ctypes.POINTER(wt.DWORD)]
    advapi.GetTokenInformation.restype = wt.BOOL
    advapi.ConvertSidToStringSidW.argtypes = [P, ctypes.POINTER(wt.LPWSTR)]
    advapi.ConvertSidToStringSidW.restype = wt.BOOL
    advapi.ConvertStringSecurityDescriptorToSecurityDescriptorW.argtypes = [wt.LPCWSTR, wt.DWORD, ctypes.POINTER(P), P]
    advapi.ConvertStringSecurityDescriptorToSecurityDescriptorW.restype = wt.BOOL
    advapi.GetNamedSecurityInfoW.argtypes = [wt.LPWSTR, ctypes.c_int, wt.DWORD, ctypes.POINTER(P), P, ctypes.POINTER(P), P, ctypes.POINTER(P)]
    advapi.GetNamedSecurityInfoW.restype = wt.DWORD
    advapi.GetAce.argtypes = [P, wt.DWORD, ctypes.POINTER(P)]
    advapi.GetAce.restype = wt.BOOL
    kernel.GetCurrentProcess.restype = wt.HANDLE
    kernel.CloseHandle.argtypes = [wt.HANDLE]
    kernel.CloseHandle.restype = wt.BOOL
    kernel.LocalFree.argtypes = [P]
    kernel.LocalFree.restype = P

    class SecurityAttributes(ctypes.Structure):
        _fields_ = [("length", wt.DWORD), ("descriptor", P), ("inherit", wt.BOOL)]

    class Acl(ctypes.Structure):
        _fields_ = [("revision", wt.BYTE), ("reserved", wt.BYTE), ("size", wt.WORD), ("count", wt.WORD), ("reserved2", wt.WORD)]

    kernel.CreateDirectoryW.argtypes = [wt.LPCWSTR, ctypes.POINTER(SecurityAttributes)]
    kernel.CreateDirectoryW.restype = wt.BOOL
    kernel.CreateFileW.argtypes = [wt.LPCWSTR, wt.DWORD, wt.DWORD, P, wt.DWORD, wt.DWORD, wt.HANDLE]
    kernel.CreateFileW.restype = wt.HANDLE
    kernel.MoveFileExW.argtypes = [wt.LPCWSTR, wt.LPCWSTR, wt.DWORD]
    kernel.MoveFileExW.restype = wt.BOOL

    def _sid_string(pointer):
        value = wt.LPWSTR()
        _require(advapi.ConvertSidToStringSidW(pointer, ctypes.byref(value)), "Windows state SID unavailable")
        try:
            return value.value
        finally:
            kernel.LocalFree(ctypes.cast(value, P))

    def _current_sid():
        token = wt.HANDLE()
        _require(advapi.OpenProcessToken(kernel.GetCurrentProcess(), 8, ctypes.byref(token)), "Windows process identity unavailable")
        try:
            size = wt.DWORD()
            advapi.GetTokenInformation(token, 1, None, 0, ctypes.byref(size))
            buffer = ctypes.create_string_buffer(size.value)
            _require(advapi.GetTokenInformation(token, 1, buffer, size, ctypes.byref(size)), "Windows process SID unavailable")
            return _sid_string(ctypes.cast(buffer, ctypes.POINTER(P))[0])
        finally:
            kernel.CloseHandle(token)

    @contextmanager
    def _windows_attributes(*, directory):
        current = _current_sid()
        descriptor = P()
        inheritance = "OICI" if directory else ""
        sddl = "O:" + current + "D:P(A;" + inheritance + ";FA;;;" + current + ")"
        _require(advapi.ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl, 1, ctypes.byref(descriptor), None), "private Windows state descriptor unavailable")
        try:
            yield SecurityAttributes(ctypes.sizeof(SecurityAttributes), descriptor, False)
        finally:
            kernel.LocalFree(descriptor)

    def _windows_file(path, disposition):
        with _windows_attributes(directory=False) as attributes:
            handle = kernel.CreateFileW(str(path), 0xC0000000, 7, ctypes.byref(attributes), disposition, 0x00200080, None)
            _require(handle != wt.HANDLE(-1).value, "private Windows state file unavailable")
        try:
            return msvcrt.open_osfhandle(handle, os.O_RDWR | os.O_BINARY)
        except BaseException:
            kernel.CloseHandle(handle)
            raise

    def _windows_private(path):
        metadata = path.lstat()
        _require(not metadata.st_file_attributes & 0x400, "release state must not be a Windows reparse point")
        owner, dacl, descriptor = P(), P(), P()
        result = advapi.GetNamedSecurityInfoW(str(path), 1, 5, ctypes.byref(owner), None, ctypes.byref(dacl), None, ctypes.byref(descriptor))
        _require(result == 0, "Windows state access control unavailable")
        try:
            current = _current_sid()
            _require(_sid_string(owner) == current and bool(dacl), "private Windows state owner and DACL required")
            owner_allowed = False
            for index in range(ctypes.cast(dacl, ctypes.POINTER(Acl)).contents.count):
                ace = P()
                _require(advapi.GetAce(dacl, index, ctypes.byref(ace)), "Windows state ACE unavailable")
                kind = ctypes.c_ubyte.from_address(ace.value).value
                if kind == 1:  # A deny ACE cannot grant another user access.
                    continue
                _require(kind == 0, "unsupported Windows state access rule")
                sid = _sid_string(P(ace.value + 8))
                _require(sid in {current, "S-1-5-18", "S-1-5-32-544"}, "Windows release state grants access to another identity")
                owner_allowed |= sid == current
            _require(owner_allowed, "Windows release state requires current-user access")
        finally:
            kernel.LocalFree(descriptor)


def check_private(path, *, directory=False):
    path = Path(path)
    metadata = path.lstat()
    _require(not path.is_symlink() and (stat.S_ISDIR(metadata.st_mode) if directory else stat.S_ISREG(metadata.st_mode)),
             "private regular release-state path required")
    if os.name == "nt":
        _windows_private(path)
    else:
        _require(stat.S_IMODE(metadata.st_mode) & 0o077 == 0, "private release-state permissions required")


def private_directory(path):
    path = Path(path)
    if os.name != "nt":
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
    elif not path.exists():
        if not path.parent.exists():
            private_directory(path.parent)
        with _windows_attributes(directory=True) as attributes:
            created = kernel.CreateDirectoryW(str(path), ctypes.byref(attributes))
            _require(created or ctypes.get_last_error() == 183, "private Windows state directory creation failed")
    check_private(path, directory=True)


@contextmanager
def state_lock(path):
    path = Path(path)
    private_directory(path.parent)
    if os.name == "nt":
        descriptor = _windows_file(path, 4)
    else:
        import fcntl
        descriptor = os.open(path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "r+b") as lock:
        check_private(path)
        if os.name == "nt":
            lock.seek(0)
            msvcrt.locking(lock.fileno(), msvcrt.LK_LOCK, 1)
        else:
            fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            yield
        finally:
            if os.name == "nt":
                lock.seek(0)
                msvcrt.locking(lock.fileno(), msvcrt.LK_UNLCK, 1)


def atomic_state(path, raw):
    path = Path(path)
    if os.name == "nt":
        # An elevated token may otherwise default new-file ownership to the
        # Administrators group. Pin ownership and the DACL at creation time.
        name = str(path.parent / (".release-state-" + secrets.token_hex(16)))
        descriptor = _windows_file(name, 1)
    else:
        descriptor, name = tempfile.mkstemp(prefix=".release-state-", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
        check_private(Path(name))
        if os.name == "nt":
            _require(kernel.MoveFileExW(name, str(path), 9), "durable Windows release-state replacement failed")
        else:
            os.replace(name, path)
            descriptor = os.open(path.parent, os.O_RDONLY)
            try:
                os.fsync(descriptor)
            finally:
                os.close(descriptor)
    finally:
        Path(name).unlink(missing_ok=True)
