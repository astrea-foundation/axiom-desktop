# Provider discovery and uploads

NEAR and Tinfoil offerings come from the backend's refreshed provider catalogs.
No model list, model-to-worker mapping or alias table is compiled into production.
The native registry still explicitly implements the accepted NEAR v3 and Tinfoil
EHBP security protocols. Discovery cannot authorize unverified keys, arbitrary
origins or a new encryption protocol.

The backend refreshes provider metadata every five minutes by default and stops
using a failed source after 30 minutes. Successful refreshes replace all entries,
including withdrawals. NEAR offerings must be ready (or an operator-approved
preview) and verifiable; the backend routes them through NEAR's attested cloud
gateway (`near-tdx-nvidia-gateway-v1`), and the native client still accepts the
direct-worker contract from backends that advertise it. Aliases cannot redirect
saved conversations. Tinfoil uses its public
API chat catalog and fixed attested router. Prices, limits, reasoning controls and
capabilities refresh together. The [native catalog parser](../crates/axiom-secure-client/src/catalog.rs)
validates the offered contract before use.

Desktop refreshes on startup, every five minutes and on window focus. TUI refreshes
when opening its model picker, and native inference obtains the current catalog.
The native cache allows up to 30 minutes after transient transport errors; auth,
invalid metadata and withdrawals do not authorize fallback. A loaded conversation whose selection
is withdrawn stays unavailable until the user chooses another model. New-session
native preferences resolve against current offerings; Desktop requires an explicit
choice when its remembered model is unavailable. The default configuration uses
`auto` to prefer NEAR's attested, abliterated DeepSeek V4.1 Flash
(`deepseek-ai/DeepSeek-V4.1-Flash-1017-abliterated`) from the validated catalog,
then Tinfoil's DeepSeek V4.1 Flash (`deepseek-v4-1-flash`), then other Tinfoil
offerings, then NEAR. Desktop and TUI show
the same provider preference order. Explicit configuration and remembered model
choices take precedence. This is a default-selection preference, not an
availability list; models are never substituted based on aliases or similar names.

An account that still needs to accept the current policies receives a catalog
authorization error directing it to the hosted account page. Locally installed
AstreaAI offerings must not hide this error by appearing as the entire catalog.
After the user reviews and accepts the policies, returning focus to Desktop
refreshes discovery. Catalog errors never display backend-supplied text or URLs.

Image and file icons replace capability words in Desktop. Tinfoil uses its official
provider symbol. The composer model control shows the provider symbol to the left
of the model brand icon and name; hovering the provider symbol shows its name.
AstreaAI uses the official blue Astrea symbol from `astrea.systems`, bundled as
an SVG without the wordmark. The same artwork appears in the model picker and
composer, while Qwen retains its model family icon.
Axiom-hosted offerings use Axiom's colored provider symbol. Qwen3.8 Uncensored
uses the Qwen family icon; Qwen names with an attached version number resolve
to the same family artwork in the model picker and composer.
The native image tools derive their adapter policy from the actual selected
chat model for each turn: a model carrying the catalog's `uncensored` tag (for
example the abliterated DeepSeek V4.1 Flash or Qwen3.8 Uncensored) requests the
bundled uncensored image LoRA at strength **0.9**, while other models use the
base image model. The native secure client reads the tag from its accepted catalog
(refreshing it when absent or stale); a model missing from the catalog uses the
base model. Locally pinned AstreaAI and staging offerings carry the tag themselves.
Both generation and editing follow this rule, including after switching models. Tool arguments
cannot override the policy, adapter path or multiplier. The flag and adapter
metadata are bound to the encrypted image request and authenticated response.
Image-only regeneration retains the original native model identity, so switching
the chat model does not change which model's tag decides the saved image's
adapter; that model's current tag applies, and a withdrawn model uses the base model. Historical
images without that provenance use the current model.
Brand artwork is presentation metadata, not a model availability
list. A previously unseen model uses the generic model icon when no brand matches.

## Model tags and picker filters

Catalog entries may carry `tags` that the backend curates, such
as Cheap, Intelligent or Uncensored, plus an automatic New tag for recently
discovered models. Each tag has an `id`, a `label`, a `tone` from a closed color
set and a sort `position`. The [native catalog parser](../crates/axiom-secure-client/src/catalog.rs)
drops malformed or duplicate tags individually, keeps at most 16 sorted by
position then ID, and never quarantines a model because of its tags. Unknown
tones render gray. Older backends that omit the field show no tag filters.
The `uncensored` tag also selects the uncensored image adapter (see above), so
the backend's tag assignments decide that content policy. Tags never affect
routing, pricing, attestation, contract selection or availability.

The Desktop picker shows one flat list in provider preference order. Uncensored
is the first tag filter; the remaining tags follow catalog position and label.
Tag filters combine with AND; host filters (from each entry's provider) combine with OR; the
image and file toggles appear only when some, but not all, offerings accept that
input. Search also matches tag labels. Filters that would leave no models are
disabled, and filters reset each time the picker opens. Rows show the host and up
to three tags; the upstream model name moves to the row tooltip. The browser chat
consumes a vendored `@axiom/chat-ui` package and shows tags only after it is
re-vendored with this picker.

## Direct uploads and compatibility

Images and original documents go through the registered provider's E2EE path.
NEAR encrypts the complete JSON image-content array. Tinfoil encrypts inline image
and file parts inside EHBP; its attested services handle document processing.
The native client does no local extraction, OCR or format conversion. General
NEAR documents, remote URLs, audio/video and realtime WebSockets are unsupported.
See [Desktop limits](desktop.md#images-and-files), [proxy input](proxy.md) and
[ACP attachments version 2](acp-compatibility.md).

Stored file attachments preserve the original bytes. Desktop and its sidecar
negotiate direct file support together. Compaction preserves attachments as
provider inputs instead of interpolating base64 into text.
