"""Native-only Axiom transport. No Chutes provider credential is installed here."""
import re
import ssl
from urllib.parse import urlsplit

import httpx
import certifi

from .client import ChutesClient, read_limited
from .wire import decode_json


class RequestAdmissionRejected(RuntimeError):
    """Reservation rejected before inference dispatch; contains no provider text."""


class InsufficientCredit(RequestAdmissionRejected):
    """The account needs more available credit before inference can start."""


class UnresolvedRequests(RequestAdmissionRejected):
    """Earlier requests must settle or reach their bounded hold deadline."""


class RelayChutesClient(ChutesClient):
    def __init__(self, origin, access_token, *, ca_file=None):
        parsed = urlsplit(origin)
        if (parsed.scheme != "https" or not parsed.hostname or parsed.path
                or parsed.query or parsed.fragment or parsed.username or parsed.password
                or not isinstance(access_token, str) or not access_token.startswith("axa_")
                or any(char in access_token for char in "\r\n")):
            raise ValueError("native authenticated HTTPS relay required")
        if ca_file and parsed.hostname != "127.0.0.1":
            raise ValueError("local staging CA is restricted to loopback")
        self.origin, self.leases = origin, {}
        http = httpx.AsyncClient(trust_env=False, follow_redirects=False,
            verify=ssl.create_default_context(cafile=ca_file or certifi.where()),
            timeout=httpx.Timeout(660, connect=10),
            event_hooks={"request": [self._lease], "response": [self._admission]})
        super().__init__(access_token, http=http)
        self.invocation_url = origin + "/api/v1/chutes/invoke"

    async def _admission(self, response):
        if response.status_code == 402:
            await response.aclose()
            raise InsufficientCredit("Insufficient balance. Top up your Axiom balance to continue.")

    async def _lease(self, request):
        nonce = request.headers.get("x-e2e-nonce")
        if nonce:
            lease = self.leases.pop(nonce, None)
            if lease is None:
                raise ValueError("invocation nonce was not issued to this native session")
            request.headers["X-Axiom-Lease"] = lease

    async def api(self, method, path, *, json=None, params=None):
        async with self._http.stream(method, self.origin + "/api/v1/chutes" + path,
                                    headers=self._headers, json=json, params=params) as response:
            await self._admission(response)
            if method == "POST" and path == "/reservations" and response.status_code == 409:
                body = decode_json(await read_limited(response, 4096), 4096)
                if body == {"detail": {"code": "unresolved_requests"}}:
                    raise UnresolvedRequests("unresolved request admission limit")
                raise RequestAdmissionRejected("request capacity or price ceiling unavailable")
            if response.status_code != 200:
                raise RuntimeError("authenticated Chutes relay rejected metadata")
            return decode_json(await read_limited(response))

    async def _json(self, method, path, **kwargs):
        if method != "GET":
            raise ValueError("unsupported metadata operation")
        if re.fullmatch(r"/e2e/instances/[a-f0-9-]{36}", path):
            value = await self.api("GET", "/instances")
            if value.get("chute_id", path.rsplit("/", 1)[-1]) != path.rsplit("/", 1)[-1]:
                raise ValueError("signed deployment routing changed before discovery")
            for row in value["instances"]:
                self.leases.update(row.pop("leases"))
            if len(self.leases) > 512:
                self.leases = dict(list(self.leases.items())[-256:])
            return value
        if re.fullmatch(r"/instances/[a-f0-9-]{36}/evidence", path):
            return await self.api("GET", path, params=kwargs.get("params"))
        raise ValueError("metadata route is not allowlisted")
