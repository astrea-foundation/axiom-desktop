"""Measured read-only guest supervisor and TLS ciphertext relay."""

import asyncio
from datetime import datetime, timedelta, timezone
import ipaddress
import os
from pathlib import Path
import secrets
import signal
import ssl
import subprocess
from urllib.parse import urlsplit

from aiohttp import web
import httpx
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import NameOID

from .cli import hardware_check, read
from .models import load
from .release import MANIFEST_DOMAIN, verify_release, capabilities
from .strict import canonical, require, sha256, signed_payload


def tls_context(origin, state):
    key = ec.generate_private_key(ec.SECP384R1())
    host = urlsplit(origin).hostname
    try: san = x509.IPAddress(ipaddress.ip_address(host))
    except ValueError: san = x509.DNSName(host)
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, host)])
    now = datetime.now(timezone.utc)
    certificate = (x509.CertificateBuilder().subject_name(name).issuer_name(name).public_key(key.public_key())
        .serial_number(x509.random_serial_number()).not_valid_before(now-timedelta(minutes=1))
        # RFC 5280 section 4.1.2.5: no well-defined expiration date. This leaf
        # is trusted only after fresh CPU binding and changes on every boot.
        .not_valid_after(datetime(9999,12,31,23,59,59,tzinfo=timezone.utc))
        .add_extension(x509.SubjectAlternativeName([san]),critical=False)
        .add_extension(x509.BasicConstraints(ca=False,path_length=None),critical=True)
        .sign(key,hashes.SHA384()))
    private = state/'tls-private.pem'
    private.write_bytes(key.private_bytes(serialization.Encoding.PEM,serialization.PrivateFormat.PKCS8,serialization.NoEncryption()))
    private.chmod(0o600)
    public = state/'tls-certificate.pem'
    public.write_bytes(certificate.public_bytes(serialization.Encoding.PEM))
    (state/'tls-certificate.der').write_bytes(certificate.public_bytes(serialization.Encoding.DER))
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(public,private)
    return context


async def relay(request):
    # The TLS certificate is itself CPU-bound. Public evidence is fetched
    # before trusting this certificate; inference uses the attested pin.
    allowed = {'/v1/attestation','/v1/admission','/v1/exchange','/v1/control','/healthz','/bootstrap/healthz','/livez'}
    if request.path not in allowed or request.query_string:
        raise web.HTTPNotFound()
    if request.method not in ('GET','POST'):
        raise web.HTTPMethodNotAllowed(request.method,['GET','POST'])
    body = await request.read()
    http = request.app['http']
    async with http.stream(request.method,'http://127.0.0.1:8005'+request.path,content=body,
            headers={'content-type':'application/json'}) as upstream:
        headers = {'cache-control':'no-store'}
        for name in ('content-type','axiom-pool-response-nonce'):
            if name in upstream.headers: headers[name]=upstream.headers[name]
        response = web.StreamResponse(status=upstream.status_code,headers=headers)
        await response.prepare(request)
        total=0
        async for chunk in upstream.aiter_bytes():
            total+=len(chunk)
            require(total<=128*1024*1024,'ciphertext response limit')
            await response.write(chunk)
        await response.write_eof()
        return response


@web.middleware
async def bounded_requests(request,handler):
    # Keep evidence/admission/liveness responsive while inference is queued.
    slots=request.app['exchange_slots' if request.path=='/v1/exchange' else 'control_slots']
    if slots.locked(): raise web.HTTPTooManyRequests()
    async with slots:
        return await handler(request)


async def ready(children, endpoint, seconds):
    until=asyncio.get_running_loop().time()+seconds
    async with httpx.AsyncClient(trust_env=False,follow_redirects=False,timeout=3) as http:
        while asyncio.get_running_loop().time()<until:
            require(all(c.poll() is None for c in children),'private workload process failed')
            try:
                if (await http.get(endpoint)).status_code==200: return
            except httpx.HTTPError: pass
            await asyncio.sleep(1)
    raise TimeoutError('private workload readiness deadline')


async def supervise(children, http, *, interval=5, failures=3):
    """Never reuse a quarantined worker or restart one engine in isolation."""
    failed=0
    endpoints=('http://127.0.0.1:10101/health',
        'http://127.0.0.1:8080/axiom/v1/health','http://127.0.0.1:8005/livez')
    while True:
        require(all(child.poll() is None for child in children),'private process exited')
        try:
            checks=await asyncio.gather(*(http.get(endpoint,timeout=5) for endpoint in endpoints))
            okay=all(response.status_code==200 for response in checks)
        except httpx.HTTPError:
            okay=False
        failed=0 if okay else failed+1
        require(failed<failures,'private workload failed liveness')
        await asyncio.sleep(interval)


async def run():
    require(os.geteuid()==0,'measured boot supervisor requires root')
    # No mutable root or provider cloud-init path can activate this runtime.
    mounts=Path('/proc/self/mountinfo').read_text().splitlines()
    require(any(line.split()[4]=='/' and 'ro' in line.split()[5].split(',') and ' - squashfs ' in line
        for line in mounts),'measured read-only root required')
    require(len(Path('/proc/swaps').read_text().splitlines())==1,'private guest memory must not use swap')
    state=Path('/run/axiom-state'); state.mkdir(mode=0o700)
    cfg=read('/run/axiom-release/worker.json')
    manifest_document=read(cfg['workload_manifest_file'])
    public=cfg['verifier']['publisher_key']
    authority=read('/opt/axiom/authority.json')
    require(public==authority['publisher_key'] and cfg['issuer_key']==authority['issuer_key'],
        'immutable publisher/issuer authority mismatch')
    manifest,raw=signed_payload(manifest_document,public,MANIFEST_DOMAIN)
    policy_document=read(cfg['policy_file'])
    declaration={'deployment_digest':sha256(raw),'worker_id':cfg['worker_id'],'logical_origin':cfg['public_origin'],
        'model_manifest':sha256(Path(cfg['model_manifest_file']).read_bytes())}
    require(declaration['deployment_digest']==cfg['deployment_digest'],'runtime release differs')
    manifest,policy,accepted=verify_release(manifest_document,policy_document,public,declaration,
        int(datetime.now(timezone.utc).timestamp()),cfg['verifier']['minimum_policy_sequence'])
    require(capabilities(manifest)==read('/opt/axiom/capabilities.json'),
        'signed capabilities differ from the immutable worker')
    print('Axiom boot: protected root and signed release policy verified.',flush=True)
    engine_bytes=Path('/opt/axiom/engines.json').read_bytes()
    require(sha256(engine_bytes)==manifest['engine_config_sha256'],'engine configuration differs from signed release')
    for attempt in range(180):
        driver=subprocess.run(['nvidia-smi','-L'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        if driver.returncode==0: break
        await asyncio.sleep(1)
    else: raise RuntimeError('confidential GPU driver unavailable')
    await hardware_check({'amd':policy['amd'],'nvidia_anchors':policy['nvidia_anchors']},accepted)
    print('Axiom boot: approved launch and confidential GPU verified.',flush=True)
    subprocess.run(['nvidia-smi','conf-compute','-srs','1'],check=True,stdout=subprocess.DEVNULL)
    await load('/opt/axiom/models.lock.json',cfg['model_manifest_file'])
    print('Axiom boot: model inventory verified and made read-only.',flush=True)
    context=tls_context(cfg['public_origin'],state)
    engines=read('/opt/axiom/engines.json')
    env={**os.environ,**engines['environment'],'AXIOM_IMAGE_ENGINE_TOKEN':secrets.token_hex(32)}
    env.pop('VLLM_API_KEY',None); env.pop('VLLM_MARLIN_INPUT_DTYPE',None)
    children=[]
    runner=None
    def start(args):
        child=subprocess.Popen(args,env=env,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,close_fds=True,start_new_session=True)
        children.append(child)
    try:
        start(engines['llm'])
        await ready(children,'http://127.0.0.1:10101/health',1800)
        print('Axiom boot: private text engine ready.',flush=True)
        start(engines['image'])
        await ready(children,'http://127.0.0.1:8080/axiom/v1/health',900)
        print('Axiom boot: private image engine ready.',flush=True)
        start(['/usr/local/bin/axiom-worker','/run/axiom-release/worker.json','--direct'])
        await ready(children,'http://127.0.0.1:8005/bootstrap/healthz',180)
        http=httpx.AsyncClient(trust_env=False,follow_redirects=False,timeout=610)
        app=web.Application(client_max_size=64*1024*1024,middlewares=[bounded_requests])
        app['http']=http
        app['exchange_slots']=asyncio.Semaphore(16)
        app['control_slots']=asyncio.Semaphore(4)
        app.router.add_route('*','/{path:.*}',relay)
        runner=web.AppRunner(app,access_log=None)
        await runner.setup()
        await web.TCPSite(runner,'0.0.0.0',urlsplit(cfg['public_origin']).port or 443,ssl_context=context).start()
        print('Axiom boot: public evidence endpoint ready; operator admission required.',flush=True)
        await supervise(children,http)
    finally:
        if runner: await runner.cleanup()
        for child in reversed(children):
            if child.poll() is None: os.killpg(child.pid,signal.SIGTERM)
        for child in children:
            try: child.wait(timeout=10)
            except subprocess.TimeoutExpired: os.killpg(child.pid,signal.SIGKILL); child.wait()


def main():
    try: asyncio.run(run())
    except BaseException as error:
        print('direct workload unavailable: '+type(error).__name__,flush=True)
        # No interactive rescue shell and no frontend-only key reuse.
        subprocess.run(['systemctl','poweroff','--no-block'],check=False)
        raise SystemExit(1)


if __name__=='__main__': main()
