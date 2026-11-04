"""Coalesce public AMD endorsement downloads inside the guest's private RAM.

No report, nonce, account, request, prompt, output or private key is cached.
Consumers must independently authenticate every supplied manufacturer object.
"""
import asyncio
import base64
import fcntl
import os
from pathlib import Path
import stat
import time

import httpx

from .snp import decode_collateral, fetch_collateral, parse_report
from .strict import canonical, decode_json, require, sha256


def shared_collateral(report, directory=Path('/run/axiom-state')):
    info=parse_report(report)
    key=sha256(canonical({'chip':info['chip_id'].hex(),'tcb':info['tcb']}))
    metadata=directory.lstat()
    require(stat.S_ISDIR(metadata.st_mode) and metadata.st_uid==os.getuid()
        and not metadata.st_mode&0o077,'private RAM collateral directory required')
    fd=os.open(directory/'amd-collateral.lock',os.O_CREAT|os.O_RDWR|os.O_NOFOLLOW,0o600)
    with os.fdopen(fd,'a') as lock:
        fcntl.flock(lock,fcntl.LOCK_EX)
        cache=directory/'amd-collateral.json'
        now=int(time.monotonic())
        if cache.exists():
            metadata=cache.lstat()
            require(stat.S_ISREG(metadata.st_mode) and metadata.st_uid==os.getuid()
                and not metadata.st_mode&0o077 and metadata.st_size<=2*1024*1024,
                'bounded ordinary collateral cache required')
            previous=decode_json(cache.read_bytes(),2*1024*1024)
            require(set(previous)=={'key','at','collateral'} and type(previous['at']) is int,
                'strict collateral cache required')
            if previous['key']==key and 0<=now-previous['at']<60:
                decode_collateral(previous['collateral'])
                return previous['collateral']
        async def download():
            async with httpx.AsyncClient(trust_env=False,follow_redirects=False,timeout=8) as http:
                return await fetch_collateral(info,http)
        values=asyncio.run(download())
        result={name:base64.b64encode(raw).decode() for name,raw in zip(('vcek','chain','crl'),values)}
        decode_collateral(result)
        temporary=directory/'amd-collateral.new'
        temporary.unlink(missing_ok=True)
        fd=os.open(temporary,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
        with os.fdopen(fd,'wb') as output:
            output.write(canonical({'key':key,'at':now,'collateral':result}))
        temporary.replace(cache)
        return result
