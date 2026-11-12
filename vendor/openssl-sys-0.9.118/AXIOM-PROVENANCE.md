# Vendored OpenSSL bindings provenance

This is the published `openssl-sys 0.9.118` crate with one manifest change:
its optional `openssl-src` build dependency is pinned to the unchanged registry
package `400.0.2+4.0.3`, which supplies OpenSSL 4.0.3. No Rust bindings, build
script, C source, cryptographic algorithm, or license has been modified.

## Verified upstream inputs

| Input | Provenance |
| --- | --- |
| Original bindings archive | [openssl-sys 0.9.118](https://static.crates.io/crates/openssl-sys/openssl-sys-0.9.118.crate) |
| Original archive SHA-256 | `19c413d6ec9ebaf5e1c829d355cb3e9f783778d700f135be0921d52fce3c60ae` |
| Bindings upstream commit | [`321ba6c2877ecdd475e4a43043ea8f8f0500e37a`](https://github.com/rust-openssl/rust-openssl/tree/321ba6c2877ecdd475e4a43043ea8f8f0500e37a/openssl-sys) |
| Source archive | [openssl-src 400.0.2+4.0.3](https://static.crates.io/crates/openssl-src/openssl-src-400.0.2%2B4.0.3.crate) |
| Source archive SHA-256 | `d2093956673b8dbda85c87fa4fb78e70698ce6c5978a2837d1613f661d3bef76` |
| Source packaging commit | [`24ac583abc62b0a5e9f7698b6cda71121e71568a`](https://github.com/alexcrichton/openssl-src-rs/tree/24ac583abc62b0a5e9f7698b6cda71121e71568a) |
| OpenSSL release commit | [`af1775b60dfa141a4ad762585052cabeb9f37e9e`](https://github.com/openssl/openssl/tree/af1775b60dfa141a4ad762585052cabeb9f37e9e), official tag `openssl-4.0.3` |

All 2,484 OpenSSL files included in the source crate were verified byte for
byte against the official release tree using Git blob hashes. The crate archive
SHA-256 is also recorded in the workspace `Cargo.lock`.

## Exact patch and packaging

The only upstream file changed is the normalized `Cargo.toml`:

```diff
 [build-dependencies.openssl-src]
-version = "300.2.0"
+version = "=400.0.2"
 features = ["legacy"]
 optional = true
```

`Cargo.toml.orig` remains the unmodified upstream reference. The original
package's unused standalone `Cargo.lock` is omitted: this dependency uses the
workspace lockfile. This provenance document is the only added file.
The existing `vendored` feature and upstream static build/link logic are
unchanged on macOS, Windows, and Linux. The workspace excludes this dependency
from its members and selects it through `[patch.crates-io]`.

## Reason and compatibility

As verified on November 11, 2026, the upstream `vendored` dependency range
`^300.2.0` selects OpenSSL 3.6.3. That release predates security fixes, and the
3.6 branch is out of support. See the upstream
[vendoring issue #2714](https://github.com/rust-openssl/rust-openssl/issues/2714),
[proposed OpenSSL 4 vendoring feature #2692](https://github.com/rust-openssl/rust-openssl/pull/2692),
and [source maintenance issue #295](https://github.com/alexcrichton/openssl-src-rs/issues/295).
No published `openssl-src` package contains current OpenSSL 3.5.9 LTS.

OpenSSL 4.0.3 was released September 29, 2026. Its branch is supported through
May 14, 2027 according to the official [download and support table](https://openssl-library.org/source/).
The bindings already support OpenSSL 4 (`ossl400` / `Version::Openssl4xx`);
that support predates this manifest patch. SQLCipher 4.20.0's unchanged OpenSSL
provider uses the modern EVP APIs. Compatibility qualification here consists of
checking the actual `cipher_provider_version` and running native encrypted
storage, migration, asset, and backup tests. This does not claim that SQLCipher's
commercial packages ship this provider version.

## Removal condition and maintenance

Remove this patch when a published `openssl-sys` release can select a supported,
security-current vendored OpenSSL source package without a local manifest
change. Preserve the `cipher_provider_version` regression check and rerun the
encrypted storage/migration checks when updating the provider. Review source
security releases as well as Rust advisories: Cargo's Rust advisory database
does not establish that bundled C libraries are current. Update the provider
before its support end date. Never restore the old `300.x` source to resolve a
build failure or weaken an audit rule.
