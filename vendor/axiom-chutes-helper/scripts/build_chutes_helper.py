#!/usr/bin/env python3
"""Build an offline, relocatable managed-helper payload on a native runner.

Use the locked bundle dependency group in an isolated environment. Only public
authority/deployment data and frozen client dependencies enter the output.
"""
import argparse
import hashlib
from importlib.metadata import distribution
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import sysconfig
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "providers/chutes"))
from axiom_chutes.client import identifier
from axiom_chutes.publisher import PublisherAuthority
from axiom_chutes.release import require, sha256
from axiom_chutes.wire import canonical


def copy_notices(payload):
    directory = payload / "licenses"
    directory.mkdir()
    components = []
    names = ["anyio", "certifi", "cffi", "cryptography", "dcap-qvl", "h11", "httpcore", "httpx",
             "idna", "pillow", "pqcrypto", "pycparser", "typing-extensions", "pyinstaller"]
    for name in names:
        package = distribution(name)
        target = directory / name
        target.mkdir()
        copied = []
        for entry in package.files or []:
            if any(word in Path(entry).name.lower() for word in ("license", "copying", "notice")):
                source = Path(package.locate_file(entry))
                filename = Path(entry).name
                require(not (target / filename).exists(), "duplicate dependency notice filename")
                shutil.copyfile(source, target / filename)
                copied.append(filename)
        if name == "dcap-qvl" and not copied:
            source = ROOT / "deploy/licenses/dcap-qvl-0.7.0.LICENSE"
            require(package.version == "0.7.0" and sha256(source.read_bytes()) ==
                    "f77e71ba8380d395e78485302cc4beccab400b9a01648e2acaf26085fe65bd7f",
                    "review the DCAP source notice when updating its wheel")
            shutil.copyfile(source, target / "LICENSE")
            copied.append("LICENSE")
        require(copied, "dependency has no packaged license notice: " + name)
        components.append({"name": name, "version": package.version, "notices": sorted(copied)})
    python_license = Path(sysconfig.get_path("stdlib")) / "LICENSE.txt"
    if not python_license.is_file() and sys.platform == "win32":
        python_license = Path(sys.base_prefix) / "LICENSE.txt"
    if not python_license.is_file() and sys.platform == "linux":
        # Debian/Ubuntu package the interpreter's full notice separately from
        # the standard library. Copy the installed runtime's own notice.
        python_license = Path(sys.base_prefix) / "share/doc" / f"python{sys.version_info.major}.{sys.version_info.minor}" / "copyright"
    require(python_license.is_file(), "Python runtime license is missing")
    shutil.copyfile(python_license, directory / "Python-LICENSE.txt")
    shutil.copyfile(ROOT / "LICENSE", directory / "Axiom-LICENSE.txt")
    (directory / "components.json").write_bytes(canonical({"python_version": platform.python_version(), "components": components}))


def build(args):
    require(sys.platform in {"darwin", "linux", "win32"}, "build on a supported native runner")
    if sys.platform == "win32":
        # The pinned verifier/KEM wheels target x64. Windows 11 on Arm runs
        # this separate process through its built-in x64 emulation.
        require(sysconfig.get_platform() == "win-amd64", "Windows helper requires an x64 Python interpreter")
        architecture = "x64"
    else:
        require(platform.machine() in {"arm64", "aarch64", "x86_64"}, "unsupported helper architecture")
        architecture = {"aarch64": "arm64", "x86_64": "x64"}.get(platform.machine(), platform.machine())
    authority = PublisherAuthority.load(args.authority)
    identifier(args.chute_id)
    require(type(args.maximum_charge_microusd) is int and 0 <= args.maximum_charge_microusd <= 9007199254740991,
            "valid native request ceiling required")
    output = args.output.absolute()
    require(not output.exists(), "bundle output already exists; select a new output directory")
    if args.codesign_identity:
        require(sys.platform == "darwin", "codesign identity only applies to macOS")
    windows_sign_script = getattr(args, "windows_sign_script", None)
    if windows_sign_script:
        require(sys.platform == "win32" and windows_sign_script.is_file(), "Windows signing requires an installed signing script")
    with tempfile.TemporaryDirectory(prefix="axiom-helper-build-") as temporary:
        temporary = Path(temporary)
        command = [sys.executable, "-m", "PyInstaller", "--noconfirm", "--clean", "--onedir", "--noupx",
            "--name", "axiom-chutes-helper", "--distpath", str(temporary / "dist"),
            "--workpath", str(temporary / "work"), "--specpath", str(temporary),
            "--paths", str(ROOT / "scripts"), "--paths", str(ROOT / "providers/chutes"),
            "--collect-all", "dcap_qvl", "--collect-all", "pqcrypto",
            "--hidden-import", "axiom_chutes.gpu", "--hidden-import", "axiom_chutes.relay",
            "--hidden-import", "axiom_chutes.publisher", "--hidden-import", "chutes_native_install",
            "--exclude-module", "inference_release", "--exclude-module", "chutes",
            "--exclude-module", "chutes_preview", "--exclude-module", "chutes_smoke",
            "--exclude-module", "pytest", "--exclude-module", "tkinter"]
        if args.codesign_identity:
            command += ["--codesign-identity", args.codesign_identity]
        command += [str(ROOT / "scripts/chutes_helper_entry.py")]
        subprocess.run(command, check=True, cwd=temporary)
        runtime = temporary / "dist/axiom-chutes-helper"
        if windows_sign_script:
            for binary in sorted(runtime.rglob("*")):
                if binary.is_file() and binary.suffix.lower() in {".exe", ".dll", ".pyd"}:
                    subprocess.run(["pwsh", "-NoProfile", "-NonInteractive", "-File",
                        str(windows_sign_script.absolute()), "-FilePath", str(binary)], check=True)
        executable_name = "axiom-chutes-helper.exe" if sys.platform == "win32" else "axiom-chutes-helper"
        executable = runtime / executable_name
        clean_environment = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin"}
        if sys.platform == "win32":
            clean_environment = {key: os.environ[key] for key in
                ("SystemRoot", "WINDIR", "USERPROFILE", "LOCALAPPDATA", "TEMP", "TMP") if key in os.environ}
            clean_environment["PATH"] = str(Path(os.environ["SystemRoot"]) / "System32")
        self_test = subprocess.run([str(executable), "--self-test"], cwd=temporary, check=True,
            env=clean_environment, capture_output=True, text=True, timeout=30)
        report = json.loads(self_test.stdout)
        require(report.get("passed") is True and report.get("frozen") is True, "standalone self test failed")
        payload = temporary / "payload"
        payload.mkdir()
        shutil.move(str(runtime), payload / "runtime")
        # Re-encode the validated public schema; never copy an operator's
        # arbitrary configuration directory or private provider credentials.
        (payload / "authority.json").write_bytes(canonical(json.loads(args.authority.read_bytes())))
        (payload / "release.json").write_bytes(canonical({"authority": "authority.json", "user_state": True}))
        (payload / "bridge.json").write_bytes(canonical({"hosted_relay": True, "chute_id": args.chute_id,
            "release_verification_config": "release.json"}))
        (payload / "native.json").write_bytes(canonical({"helper": "runtime/" + executable_name,
            "helper_sha256": sha256((payload / "runtime" / executable_name).read_bytes()),
            "config": "bridge.json", "release_authority_fingerprint": sha256(bytes.fromhex(authority.public_key)),
            "maximum_charge_microusd": args.maximum_charge_microusd}))
        copy_notices(payload)
        files = []
        for path in sorted(payload.rglob("*")):
            if path.is_symlink():
                target = os.readlink(path)
                require(not Path(target).is_absolute() and path.resolve().is_relative_to(payload.resolve()),
                        "runtime symlink escapes bundle")
                files.append({"path": path.relative_to(payload).as_posix(), "symlink": target})
            elif path.is_file():
                with path.open("rb") as source:
                    files.append({"path": path.relative_to(payload).as_posix(), "bytes": path.stat().st_size,
                                  "sha256": hashlib.file_digest(source, "sha256").hexdigest()})
        metadata = {"schema": "axiom-chutes-helper-bundle-v1", "platform": sys.platform,
            "architecture": architecture,
            "repository": authority.repository, "channel": authority.channel,
            "authority_fingerprint": sha256(bytes.fromhex(authority.public_key)),
            "self_test": report, "files": files}
        if sys.platform == "win32":
            metadata["windows_arm64_emulation"] = True
        (payload / "bundle.json").write_bytes(canonical(metadata))
        output.parent.mkdir(parents=True, exist_ok=True)
        shutil.move(str(payload), output)
    return {"output": str(output), "platform": metadata["platform"], "architecture": metadata["architecture"],
            "files": len(files), "bytes": sum(row.get("bytes", 0) for row in files), "self_test_passed": True,
            "signed_installer_qualified": False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--authority", type=Path, required=True)
    parser.add_argument("--chute-id", required=True)
    parser.add_argument("--maximum-charge-microusd", type=int, default=100000)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--codesign-identity")
    parser.add_argument("--windows-sign-script", type=Path)
    print(json.dumps(build(parser.parse_args())), flush=True)


if __name__ == "__main__":
    main()
