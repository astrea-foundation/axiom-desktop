# Release acceptance

The 0.1.21 installed-image proof explicitly selected AstreaAI/Qwen and did not
establish the normal fresh-profile default or NEAR catalog availability. During
production catalog discovery failures, NEAR offerings can disappear and the
runtime's existing preference order selects Tinfoil DeepSeek. Future signed
qualification must require the exact abliterated NEAR offering and Uncensored
tag, verify the actual Desktop/CLI default without a model environment override,
then separately select the image-test model. Until this live gate passes, do not
claim restored NEAR availability from the 0.1.21 image receipts.

The 0.1.21 release audit updates rusqlite to 0.41.1 with libsqlite3-sys 0.39.1
(SQLite 3.54.1), addressing RUSTSEC-2026-0459 in the bundled database engine.
The MCP SDK moves to rmcp 3.9.0, covering RUSTSEC-2026-0457, RUSTSEC-2026-0461
and RUSTSEC-2026-0462. The dependency audit remains required without new advisory
exceptions. Existing conversation storage and MCP behavior must pass native
regression tests with these versions.
The workspace minimum Rust version is 1.89 for the patched database crate;
the pinned CI and release toolchain remains Rust 1.97.1.

The launch dependency audit updates Tokio to 1.53.3, the rustls verifier to
0.103.16, both webpki root lists to 1.0.10, libz-sys to 1.1.31 and
pulldown-cmark to 0.13.6. These retain the existing security audit gate.

Native pool ES256 proof-key generation/signing uses P-256 0.14 and
crypto-bigint 0.7.10, beyond the fix for
[RUSTSEC-2026-0387](https://rustsec.org/advisories/RUSTSEC-2026-0387.html).
Pool HPKE explicitly enables only X25519. The remaining crypto-bigint 0.5.5
dependency is confined to public signature/certificate verification: NEAR's
P-384 attestation verifier, Tinfoil's P-256 Fulcio/Sigstore verifiers and P-384
SEV verifier. The pinned Tinfoil revision is
`91e8aef8fbc34129b68de8667ece5bd9ef7b7110`; its signing keys occur only in
tests, and its EHBP revision `93cc1fa1ad61e19f9a34fb23cb5b5d5635d3edb0`
selects `X25519HkdfSha256` explicitly. Public signature operands contain no
private scalar whose timing could leak. A scoped audit exception covers only
that residual version. Remove it when these verifiers update their curve
dependencies; any new private-key use requires a fresh review and a patched
implementation. This exception changes no evidence or response verification.

Qualify the exact promoted source revision and signed installer bytes. Record
OS, architecture, browser, source revision, artifact SHA-256, signing identity,
result and limitations in the release's maintainer acceptance record. Unit tests
and browser fixtures do not establish signed native OS acceptance.

macOS native-session regressions cover a single password read per restore,
continuous CLI/Desktop rotation locking, denied reads without repeated attempts,
and in-place writes in an isolated Keychain fixture. Signed macOS acceptance must
still verify that **Always Allow** survives quit/relaunch and shared CLI/Desktop
use. Ad hoc development binaries do not establish approval persistence for the
certificate-signed installer.

Browser payment quote fixtures derive issuance and expiry from one timestamp so
the exact 60-second validity contract remains deterministic at clock boundaries.
The expiry assertion still advances past that interval and requires the display
to stop showing the quote.

## AstreaAI rollout gates

The [generation-9 / permanent-policy-14 qualification](../../axiom-inference/docs/qa/direct-verda-reference-images-2026-11-03.json)
establishes fresh CPU/GPU/workload/key/TLS verification, authenticated text and
vision, five-reference image generation/editing and the 1536-square default in
the operator's macOS development connection. Its fresh QA account and separate
profile do not establish a hosted customer rollout.

The [generation-12 / permanent-policy-18 qualification](../../axiom-inference/docs/qa/direct-verda-capacity-recovery-2026-11-04.json)
adds four independent completed clients, an independent 14-request burst with
eight authenticated completions and six authenticated capacity refusals, active
disconnect isolation, five-reference 1536-square image regression, and a real
VM power-off recovered by the always-on LAN watchdog. A fresh boot with new
worker keys passed full verification and encrypted completion. These results
qualify the private operator path rather than production account enrollment.

The [generation-13 / permanent-policy-19 account qualification](../../axiom-inference/docs/qa/direct-astreaai-account-staging-2026-11-04.json)
adds service-owned issuer keys, real staging account grants and ledger charges,
idempotent receipt settlement, account-scoped usage and an insufficient-credit
refusal before input dispatch. Independently verified chat, vision, tools and
five-reference 1536-square images completed, including two concurrent requests
and a fresh verified boot after watchdog recovery. A named hosted staging
profile also loaded its own Keychain credential after the short read deadline
was removed. That record predates the browser startup repair and production
promotion; it does not itself establish either result.

The [production qualification](../../axiom-inference/docs/qa/direct-astreaai-production-2026-11-05.json)
records generation 13 / permanent policy 20, the production account service and
the independently verified generation-3 browser gateway. Public worker,
controller, issuer and immutable price pins are distributed to clients; the
issuer private key stays on the account-service host. Account-owned grants bind
the session, client proof key, approved worker and request. Native chat now keeps
the same request identity through reservation, encrypted execution and receipt
reconciliation.

Two independent production accounts completed encrypted requests concurrently.
Foreign status and receipt access failed, another account received no usage
records, duplicate settlement added no debit, and an account without credit was
refused before input dispatch. Twelve metadata-only reservations were admitted;
the next returned HTTP 429 with Retry-After. All twelve were cancelled without
dispatch or balance changes. The final API startup guard was deployed after a
fenced zero-request drain and checks worker context/output caps before enrollment.

The production browser completed five-reference 1536-square generation, optional
image viewing, saving and regeneration with the original prompt, dimensions and
reference order. Authenticated image usage included normalized input pixels,
2,359,296 output pixels and the immutable $0.008 request price. Only newly created
synthetic accounts and profiles were used; existing user chats were not read.

The enabled watchdog on `salt@10.0.0.191` now verifies the production origin and
policy 20. Its rental API credential remains outside the GPU VM. Earlier actual
power-off recovery produced a new verified boot and authenticated completion.
The production backend also has a persistent container restart policy. The
single shared GPU was drained out of staging before production enrollment.

Desktop 0.1.17 at `7c41a12b31d42aefe4f21896864a166f0d087b3a` passed exact-source
Rust, UI and all four native CI jobs. Its release gate completed three production
Observe investigations with the frozen verifier and one tool call each. The
recorded times were 34,147, 11,319 and 8,705 ms. The 0.1.16 candidate was never
published; 0.1.17 fixes its workspace-relative verifier path. Assembly-only Mac
recovery preserves the same qualified native source and receipts; see the
[release procedure](releasing.md#completing-unpublished-packaging-from-retained-inputs).
Signed Desktop and CLI 0.1.17 are now published in the public update feed with
all eight installer records verified. Both Mac PKGs passed Developer ID signing,
Apple notarization, stapling and installer assessment. Installed Desktop launch
passed on Intel and Apple Silicon; the retained signed Windows packages passed
x64/ARM64 installation acceptance. The actual signed Mac package completed an
encrypted production Observe request with one `read_file` call in 10,867 ms,
using its bundled verifier and automatically discovered public configuration.
Its isolated test fixture remained unchanged. The publisher verified every
installer on both mirrors before activating the website and channel.

Desktop and CLI 0.1.18 restore additive NEAR AI, Tinfoil and AstreaAI discovery.
The public signed manifest identifies source
`5d520d672b2303f15fbd14ee0862c28ad5af9154`; exact-source Rust, UI and all four
native checks passed. The normal release passed its live evaluation, retained
native-build receipts, Windows ARM64 installer acceptance and Mac signing and
notarization. Installed Mac launch passed on Intel and Apple Silicon. The Mac
signing job alone was retried after a notarization-ticket network failure;
successful native and Windows inputs were retained.

All eight public installer entries and lengths were checked. The actual
downloaded Mac package passed its SHA-256, Developer ID installer signature,
stapled notarization ticket, bundled CLI and verifier signatures, and automatic
installed configuration discovery. Its bundled client found all twelve models
and completed encrypted synthetic replies with NEAR, Tinfoil and AstreaAI in
11,543, 4,645 and 7,422 ms respectively. No external operator configuration or
existing user profile was used. Browser gateway generation six separately passed
fresh independent attestation and authenticated replies from all three providers.
The provider completion-notification fix is included in the gateway's shared
client pin; the immutable 0.1.18 Desktop tag remains unchanged.

The full retained-input Mac reproduction also verified the original native
receipts and reproduced Electron's nested Python-framework plist rewrite. The
final assembly adds complete frozen runtimes after the universal merge, preserving
their exact original inventories and signatures before app signing. Application
and native source remain at the same qualified revision.

Retain these limits and remaining interactive requirements:

- Four short executions share 65,536 reserved tokens, with at most eight waiters
  for 30 seconds and one image execution. Long 32K requests can reduce concurrency
  to two. The bounded account tests do not establish sustained customer throughput.
- A single GPU cannot serve during a full VM recovery. Warm redundant capacity
  and actual failover qualification remain necessary for continuous availability.
- Signed native sign-in, account switching, all image/reference interactions,
  data-preserving upgrades and failure recovery on every supported OS remain
  subject to the interactive acceptance matrix below. CI, packaging and a
  successful CLI completion cover their stated scope only.
- Keep cancellation, recipient/request binding, stream ordering/completion,
  fresh evidence and receipt checks strict during account changes and replacement.
  Permanent workload policy does not waive fresh hardware or response checks.

See [the Direct connection guide](direct-verda.md) and [release procedure](releasing.md).

## Confidential pool qualification

The `axiom-pool` provider admits only installed `tinfoil-gpu-v3` offerings with
an installed verifier. Before enabling production custom pool inference or
claiming exact application/model execution, verify the exact application/model
release and Rust boot-key endorsement against accepted runtime measurements;
implement and qualify native/controller verification and instance routing;
exercise actual two-account concurrent traffic, replacement, cancellation,
stream truncation/reordering and authenticated completion on the admitted GPU.
The local native test-account flow has been exercised; the hosted local auth
frontend and production account flow still need an interactive qualification pass.
Hardware diagnostics and deterministic encrypted fixtures do not satisfy these
requirements. See [the pool guide](inference-pool.md).

## Automated checks

Run the [development checks](development.md#checks), including Rust formatting,
strict Clippy, functional tests, dependency policy, generated ACP contract checks,
TypeScript checks and Desktop unit/browser tests. Validate installer changes
through the explicit [unsigned candidate workflow](releasing.md#development-previews)
before production tags; retain its source SHA and candidate receipt. Use the
[release procedure](releasing.md#ci-baseline) to reuse successful CI for the exact
candidate instead of repeating unrelated jobs. The live release gate uses the
registered provider's attested E2EE protocol; it does not replace interactive QA.

The Desktop unit suite also builds the production Electron main entry and
rejects runtime imports of workspace TypeScript packages. Native installer
qualification must launch the installed app and reach its first window; a
successful renderer build or source-mode launch does not establish that the
packaged main process can start.

## Interactive qualification

Image tools use only the installed AstreaAI image adapter. Its runtime fixes
the 1:1 default, the supported ratios and the eight-step schedule, and the chat
model's catalog `uncensored` tag selects the pinned image LoRA. Regeneration
preserves the original model's policy after a model switch. Native checks cover
reference order, five-reference to text-only transitions, duplicate and foreign
IDs, bounded conditioning copies, output dimensions and optional viewing.
Broader co-resident load, encrypted multi-account qualification and signed
installer acceptance remain release requirements. AstreaAI image
qualification records are in [the Direct connection guide](direct-verda.md).

The unified `imageGeneration@2` tool and automatic vision-model gating replace
the earlier Images toggle. Native regression checks cover generation/edit
dispatch, conversation reference isolation, model switching, unavailable tools
and nonvision upload rejection. Renderer checks cover toggle removal and the
generated-image viewer. Installed app checks, cross-platform release validation
and broader load tests remain release requirements; source-mode qualification
does not replace them.

Generated-image presentation checks also cover one-time binary IPC payloads,
stable Blob URLs through streaming updates, account/runtime resource revocation,
late bootstrap recovery, 1536-pixel viewing while typing, and cancellable inline
decoding that yields to UI tasks. These local transport checks do not change
provider attestation, authenticated completion, original image bytes or size caps.

Chat recovery regressions cover missing attachment payloads, damaged image
metadata, unavailable historical image views, interrupted tool replies, bounded
image-reference expiration, incompatible historical files after a model switch,
and failed automatic summarization before a new prompt. Client completion checks
also delay an older running snapshot across send, edit and image regeneration,
then require a fresh ready snapshot. These checks preserve normal provider
verification and never replay an interrupted image job or local tool action.

Before releasing cross-provider AstreaAI image support, qualify verified
1536-square generation and ordered five-reference editing from vision-capable
Tinfoil and NEAR chats. Check that image provenance and regeneration retain the
originating chat model while image evidence and settled usage identify AstreaAI.
Hosted browser support also requires a newly measured gateway whose shared
client pin includes the same routing fix; updating the browser alone does not
update the image-provider implementation inside the gateway.
The image worker also needs a newly measured build with the Comfy parameter
restoration fix. Qualify alternating uncensored/base requests in one long-lived
engine, including 1536-square output and five ordered references; a cold-start
image alone does not exercise adapter restoration. The isolated GPU result in
the [local image qualification](../../axiom-inference/docs/qa/cross-provider-images-local-2026-11-05.json)
does not establish production attestation or cross-provider image acceptance.
Exercise both progress-reporting and progress-free native image APIs; completion
must not wait for a progress consumer that the caller never requested.

Image-tool registration must follow the secure provider's loaded local image
configuration, including automatic installed-bundle discovery. Qualify an actual
model-generated `generate` call from the signed installation without
`AXIOM_DIRECT_CLIENT_CONFIG_FILE` set. Include the standalone CLI layout and a
Desktop layout. Catalog discovery and encrypted chat alone do not prove the image
tools are registered. Keep a candidate unpublished if this check fails.
Also initialize the installed ACP runtime without that override and require
its advertised image-generation capability to match the registered tool.
Verify the standalone Mac package after extraction or installation, including
relative Python-framework links, bundled verifier signatures and a fresh worker
proof. Build-directory paths must not survive in installed runtime symlinks.
Exercise the standalone CLI through its installed command symlink as well as
the physical executable; both must discover the same pinned local configuration.

Optional `view_image` inspection has native regression coverage for uploaded,
generated and edited images, generation followed by viewing before the final
answer, tool-batch ordering, persisted restoration, malformed/foreign references,
decode failures, cancellation, model/service gating and retaining more than ten
images in the conversation. Vision-copy checks include transparent images and generated PNGs larger
than the upload byte limit. The October 27 live record predates this viewing
tool; live model selection of the new tool remains a separate qualification.

Synthetic native context checks cover model-specific image allowances after
restore and in the token guard, new views added to the last reported context count,
new uploads that exceed AstreaAI's eight-image protocol cap without triggering
compaction, view refusals at that cap, model switches from larger image histories,
retaining encoded media below the token threshold regardless of payload size,
provider report precedence over image estimates including reload, attachment
order and Unicode text across bounded summary stages,
encoded attachment and message bounds including summary overhead, and reasoning
output space before the visible summary. Existing failure/cancellation checks
retain original context until all summary stages succeed. These fixtures do not
establish live-provider or packaged Desktop acceptance of the context-accounting
changes.

Compaction progress checks cover cumulative streamed summary/reasoning estimates,
replacement with provider token counts, pass totals, spinner rendering and cleanup
on completion, failure and cancellation. An unresponsive provider cannot prevent
local cancellation. Store regressions cover repeated observer opens during live
turns and manual compaction, conflicting execution claims, and recovery only after
a subprocess owner dies. Usage-meter checks scramble random request IDs and late
deliveries to verify selection by request timestamps.

Image-progress checks cover authenticated request/sequence binding, rounded
sampling percentages, truncation and forged-update rejection, queue cancellation,
clean transport completion, native propagation and durable tool metadata. Browser
checks cover percentage bars, the 99%-to-100% completion boundary, and distinct
random filenames for generated images and repeated Save actions. The matching
worker/backend rollout and actual GPU preview-progress behavior still
require live qualification; local encrypted fixtures do not establish it.

| Area | Required evidence |
|---|---|
| Installation | Fresh install and uninstall on Windows x64/ARM64, macOS Intel/Apple Silicon, and each supported Linux package; expected publisher, notarization and architecture |
| Native authentication | System-browser sign-in using each supported method, callback/poll completion, cancellation and expiry, vault storage, restart, refresh/logout and account switching |
| Provider security | Current catalog, fresh local attestation, authenticated encrypted completion, stream cancellation/truncation, proof expiry, and explicit degraded-TEE consent/reset |
| Conversation state | Draft/queue/history retention, attachments, edit/regenerate, compaction, interrupted-turn recovery, restart and account isolation |
| Tools | Approval, denial, cancellation and workspace boundaries on each supported OS; Web consent and search's distinct privacy boundary |
| Account credit | Address provisioning, deposit progress and USD valuation, posted usage, gift-code masking/redemption, trial-capacity retry/paid continuation, six-hour invoice timers, five-active-invoice enforcement, server cancellation and account-switch cleanup; coordinate real settlement/reorg checks with platform maintainers |
| Automatic updates | Signed A-to-B upgrades from idle and busy Desktop/TUI; same-terminal workspace/session restoration, GUI reopen, matching CLI version and preserved drafts/history |
| Update failure paths | Denied OS authorization, cancellation, locked-process waits, interruption/repair, corrupt or wrong-target artifacts, rollback rejection and duplicate updater attempts; staging cleanup, including Windows helpers still in use and abandoned downloads |
| Publication | Matching source tag, signed inventory, complete installer matrix, verified mirrors, feed activation and safe publication retries |
| Branding and notices | Confirm the copyright/publisher line, artwork/font redistribution permissions, third-party notices and packaged icons |

An update must reset runtime-only outdated-TEE consent and preserve account data.
Do not downgrade a database into an incompatible client. Test real OS dialogs,
vaults and signed installers rather than treating mocks as acceptance evidence.

Backend rollback, payment-receiver recovery and secret management are separate
platform operations. Coordinate their acceptance with compatible native releases;
this document does not authorize a production reset or deployment.

## NEAR completion qualification

The NVIDIA signing-key download remains bounded at 1 MiB and requires the same
certificate, signed-policy anchor, ES384 signature, nonce and passing-verdict
checks. Qualify fresh NEAR attestation and authenticated completion against the
compatible platform relay before release, including a reasoning-only response
that ends at the output-token limit. Keep missing receipts, altered ciphertext,
expired evidence and oversized key sets as rejecting cases. A successful
attestation or posted usage charge does not establish authenticated completion.

## Dependency maintenance

The 0.1.15 candidate updates Rustls to 0.23.46, resolving
[RUSTSEC-2026-0346](https://rustsec.org/advisories/RUSTSEC-2026-0346.html),
[RUSTSEC-2026-0347](https://rustsec.org/advisories/RUSTSEC-2026-0347.html),
[RUSTSEC-2026-0348](https://rustsec.org/advisories/RUSTSEC-2026-0348.html) and
[RUSTSEC-2026-0349](https://rustsec.org/advisories/RUSTSEC-2026-0349.html).

The Linux Secret Service credential-store chain still requires zbus 5 and its
standalone zvariant, zbus_names and zvariant_derive crates. Upstream merged those
crates into zbus 6 and issued informational maintenance notices
[RUSTSEC-2026-0370](https://rustsec.org/advisories/RUSTSEC-2026-0370.html),
[RUSTSEC-2026-0371](https://rustsec.org/advisories/RUSTSEC-2026-0371.html) and
[RUSTSEC-2026-0372](https://rustsec.org/advisories/RUSTSEC-2026-0372.html).
The specific exceptions in [deny.toml](../deny.toml) cover those notices while
the published keyring backend requires zbus 5; they do not suppress vulnerability
advisories. Migrate when the credential-store backend supports zbus 6, verify
existing Linux credentials remain readable, and remove all three exceptions.

## Recording results

Keep unresolved requirements visible in the candidate's acceptance record. Record
fresh outcomes rather than carrying forward old test counts or failure claims.
Use synthetic accounts for screenshots and never attach credentials, gift codes,
private keys or real conversation data. Publish a concise qualification summary
with the release; retain raw operational captures privately.

## Windows setup and Store acceptance

Factory reset requires Windows VM qualification before release: start with
synthetic chats in multiple accounts, persisted Desktop/CLI settings, a native
refresh token and an MCP credential. Confirm from signed-out/disconnected
Settings, verify cleanup starts after Desktop exits, inspect both application
folders and Credential Manager, and sign in again to an empty profile. Also
exercise a locked/denied vault, another active CLI, and a file deletion failure;
incomplete resets must reopen with a visible error. Native/controller and browser
tests cover isolated files, mock credentials, exclusion, confirmation and retry;
these do not replace Windows vault and installer qualification.

The 0.1.12 offline package and website Setup were published after signed Windows
installation qualification. The live older-to-current first-launch check then
found a separate native updater failure: the canonical job path is rejected
against an ordinary Windows cache path. A local preview then exposed a second
failure: Desktop waited for bridge pipe closure while the detached helper waited
for Desktop to exit. Once Desktop closed, PowerShell's missing-parent lookup
incorrectly reported failure. These corrections must pass locally in the Windows
VM before a subsequent actual release. Do not replace the published 0.1.12
artifacts or bump versions merely to test the corrections. Local MSVC previews
passed 14 native update tests and the full standard-user first-launch handoff
from a private older-version fixture to the real signed 0.1.12 feed, including
installation, restart and unchanged user-data digest. Windows Desktop update
tests, typecheck and compilation passed as well. This is development evidence;
the corrected code has not shipped in a signed public installer. Store submission
remains blocked on that actual release's signed qualification and Partner
Center/reviewer access; the existing website Setup provides manual upgrades.

For the 0.1.12 Windows distribution candidate, record the promoted revision and final signed
bytes before release. Cross-compilation, controller/browser tests and hidden
native-control initialization checks are development evidence, not Windows Store
certification. Outstanding signed-package acceptance includes:

The 0.1.10 build failed before publication because an installer-only NSIS
variable was declared in the separately compiled uninstaller. The corrected
include guards that declaration. Both modes were checked with electron-builder's
NSIS 3.0.4.1 and warnings treated as errors; 0.1.10 remains an unpublished build.

The 0.1.11 VM qualification found that the setup process held its PowerShell
helper open for writing, preventing Windows PowerShell from reading it. Setup
1.0.1 closes that handle while retaining automatic path cleanup. The native
self-check now executes the bundled helper on both Windows architectures without
networking or installation. The final setup inventory also accepts an unset or
empty publisher setting consistently with the setup build. 0.1.11 remains
unpublished; qualification continues with 0.1.12. VM testing also caught NSIS
interpreting the forward slash in the fresh setup destination as an option
delimiter; setup now constructs that path with native Windows separators. The
helper probe runs before UI initialization to keep its synchronous subprocess
independent of the GUI lifecycle.

An additional VM upgrade check found that canonical `\\?\` destinations kept
the app files but prevented NSIS from creating shortcuts. The shared Windows
installer boundary now removes that namespace and passes `/D` as the unquoted
last argument, preserving Unicode and spaces. The Desktop installer also
normalizes paths from older clients before registering the app and creating
shortcuts. Signed native checks include that legacy update argument format.

VM qualification also exercises a previously persisted canonical installation
path, rather than only passing one to a new installer. Setup normalizes that
registry path before using Windows PowerShell's path provider. Desktop normalizes
the equivalent installation location and quoted uninstall command before running
the old uninstaller, preserving its arguments. Signed native checks cover both
Setup inspection and a default-destination upgrade from that registration.

- Fresh standard-user silent offline install at the default destination on x64
  and ARM64; shortcuts, Add/Remove Programs, no installer-triggered app launch,
  native CLI/proxy pairing, timestamped publisher and clean uninstall.
- First-launch latest/no-update/offline/error/retry/continue paths, saved-state
  handoff and one restart, including interrupted launches and no restart loop.
- Website setup on both CPUs: signed feed, digest/size/publisher rejection,
  cancelled downloads, equal/newer installed versions, lock waits, existing
  destination/scope preservation, denied UAC and installer repair.
- Defender scan, full EXE qualification and private Partner Center listing,
  funded reviewer credentials, privacy review and certification. The Store
  receives the full offline EXE; the website setup EXE is never submitted there.

The release workflow emits `windows-store-submission` with immutable full-package
references and remaining fields. Private maintainer records live in the platform
repository. Store approval covers its initial installation channel; direct website
downloads and subsequent updates can still receive Windows reputation prompts.


## Crypto funding release qualification

NOWPayments and the Zcash discount have local native, SDK, Desktop and browser
coverage. Before release, qualify the matching backend's merchant configuration
and sandbox payments, then verify Balance/network/memo/status/account switching,
the deposit dialog's keyboard picker, focus restoration, countdown expiry,
automatic renewal with the updated amount/address, safe renewal retries,
cancel-and-restart flow, the saved sent-transfer hint, continued tracking beyond
the quote deadline and small-window layout
on Windows and macOS previews. Keep Desktop and its bundled `billing@4` sidecar
together. Installer publishing remains separate release work.

Before releasing the schema-6 conversation store, qualify execution ownership on
Windows as well as macOS: keep one client running while another opens the store,
reject concurrent prompt/manual compaction, kill the owner, and recover from the
already-open observer. Run the canonical-history and export-omission regressions
against migrated account data. No release build or version change is required
for these development checks.

Windows command containment requires native VM qualification before release.
Run `cargo test -p axiomcli --lib windows_command_jobs_own_descendants` on the
Windows development checkout and verify leader exit, timeout, cancellation and
dropped-future cleanup while descendants hold output pipes. Confirm the nested
command jobs coexist with the existing process-wide exit-cleanup job. Portable
output-reader and POSIX process-tree tests do not qualify Windows behavior.
If a Windows VM is unavailable, use the manual `ci.yml` portability workflow on
a published non-production feature branch containing the exact reviewed change:
`gh workflow run ci.yml --ref <feature-branch> -f target=windows-x64 -f extended=false`.
Record the run's `headSha` and successful Windows portability job against the
reviewed source revision. This workflow runs checks; release workflows and tags
are not part of command-containment qualification.

The managed development Rust verifier replacement passed 49 signed synthetic
Python/Rust equivalence cases, hostile-input/clock/canonical-binding checks,
strict Clippy, the Python operator adapter/reference suite and an independently
locked standalone build. Its unsigned Darwin ARM64 native sidecar passed clean
environment self-test/rejection, native architecture, inventory and executable
digest checks. Packaging fixtures cover native Windows ARM64 and the retained
x64 fallback. Other platform binaries, measured worker/gateway images and live
hardware/provider acceptance remain release qualification work; no installer,
production deployment or signed trust-policy change was performed by this batch.

The A31 trust-review follow-up adds seven signed timing cases to the 49-case
corpus. NVIDIA claims and certificates are checked at post-I/O time; reusable
evidence expires no later than the earliest certificate in either token chain.
The unchanged Python reference remains the timing-equivalence oracle, with the
stricter certificate-lifetime cap recorded explicitly. VMPL, signer, TCB and
missing alias-complete negatives carry valid synthetic VCEK signatures; signature
corruption and padding remain separate rejection cases. All named valid corpus
cases are required to stay positive.
