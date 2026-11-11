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
Older builds without SQLCipher support cannot open the migrated store. Preserve
an encrypted backup and recovery key before moving the store to another device.

The schema migration decodes recognized image/file slots in accepted attachments,
generated-image timeline content, authoritative events and exact model-message
checkpoints. It writes durable ciphertext files before committing asset references
and preserves message/checkpoint IDs and hashes of hydrated model messages.
Corrupt/missing media causes a recoverable error, not silent omission. Cleanup
removes obsolete payload pages and truncates the WAL before migration completes.

Only unreferenced ciphertext is reclaimed. Compaction alone does not erase
original images or history needed for viewing, editing, revision or replay.
Draft replacement updates its asset ownership; deleting a draft or thread releases
its owners. Cleanup is serialized with SQLite writers so rollback cannot restore
a reference to a prematurely deleted file.

Desktop migrates existing browser drafts and outbox records after the owning
account unlocks. Native commits acknowledge imported data before localStorage or
IndexedDB copies are removed. Other accounts are imported when they unlock.
Legacy anonymous drafts acquire an owner at first unlock; conflicts are retained
as recovered saved preparations and never sent automatically. New anonymous
drafts remain in memory. Native persistence errors retain in-window data and block
durable submission instead of falling back to plaintext browser storage. Encrypted
receipts make browser cleanup retryable without replaying already imported queue
entries or discarding a draft edited after an earlier import.

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
