# Release acceptance

Qualify the exact promoted source revision and signed installer bytes. Record
OS, architecture, browser, source revision, artifact SHA-256, signing identity,
result and limitations in the release's maintainer acceptance record. Unit tests
and browser fixtures do not establish signed native OS acceptance.

## Direct Verda rollout gates

The [generation-9 / permanent-policy-14 qualification](../../axiom-inference/docs/qa/direct-verda-reference-images-2026-11-03.json)
establishes fresh CPU/GPU/workload/key/TLS verification, authenticated text and
vision, five-reference image generation/editing and the 1536-square default in
the operator's macOS development connection. Its fresh QA account and separate
profile do not establish a hosted customer rollout.

Public release remains blocked on these concrete requirements:

- Replace the client-held operator issuer key with production account-owned
  reservation/grant issuance. Each grant must bind the authenticated account,
  session, client proof key, exact approved worker and one request. Distribute
  public trust material and the verifier to clients; keep private issuer and
  publisher keys under operator/service control.
- Register and select the Verda deployment through the production account API
  and model catalog. The current installed Direct configuration explicitly
  requires a loopback account API and an external private configuration file;
  the development launcher and local-profile overrides are not the public
  enrollment path.
- Exercise real independent accounts concurrently against the production
  authorization/routing path, including cancellation, account switching,
  wrong-request responses, incomplete streams and worker replacement. Retain
  strict attestation and authenticated completion throughout.
- Qualify bounded admission/queueing and recovery under the intended customer
  rollout. The capacity branch reserves trusted input plus maximum output tokens,
  permits four short chats within 65,536 reserved tokens, queues at most eight
  requests for 30 seconds, and returns an authenticated busy refusal without
  replay. Long 32K requests can still reduce concurrency to two. Worker/queue
  tests and an operator load test do not establish real-account routing or
  sustained customer capacity. Recovery also needs an always-on external Verda
  watchdog and redundant ready capacity for a production availability claim.
- Package the independent verifier and public trust configuration for every
  supported installer target, then qualify production sign-in, the installed
  provider, image/reference flows, account-data preservation and updates using
  the exact signed candidate bytes. Source-mode macOS checks do not establish
  Windows, Intel macOS, Linux or signed-installer acceptance.

Keep this gate separate from the existing managed Chutes release qualification.
Publishing the current operator branch alone does not enroll ordinary customer
installations into Verda. See [the Direct connection guide](direct-verda.md)
and [release procedure](releasing.md).

## Confidential pool qualification

The separate registered managed Chutes channel now has account-owned opaque
backend routing, local vendor verification, publisher authorization, independent
usage signatures, native cost ceilings and billing settlement. Its runtime
trust scope is documented in the [managed guide](../../axiom-inference/docs/chutes-managed.md).
Keep its qualification distinct from the stronger pool requirements below.
Public staging rollout still needs the authorized source/feed and backend
deployment, actual worker callback origin and installed native bundle. Windows
and signed installer helper packaging require separate release validation.
The optional self-contained helper payload now has macOS Apple Silicon and
Ubuntu 24.04 x86-64 build, relocation and stdio checks, inventory verification and a packaging hook; its
native executable pin is checked before startup. Signed installer acceptance
and older Linux/Intel macOS target builds remain separate; Windows is disabled.
See the [helper build guide](../../axiom-inference/docs/native-helper.md).
Warm redundant capacity and
draining/replacement testing are required for a production availability claim.

The qualified Chutes pool offering remains discovery only. A separate debug-only
interactive adapter permits local encrypted testing. Its signed staging variant
verifies hardware, the Chutes recipient key and publisher authorization, with
execution provenance explicitly outside that claim. Before enabling production
custom pool inference or claiming exact application/model execution, verify the exact application/model
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
TypeScript checks, Desktop unit/browser tests and the production build. Use the
[release procedure](releasing.md#ci-baseline) to reuse successful CI for the exact
candidate instead of repeating unrelated jobs. The live release gate uses the
registered provider's attested E2EE protocol; it does not replace interactive QA.

The Desktop unit suite also builds the production Electron main entry and
rejects runtime imports of workspace TypeScript packages. Native installer
qualification must launch the installed app and reach its first window; a
successful renderer build or source-mode launch does not establish that the
packaged main process can start.

## Interactive qualification

Signed [backend runtime settings](../../axiom-inference/docs/runtime-updates.md)
now drive image tool defaults/ratios, model-specific LoRA policy, sampling
validation and hosted deployment selection. Native stdio regressions cover
changes on a reused helper, substituted policy bindings and unsupported settings
rejected before dispatch. Initial runtime-capable worker, backend migration and
installed-helper live qualification remain deployment requirements. The October
30 GPU record below covers the previous fixed policy.

The development `imageGeneration@3` contract exposes `generate` with up to five
ordered references and uses the ComfyUI image engine. The October 29 policy
restores the pinned uncensored LoRA at 0.9 for the exact uncensored model IDs.
Native helper tests cover alternating model policies on one reused helper and
rejecting missing or substituted adapter metadata. Regeneration tests preserve
the original model's policy after a model switch. The
[October 30 live qualification](../../axiom-inference/docs/qa/image-lora09-b-2026-10-30.json)
records the deployed private GPU image and installed standalone helper, fresh
hardware checks, uncensored/base/uncensored generations, five references,
authenticated progress and exact base-model restoration. These checks used
synthetic fixtures without opening personal threads or images. Native
checks cover reference order, five-reference to text-only transitions, duplicate
and foreign IDs, bounded conditioning copies, output dimensions and optional
viewing. Broader co-resident load, encrypted multi-account qualification and
signed installer acceptance remain release requirements for the new build.
The October 27 evidence below covers the
previous engine and tool contract.

The [October 28 Comfy qualification](../../axiom-inference/docs/qa/comfy-implementation-2026-10-28.json)
records five successful private GPU checks, including ordered five-reference
conditioning, a following text-only request, generated-reference editing and
two-account isolation. A local Desktop check also completed `generate`, optional
`view_image`, authenticated chat completion and signed settlement using the
standalone helper. Managed-request expiry regression checks cover the 15-minute
deadline, account isolation, no-charge late receipts and safe native admission
errors; the [expiry validation record](../../axiom-inference/docs/qa/chutes-request-expiry-2026-10-28.json)
includes background recovery and the updated standalone-helper Desktop run.
These development results do not replace public staging or signed
installer acceptance.

The unified `imageGeneration@2` tool and automatic vision-model gating replace
the earlier Images toggle. Native regression checks cover generation/edit
dispatch, conversation reference isolation, model switching, unavailable tools
and nonvision upload rejection. Renderer checks cover toggle removal and the
generated-image viewer. The
[October 27 live source-mode qualification](../../axiom-inference/docs/qa/chutes-vision-staging-live-2026-10-27.json)
records a fresh verified Chutes worker, ordered ten-image vision with streaming,
no image creation during an analysis-only request, 1536-square generation and
edits from both source types, signed billing and the full-image viewer. It also
checks local development credentials across rebuilds and restarts. Installed
app checks, cross-platform release validation and broader load tests remain
release requirements; source-mode qualification does not replace them.

Generated-image presentation checks also cover one-time binary IPC payloads,
stable Blob URLs through streaming updates, account/runtime resource revocation,
late bootstrap recovery, 1536-pixel viewing while typing, and cancellable inline
decoding that yields to UI tasks. These local transport checks do not change
provider attestation, authenticated completion, original image bytes or size caps.

Optional `view_image` inspection has native regression coverage for uploaded,
generated and edited images, generation followed by viewing before the final
answer, tool-batch ordering, persisted restoration, malformed/foreign references,
decode failures, cancellation, model/service gating and the ten-image context
limit. Vision-copy checks include transparent images and generated PNGs larger
than the upload byte limit. The October 27 live record predates this viewing
tool; live model selection of the new tool remains a separate qualification.

Image-progress checks cover authenticated request/sequence binding, rounded
sampling percentages, truncation and forged-update rejection, queue cancellation,
clean transport completion, native propagation and durable tool metadata. Browser
checks cover percentage bars, the 99%-to-100% completion boundary, and distinct
random filenames for generated images and repeated Save actions. The matching
worker/backend/helper rollout and actual GPU preview-progress behavior still
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
