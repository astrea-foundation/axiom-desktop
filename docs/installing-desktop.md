# Install Axiom Desktop and AxiomCLI

Each desktop download includes the matching command-line program, `axiomcli`.
You do not need Rust, Node.js, or a separate CLI download. Choose the build for
your operating system and processor. These examples use version 0.1.0.

## Windows

Run `Axiom-0.1.0-win-x64.exe`, or the `arm64.exe` build on Windows on ARM,
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

The **PKG** is the easiest way to install both programs. Choose
`Axiom-0.1.0-mac-arm64.pkg` for Apple Silicon or
`Axiom-0.1.0-mac-x64.pkg` for Intel. Run the installer and approve its normal
administrator prompt. It installs `/Applications/Axiom.app` and makes
`axiomcli` available from Terminal through `/usr/local/bin/axiomcli`.

For the **DMG or ZIP**, copy Axiom.app into Applications, open it, and choose
**Axiom → Install Command-Line Tool…** to register the CLI. This step also
requires administrator authorization. An unrelated command already named
`/usr/local/bin/axiomcli` is preserved; resolve that collision first.

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
sudo apt install ./Axiom-0.1.0-linux-amd64.deb
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
chmod +x Axiom-0.1.0-linux-x86_64.AppImage
./Axiom-0.1.0-linux-x86_64.AppImage --install
```

Open a new terminal to use `axiomcli`. The installed app and CLI use a durable
copy under `~/.local/share/axiom-desktop`; you can then remove the original
download. Running the AppImage without `--install` opens it portably. Use
`./Axiom-0.1.0-linux-x86_64.AppImage --axiom-cli --help` for the portable CLI.

AppImages require working FUSE support and the desktop's system libraries.
If mounting fails, use your distribution's package above or follow the
[AppImage FUSE setup instructions](https://docs.appimage.org/user-guide/troubleshooting/fuse.html).

## Install a newer version

Quit Axiom and any running AxiomCLI sessions, then install the newer download
using the same format and installation location. The Windows EXE, macOS PKG,
DEB, and Arch package replace the existing program. For DMG/ZIP, replace
`/Applications/Axiom.app`; the existing CLI link continues to work. For
AppImage, run the newer file with `--install` again.

These installers keep account data separately from program files and preserve
it during replacement. Avoid mixing the AppImage and system-package installs:
remove the previous program installation when switching formats to prevent
duplicate launchers. Automatic in-app updating is not part of these installers.
