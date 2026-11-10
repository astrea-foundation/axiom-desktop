"""Authenticate every model into CPU-private RAM before making it read-only."""

import asyncio
import hashlib
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess

import httpx

from .strict import canonical, decode_json, hex_bytes, integer, require, sha256


def inventory(lock):
    require(isinstance(lock,dict) and set(lock)=={"schema_version","files"} and lock["schema_version"]==1,
            "model lock schema")
    require(isinstance(lock["files"],list) and 1<=len(lock["files"])<=4096,"model file count")
    files, names = [], set()
    for item in lock["files"]:
        require(isinstance(item,dict),"invalid model record")
        path = item.get("path")
        require(isinstance(path,str) and len(path)<=512,"model path required")
        relative = PurePosixPath(path)
        require(relative.as_posix()==path and not relative.is_absolute() and ".." not in relative.parts
            and path.startswith(("llm/","image/")) and path not in names,"canonical unique model path required")
        names.add(path)
        integer(item["bytes"],1,200*1024**3)
        hex_bytes(item["sha256"],32)
        if "bundled_source" in item:
            require(set(item)=={"path","bytes","sha256","bundled_source"}
                and item["bundled_source"]=="models/loras/uncensored.safetensors","unapproved bundled model")
        else:
            require(set(item)=={"path","bytes","sha256","repository","revision","filename"},"invalid download record")
            import re
            require(isinstance(item["repository"],str) and re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+",item["repository"])
                and isinstance(item["revision"],str) and re.fullmatch(r"[0-9a-f]{40}",item["revision"]),"pinned model repository required")
            filename = PurePosixPath(item["filename"])
            require(filename.as_posix()==item["filename"] and not filename.is_absolute() and ".." not in filename.parts,
                "canonical model filename required")
        files.append({k:item[k] for k in ("path","bytes","sha256")})
    return canonical({"schema_version":1,"files":sorted(files,key=lambda f:f["path"])})


async def load(lock_path, manifest_path, root=Path('/models'), bundled=Path('/opt/axiom/bundled')):
    lock=decode_json(Path(lock_path).read_bytes(),1024*1024)
    raw=inventory(lock)
    require(raw==Path(manifest_path).read_bytes(),"model inventory differs from immutable release")
    require(root.is_dir() and not root.is_symlink() and not any(root.iterdir()),"fresh private model mount required")
    total=sum(item['bytes'] for item in lock['files'])
    require(shutil.disk_usage(root).free > total + 1024**3,"insufficient private model RAM")
    semaphore=asyncio.Semaphore(3)
    async def download(item):
        async with semaphore:
            destination=root/item['path']
            destination.parent.mkdir(parents=True,exist_ok=True)
            digest,total=hashlib.sha256(),0
            with destination.open('xb') as output:
                if 'bundled_source' in item:
                    source=bundled/'uncensored.safetensors'
                    require(not source.is_symlink(),"bundled model symlink")
                    with source.open('rb') as incoming:
                        while chunk:=incoming.read(4*1024*1024):
                            total+=len(chunk); digest.update(chunk); output.write(chunk)
                else:
                    url=f"https://huggingface.co/{item['repository']}/resolve/{item['revision']}/{item['filename']}"
                    async with httpx.AsyncClient(trust_env=False,follow_redirects=True,timeout=120,max_redirects=5) as http:
                        async with http.stream('GET',url) as response:
                            require(response.status_code==200,"model download unavailable")
                            async for chunk in response.aiter_bytes(4*1024*1024):
                                total+=len(chunk)
                                require(total<=item['bytes'],"model exceeds reviewed size")
                                digest.update(chunk); output.write(chunk)
                require(total==item['bytes'] and digest.hexdigest()==item['sha256'],"model hash/size differs")
            destination.chmod(0o444)
    try:
        await asyncio.gather(*(download(item) for item in lock['files']))
    except BaseException:
        # This guest never becomes eligible after a partial/failed load.
        raise
    subprocess.run(['mount','-o','remount,ro,nosuid,nodev,noexec',str(root)],check=True)
    return {'files':len(lock['files']),'bytes':total,'model_manifest':sha256(raw)}
