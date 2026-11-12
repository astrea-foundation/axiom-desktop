# Encrypted local storage

Desktop and AxiomCLI keep each account/frontend's chat store on the device.
`state.sqlite3` uses SQLCipher. Managed image and document bytes are stored once
in authenticated encrypted files under `state.sqlite3.assets/`, beside the
database. The database contains asset metadata, ownership and references, never
managed image/file payloads as base64 JSON or BLOBs.

## Keys and account boundaries

Each store has a random 256-bit root secret in the operating-system credential
store, under the `stream.axiom.local-storage` service. Native code derives separate
database, asset and recovery-wrapping keys. The secret is independent of sign-in
and refresh tokens; it is absent from configuration, renderer IPC and logs.
The credential identity separates data roots, accounts and frontends.

macOS uses Keychain, Windows uses its native credential store, and Linux uses
Secret Service. A locked/unavailable store or missing key produces a recoverable
error. Existing encrypted data is preserved; there is no plaintext fallback or
automatic replacement with a new empty database. Signing out closes the database
and releases active asset/key handles without deleting the durable key.

SQLCipher protects database pages and page contents in SQLite journals/WAL.
Connections require SQLCipher support, set their key before schema access and
use memory-only SQLite temporary storage. External assets use versioned
XChaCha20-Poly1305 ciphertext with random nonces and authentication bound to the
store and asset identity. Filenames are opaque. Asset lookup verifies ownership
before reading a file and checks authentication, length and content hash.

Content deduplication applies to binary bytes within a store. Conversation image
IDs, names, generation proofs, reference ordering and original/preview identities
remain distinct. Runtime readers hydrate the existing message types when needed;
base64 may still appear in an in-memory protocol requiring it. Catalog and
conversation image counts have no lifetime limit; model/token and per-request
resource bounds still apply.

## Forward migration

Opening an authenticated account upgrades its existing store. A consistent
plaintext SQLite database, including committed WAL data, is exported into a new
encrypted file. The migration preserves application/schema identifiers and
validates schema, row contents and references before replacement. Synced migration
markers make interrupted replacement recoverable, and missing/wrong keys never
authorize deleting the original. Other native writers must be closed for storage
maintenance; a busy migration is retried rather than bypassing the fence.
Desktop shows a dedicated storage-update screen while an existing account is
upgraded, including after browser approval and when restoring a signed-in
session. It reports database encryption, media conversion, verification and
final cleanup. Record/file counters describe the current phase; indivisible
database work has an indeterminate indicator. Storage work runs outside the ACP
executor so progress remains visible. The account becomes usable only after the
upgrade succeeds. Failure keeps a recovery screen with details and retry;
reconnecting resumes through the same durable migration markers. New accounts
and stores that are already current do not show a migration screen.
Older builds without SQLCipher support cannot open the migrated store. Preserve
an encrypted backup and recovery key before moving the store to another device.

The schema migration decodes recognized image/file slots in accepted attachments,
generated-image timeline content, authoritative events and exact model-message
checkpoints. It writes durable ciphertext files before committing asset references
and preserves checkpoint message IDs and untouched message hashes. In these
explicit typed slots, malformed or missing inline media is omitted during
conversion. Byte-free unavailable attachment slots preserve original positions
and surviving image IDs. Usable text, tool outcomes and unaffected media remain
available. New uploads cannot supply these migration-only markers.

Before committing an omission, migration archives the exact original record in
encrypted, thread-owned recovery assets. Large records use ordered chunks. These
records are never loaded as media, remain available for offline recovery and
portable backup, and are released when their thread is deleted. An omitted
canonical message keeps its numeric checkpoint ID and receives a separate
recovery deduplication identity; its original digest is retained in the recovery
record. Reintroducing the original valid message can therefore succeed without a
false digest collision. Valid untouched message digests stay stable.

Omission, recovery ownership and reference updates commit in the same migration
transaction. A storage-write failure leaves the original database records intact
for retry; orphan staged ciphertext is reclaimed after a successful retry.
Unsupported schemas, invalid database/key authentication and unrelated malformed
records remain errors. The converter never scans text or arbitrary tool arguments
for media. Cleanup removes obsolete payload pages and truncates the WAL before
migration completes.

Only unreferenced ciphertext is reclaimed. Compaction alone does not erase
original images or history needed for viewing, editing, revision or replay.
Draft replacement updates its asset ownership; deleting a draft or thread releases
its owners. Cleanup is serialized with SQLite writers so rollback cannot restore
a reference to a prematurely deleted file.

## Unavailable saved media

A missing, unreadable, tampered or wrong-key external asset does not prevent
opening the account or using unaffected conversations. Reads still verify the
owning record, asset identity, ciphertext authentication, length and content hash
before returning any bytes. An unavailable asset supplies no image or file bytes.
History, generated-image records, exact checkpoints, attachments, drafts and queued
preparations omit the unavailable media while preserving usable text, tool
arguments and recorded tool outcomes. Recovery warnings identify the omission;
queued records remain saved for review after reconnect and are not automatically
sent. Lazy timeline image descriptors do not authenticate or load an image;
opening an unavailable image reports that it cannot be restored.

Recognized images must decode completely through their first frame with a MIME
type matching the encoded format. The storage boundary checks canonical base64,
encoded byte limits (5 MiB for uploads/model inputs, 10 MiB for generated originals),
an 8192-pixel edge limit, a 32-megapixel source limit and a
128 MiB decoder allocation limit. Signature-only or truncated images are omitted
on both legacy migration and current external-asset reads; malformed new image
writes are rejected. Validation never re-encodes or substitutes valid bytes.
Unreadable external originals and their ownership remain intact for recovery.

Semantic replay uses text-only omission markers where needed to preserve original
attachment positions and the IDs of surviving uploaded images. A media-only
canonical message receives a short unavailable-media note if omitting its bytes
would leave it empty. These recovery projections do not rewrite the original
saved records, invent authenticated media, restore live provider trust, or imply
that an interrupted tool operation succeeded. Unrelated text and JSON are never
interpreted as asset references.

Broken references retain their ownership and any original ciphertext. Opening
and compaction do not delete them. Reattaching the exact original valid bytes
repairs an unavailable deduplicated asset under the storage write lock, preserving
its ID and existing owners. Before replacement, a damaged regular file is moved
beside the assets under an opaque `.unavailable-` name. Such quarantine files are
never resolved as media and remain available for offline recovery; ordinary
orphan collection does not remove them. Ordinary draft/thread deletion still
releases live ownership as before. Explicit portable-backup verification remains
strict and reports unavailable live assets rather than certifying a damaged
archive as complete; conversation export can preserve usable text and tool
outcomes with omission warnings.

Runtime semantic restore selects the newest authoritative checkpoint before the
requested revision boundary and the events after it. Earlier assistant text,
reasoning, tool outputs, file bytes and superseded checkpoint media are not
hydrated. A small earlier state/lifecycle projection retains model and thinking
settings, workspace changes, turn IDs and unknown interrupted side effects.
Earlier valid uploads and generated images remain available to the image catalog;
the current catalog still hydrates those images, so catalog size can contribute
to restore cost. TUI and standard ACP transcript replay use a separate complete
presentation load; image regeneration reads its original recipe through that
same path. Desktop can skip presentation replay and fetch timeline pages instead.
The complete presentation timeline, export source and earlier revision checkpoints
remain in storage.

Desktop migrates existing browser drafts and outbox records after the owning
account unlocks. Native commits acknowledge imported data before localStorage or
IndexedDB copies are removed. Other accounts are imported when they unlock.
Legacy anonymous drafts acquire an owner at first unlock; conflicts are retained
as recovered saved preparations and never sent automatically. New anonymous
drafts remain in memory. Native persistence errors retain in-window data and block
durable submission instead of falling back to plaintext browser storage. Encrypted
receipts make browser cleanup retryable without replaying already imported queue
entries or discarding a draft edited after an earlier import.

The `localPayload@2` legacy-import action validates only recognized typed media
slots, omits damaged image/file inputs and returns the normalized source plus an
omission count. Authored text remains exact, including at the prompt-size limit.
Ordinary new writes continue to reject malformed media. Native creates an import
destination only if absent; acknowledgement retries compare the normalized source
and preserve newer native edits as well as changed browser sources. A preview
mode archives and normalizes anonymous inputs before deciding whether both
composer fields can be adopted. It never creates or overwrites those fields.

Before browser cleanup, the import transaction archives omitted prompt inputs
and complete affected queue-item records as encrypted recovery capsules. Each
capsule identifies the SHA-256 of the full serialized source, so ordered chunks
from multiple changed versions can be reconstructed independently. Recovery uses
separate `payload-recovery:` ownership: ordinary draft/outbox replacement retains
the archive, while explicit payload or account deletion releases it. Damaged
imports add review notices to saved queue/error metadata or recovered preparations,
without appending notices to authored text or resending messages. A sidecar that
supports only `localPayload@1` can still perform ordinary persistence; legacy
import requires an updated native app and keeps its browser source until then.

Text draft writes use a short debounce. Normal window close, quit, update restart
and explicit account transitions flush pending writes and wait for native
acknowledgement before closing the current storage context. A failed or timed-out
close keeps the window and sidecar open for retry. Force-killing a process or
reloading a page before that acknowledgement can lose the latest uncommitted edit.

## Portable backup and recovery

Close Desktop, CLI and local proxy processes before offline maintenance. Back up
one account/frontend store with:

```sh
axiomcli local-data backup --account ACCOUNT_ID --frontend desktop-chat --output chats.axbackup
```

The archive contains a consistent encrypted database snapshot and encrypted asset
files. Its authenticated manifest binds the account, frontend, filenames, lengths
and digests. A fresh recovery key wraps the storage secret and is printed once
for this explicit export. Keep it separately from the backup, such as in a
password manager. The raw storage secret is never exported.

To restore on another installation, pipe that recovery key through standard input:

```sh
axiomcli local-data restore --input chats.axbackup --recovery-key-stdin
```

The command accepts the recovery key from a pipe, not a command argument or an
interactive echoed prompt. It restores only into an empty account/frontend store,
verifies ciphertext and references, installs the recovered key in the credential
store and activates the database/assets as one directory bundle. It never
overwrites existing chats. If interrupted, retry with the same backup; the pending
restore marker blocks ordinary opening until recovery completes.

If the encrypted database and assets are already present but their OS credential
entry was lost, recover only the key:

```sh
axiomcli local-data recover-key --input chats.axbackup --recovery-key-stdin
```

The existing database must unlock with the recovered key. A different credential
entry is never silently overwritten. Account password reset/sign-in cannot
recover a lost random local storage secret without a recovery backup.

## Scope and validation

Encryption protects app-managed persisted chat data. A running unlocked client
necessarily holds plaintext in memory. Explicit image saves, session exports,
clipboard contents, original upload source files and Agent workspace outputs are
user-controlled files outside this encrypted store. Migration cannot erase
plaintext retained by older external backups, OS snapshots or SSD history.

Native encryption, asset migration and browser import use synthetic fixtures;
development tests never migrate real chats or access real credential entries.
The release acceptance guide retains required Windows/Linux, credential-store
and packaged application qualification. See [privacy](privacy.md),
[configuration](configuration.md#paths-and-local-state) and
[release acceptance](validation.md).
