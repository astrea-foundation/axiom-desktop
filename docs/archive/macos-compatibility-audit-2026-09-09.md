# macOS compatibility audit — September 9, 2026

> Historical record, archived 2026-09-21. This preserves the original proposal
> or dated evidence, including superseded behavior and unfinished checklists.
> It is not a current implementation or deployment runbook. See the
> [current guides](../README.md) and [validation follow-up](../validation.md).

The audit covered the current Desktop/AxiomCLI checkout, the hosted API/auth
integration boundary in `/home/salt/clone/axiom`, and the existing 0.1.0 Intel
and Apple Silicon installers in `dist/desktop-0.1.0`. Inspection and new tests
ran on Linux. Existing GitHub Actions logs supplied the native Mac installation
evidence; this audit did not run a new Mac job or publish installers.

## Findings and changes

1. **IME confirmation could send an unfinished message. Fixed in source.**
   `ChatComposer.tsx` treated every unshifted Enter as Send, including the key
   that confirms a Japanese, Chinese, or Korean input-method candidate. The
   handler now leaves composing events and IME key code 229 to the input method.
   Browser regressions reproduced the failure before the fix on all three
   renderer layouts, then passed afterward. They check that confirmation keeps
   the draft and starts no thread/request, Shift+Enter inserts a newline, and a
   separate Enter sends exactly once. This is a shared input bug affecting Mac
   users, not a Mac-exclusive bug.

2. **Current downloads can be blocked at first launch by Gatekeeper. Still open.**
   The existing early-access artifacts have ad-hoc integrity signatures, not
   Apple Developer ID signatures or notarization. Native CI's successful
   `codesign --verify` does not establish Gatekeeper acceptance of a quarantined
   browser download. Users may need the per-app approval described in
   [Apple's instructions](https://support.apple.com/en-us/102445); managed Macs
   can restrict overrides. A normal trusted release requires the Apple signing
   certificates and notarization credentials documented in `releasing.md`.
   Signing defaults already require credentials, so no security bypass was added.

3. **Native test coverage was blocked by an unrelated formatting failure.
   CI configuration improved.** Both Mac portability jobs in
   [run 34401374836](https://github.com/astrea-foundation/axiomai/actions/runs/34401374836)
   stopped at `cargo fmt`, before compilation and tests. The duplicate formatting
   step was removed from the portability job; the separate Rust job still
   enforces formatting. The installer workflow also now launches the installed
   Electron runtime in Node mode and evaluates JavaScript without opening a
   window. This extends its existing Rust CLI startup check. The new Mac check
   awaits the next workflow run and does not test renderer, GPU, or login behavior.

4. **macOS 11 is the minimum, including on Intel. Made explicit.** The current
   app plists, Electron binaries, and both PKG distributions already require
   macOS 11. Older Intel Macs running Catalina or earlier cannot run this build.
   `mac.minimumSystemVersion` now states `11.0` explicitly, and the installation
   guide documents the requirement. This records the existing artifact limit;
   it does not add support for older macOS versions.

## Artifact and code checks

- Inspected 16 Mach-O executables/libraries per ZIP. Every architecture matches
  its package. Library imports use Apple system paths or bundle-relative paths;
  runtime search paths are bundle-relative. No Homebrew, build-machine, or
  unexpected absolute library dependency was found.
- The Rust sidecars link only to Apple system libraries/frameworks. Their
  deployment targets do not exceed Electron's macOS 11 minimum.
- The native Intel and ARM64 installer jobs in
  [run 34405827843](https://github.com/astrea-foundation/axiomai/actions/runs/34405827843)
  passed PKG installation, CLI version/help, command symlink, recursive signature
  verification, and reinstall preserving a test data marker. That run failed
  overall on Windows ARM64, not either Mac job.
- Reviewed packaged sidecar resolution, Homebrew/tool PATH additions, native
  Keychain selection, platform data roots, numeric loopback callbacks and polling,
  native traffic lights, Edit-menu shortcuts, close/reopen behavior, and process
  cleanup. No additional confirmed Mac launch blocker was found in those paths.

## Validation and remaining limits

The six targeted browser checks and seven packaging/window-chrome checks passed,
as did Desktop typechecking, production build, and parsing the modified YAML.
The input regression covers synthetic IME events in Chromium on Linux; it does
not substitute for the real macOS input-method UI.

Still needed on real Macs: first launch from a quarantined download, Keychain
save/refresh/logout and upgrade prompts, browser sign-in return, Dock reopening,
renderer/GPU behavior, clipboard and native dialogs, and long streaming across
sleep/wake. Intel and Apple Silicon should both be exercised. No live inference
was run during this audit, and provider attestation/E2EE handling was unchanged.

The source fixes and CI improvements are local changes. The existing published
0.1.0 installers have not been rebuilt or replaced by this audit.
