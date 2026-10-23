# 0017: Gift redemption is an account operation

Status: accepted, September 21, 2026.

TUI previously had no balance/funding controls. Desktop already exposed Zcash
deposits. Both now share the native billing client for gift redemption and retain
the backend as the sole authority for credit and exact-once redemption.

`billing@3` adds `_axiom/billing/redeem_gift_code`, with a bounded secret `code`
request and a response containing `creditedMicrousd`, `alreadyRedeemed` and the
updated `status`. Existing `billing@2` clients can still fetch status from a new
sidecar; the new Desktop requires version 3. Generate schema and TypeScript from
the Rust contract and ship Desktop and its sidecar together.

Native interactive credentials authorize the HTTPS account API. Codes must not
be inference prompts, activity events, transcript content or diagnostic output.
The request's Rust Debug implementation redacts the code. Desktop checks the
captured account across IPC; native credential leases and ACP account-work
registration cancel or discard work during a switch. A network error after commit
is recoverable by submitting the same code; clients do not infer a failed credit
from cancellation alone.

The TUI uses dedicated masked input under `/redeem`, separate from its composer.
`/balance` and `/topup` expose status, the deposit address, refresh and redemption.
Desktop adds a redemption form to Balance. New gift secrets have no local durable
storage. Platform owns issuance, hashes, USD denominations, immutable ledger
credits and operator revocation; see the
[Platform runbook](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/operations/gift-cards.md).
