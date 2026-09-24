# Provider discovery and uploads

NEAR and Tinfoil offerings come from the backend's refreshed provider catalogs.
No model list, model-to-worker mapping or alias table is compiled into production.
The native registry still explicitly implements the accepted NEAR v3 and Tinfoil
EHBP security protocols. Discovery cannot authorize unverified keys, arbitrary
origins or a new encryption protocol.

The backend refreshes provider metadata every five minutes by default and stops
using a failed source after 30 minutes. Successful refreshes replace all entries,
including withdrawals. NEAR requires an exact ready, verifiable model/direct-worker
intersection; aliases cannot redirect saved conversations. Tinfoil uses its public
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
`auto` to prefer Tinfoil's DeepSeek V4.1 Flash (`deepseek-v4-1-flash`) from the
validated catalog, then other Tinfoil offerings, then NEAR. Desktop and TUI show
the same provider preference order. Explicit configuration and remembered model
choices take precedence. This is a default-selection preference, not an
availability list; models are never substituted based on aliases or similar names.

Image and file icons replace capability words in Desktop. Tinfoil uses its official
provider symbol. The composer model control shows the provider symbol to the left
of the model brand icon and name; hovering the provider symbol shows its name.
Brand artwork is presentation metadata, not a model availability
list. A previously unseen model uses the generic model icon when no brand matches.

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
