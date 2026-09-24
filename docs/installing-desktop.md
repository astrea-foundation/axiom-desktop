# Install Axiom Desktop and AxiomCLI

Each desktop download includes the matching command-line program, `axiomcli`.
You do not need Rust, Node.js, or a separate CLI download. Choose the build for
your operating system from the published download catalog.
These filenames illustrate version 0.1.8; they do not assert its current
availability. [Supported build targets](releasing.md#supported-platforms) and
[Desktop behavior](desktop.md) are documented separately.

## Windows

Run `Axiom-0.1.8-win-universal.exe`. It selects the native x64 or ARM64 payload,
and complete the installation wizard. Open Axiom from the Start menu. Open a
new terminal window and run:

```text
axiomcli --version
axiomcli --help
```

The installer adds the CLI to PATH and includes its Visual C++ runtime.
If an already-running terminal application retains the old PATH, close that
application completely and reopen it.

Unsigned early-access installers can trigger SmartScreen. If you obtained the
file from the trusted Axiom release, the warning may offer **More info → Run
anyway**. Managed-device or Smart App Control policies can prevent this option.
See [Microsoft's SmartScreen documentation](https://learn.microsoft.com/en-us/windows/security/operating-system-security/virus-and-threat-protection/microsoft-defender-smartscreen/).

## macOS

Requires macOS 11 (Big Sur) or newer. One universal download supports Intel and
Apple Silicon; the app includes its runtime and needs no Homebrew installation.

The **PKG** is the easiest way to install both programs. Choose
`Axiom-0.1.8-mac-universal.pkg` for either processor. Run the installer and approve its normal
administrator prompt. It installs `/Applications/Axiom.app` and makes
`axiomcli` available from Terminal through `/usr/local/bin/axiomcli`.

Unsigned early-access builds are not notarized or signed with an Apple
Developer ID. After attempting to open the trusted Axiom download, use
**System Settings → Privacy & Security → Open Anyway** if macOS offers that
option, then confirm opening it. The app may require approval separately from
the installer. See [Apple's instructions](https://support.apple.com/en-us/102445).

## Linux

The release builds target x86-64 Linux with glibc 2.35 or later. Use the package
format for your distribution so its package manager installs the dependencies.

Ubuntu/Debian:

```sh
sudo apt install ./Axiom-0.1.8-linux-amd64.deb
axiomcli --version
```

Arch Linux:

```sh
sudo pacman -U ./axiom-desktop-0.1.0-x64.pkg.tar.xz
axiomcli --version
```

Both packages register Axiom in the applications menu and own
`/usr/bin/axiomcli`.

For an installation under your home directory, run the **AppImage** installer
as your normal user:

```sh
chmod +x Axiom-0.1.8-linux-x86_64.AppImage
./Axiom-0.1.8-linux-x86_64.AppImage --install
```

Open a new terminal to use `axiomcli`. The installed app and CLI use a durable
copy under `~/.local/share/axiom-desktop`; you can then remove the original
download. Running the AppImage without `--install` opens it portably. Use
`./Axiom-0.1.8-linux-x86_64.AppImage --axiom-cli --help` for the portable CLI.

AppImages require working FUSE support and the desktop's system libraries.
If mounting fails, use your distribution's package above or follow the
[AppImage FUSE setup instructions](https://docs.appimage.org/user-guide/troubleshooting/fuse.html).

## Standalone CLI and proxy

Choose the **AxiomCLI** installer when you do not need Desktop: NSIS EXE on
Windows x64/ARM64, PKG on macOS Intel/Apple Silicon, or SH on Linux x64.
Public releases offer the same installer bytes from the CDN and the public
[GitHub distribution repository](https://github.com/astrea-foundation/axiom-releases/releases).
Availability follows release qualification; source build targets are not a claim
that these files have already been published.

On Linux run `sh AxiomCLI-VERSION-linux-x64.sh`. It installs into
`${XDG_DATA_HOME:-~/.local/share}/axiom-cli` and links commands into
`${XDG_BIN_HOME:-~/.local/bin}`. Add that command directory to PATH if necessary.
Use `--prefix /absolute/dedicated/directory` for a different program location.
Version directories contain the CLI and proxy together. For removal, run
`sh INSTALL_ROOT/current/uninstall.sh --yes` after stopping both programs.

The macOS standalone PKG owns `/usr/local/lib/axiom-cli` and links both commands
from `/usr/local/bin`; removal is
`sudo sh /usr/local/lib/axiom-cli/uninstall.sh --yes`. Windows installs per-user,
adds its launcher directory to PATH, and provides an Apps/Settings uninstaller.
Uninstall preserves account data. Desktop and standalone CLI command collisions
must be resolved before changing the installation owner.

## Install a newer version

Open **Settings → Updates → Update and restart**. Axiom downloads and verifies
its matching installer, waits for running chats and its proxy to finish, saves
local drafts, closes, installs and reopens. Cancellation is available before
installation starts. Stop other CLI/proxy sessions when asked; they are never
forcibly killed. System packages may display the OS authorization dialog.

In the TUI use `/update`, or run `axiomcli update`; use `--check` to check without
installing. A bundled TUI updates the whole Desktop package even when Desktop is
closed. A standalone TUI updates CLI and proxy together. The TUI resumes its
workspace/session; runtime-only attestation consent resets on restart.

Checks run on startup and every six hours. Update metadata has an Ed25519
signature verified by a key compiled into the client, and every download must
match its signed size and SHA-256. There are no account credentials in update
requests. Source builds and unsigned previews do not receive stable updates.

Desktop recognizes the combined macOS and Windows installers on both x64 and
ARM64. Installer downloads and copied helpers are removed after an installation
attempt; the last result and signed feed remain available. Files still in use by
Windows are retried on the next update check. Abandoned downloads are pruned
after seven days on a check or a new download, while active downloads/installers
hold a lock that prevents pruning.

A failed download or verification leaves installed programs untouched. For an
installer failure, retry the same package from the signed release. Programs keep
account data separately from installation files. Keep the same package format;
use the OS uninstaller before switching installation owners to avoid duplicate
launchers. See [troubleshooting](troubleshooting.md#updates).
