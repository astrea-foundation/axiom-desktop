# Platform support

Axiom has two distinct platform surfaces:

- AxiomCLI's coding-agent tools are currently exercised and released on
  `x86_64-unknown-linux-gnu`. Bubblewrap is mandatory for local process and
  filesystem tool isolation, so the coding-agent permission profiles are not a
  macOS or Windows support claim.
- Axiom Desktop is packaged as a native application for macOS Intel (`x64`),
  macOS Apple Silicon (`arm64`), Windows x64, and Windows ARM64. Its
  `desktop-chat` profile exposes Web tools only and never grants local
  file/process or MCP execution.

Each Desktop artifact contains an AxiomCLI sidecar built on a native runner for
the same operating system and architecture. Electron resolves that binary only
from the packaged application resources. The release workflow produces signed,
notarized DMG and ZIP artifacts for both macOS architectures and
Authenticode-signed NSIS installers for both Windows architectures.

The cross-platform account contract is the same everywhere. Native sign-in uses
the system browser; passkey, Google, email/password, and Ethereum-wallet
credentials remain in the hosted auth surface. Axiom persists only a rotating
refresh token in the platform credential vault (Keychain on macOS, Credential
Manager on Windows, or a supported Secret Service on Linux). There is no
plaintext credential-file fallback. Local threads are partitioned by opaque
account ID under the platform-native application-data directory. An explicit
absolute `XDG_CONFIG_HOME` or `XDG_DATA_HOME` overrides those native roots on
every platform; relative values are rejected.

The release workflow statically and headlessly tests and packages all four
Desktop targets. Interactive acceptance checks—system-browser return,
passkey/Google/password/wallet sign-in, credential-vault behavior, installer and
uninstaller UX, window controls, accessibility, sleep/resume, and OS security
prompts—must be completed on real target machines before calling a particular
release generally available. These checks are deliberately separate from the
automated packaging claim and must not be inferred from a successful CI build.

Linux musl, Linux Arm, LAN binding, and multi-user proxy hosting are not release
claims. Platform availability never enables a plaintext inference fallback:
every supported client still requires fresh, locally verified provider-E2EE
attestation and fails closed when it cannot establish it.
