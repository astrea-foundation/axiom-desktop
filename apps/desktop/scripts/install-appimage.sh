#!/bin/sh
# Explicit user installation: ./Axiom.AppImage --install
set -eu
if [ -z "${APPIMAGE:-}" ] || [ ! -f "$APPIMAGE" ]; then
  echo 'Run the downloaded AppImage with --install.' >&2
  exit 1
fi
if [ "$(id -u)" = 0 ]; then
  echo 'Run this installer as your normal user, without sudo.' >&2
  exit 1
fi
app_dir="$HOME/.local/share/axiom-desktop"
bin_dir="$HOME/.local/bin"
applications="$HOME/.local/share/applications"
case "$HOME" in *'
'*) echo 'Home directory contains an unsupported newline.' >&2; exit 1;; esac
mkdir -p "$app_dir" "$bin_dir" "$applications"
# Refuse to replace commands owned by another installation.
for command in axiomcli axiom-desktop; do
  link="$bin_dir/$command"
  if [ -e "$link" ] || [ -L "$link" ]; then
    if [ ! -L "$link" ] || [ "$(readlink "$link")" != "$app_dir/$command" ]; then
      echo "A different $command exists at $link; relocate it before installing." >&2
      exit 1
    fi
  fi
done
temporary=$(mktemp "$app_dir/.Axiom.AppImage.XXXXXX")
trap 'rm -f "$temporary"' EXIT HUP INT TERM
cp -- "$APPIMAGE" "$temporary"
chmod 755 "$temporary"
# Rename replaces the file atomically; an already mounted image remains valid.
mv -f -- "$temporary" "$app_dir/Axiom.AppImage"
cat > "$app_dir/axiomcli" <<'LAUNCHER'
#!/bin/sh
exec "$HOME/.local/share/axiom-desktop/Axiom.AppImage" --axiom-cli "$@"
LAUNCHER
cat > "$app_dir/axiom-desktop" <<'LAUNCHER'
#!/bin/sh
exec "$HOME/.local/share/axiom-desktop/Axiom.AppImage" "$@"
LAUNCHER
chmod 755 "$app_dir/axiomcli" "$app_dir/axiom-desktop"
ln -sfn "$app_dir/axiomcli" "$bin_dir/axiomcli"
ln -sfn "$app_dir/axiom-desktop" "$bin_dir/axiom-desktop"
# Desktop Exec quoting has different rules from shell quoting.
escaped_home=$(printf '%s' "$HOME" | sed 's/\\/\\\\/g; s/"/\\"/g; s/`/\\`/g; s/\$/\\$/g; s/%/%%/g')
cat > "$applications/stream.axiom.desktop.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Axiom
Exec="$escaped_home/.local/share/axiom-desktop/Axiom.AppImage" %U
Icon=stream.axiom.desktop
Terminal=false
Categories=Utility;
StartupWMClass=Axiom
DESKTOP
if [ -n "${APPDIR:-}" ] && [ -f "$APPDIR/resources/icon.png" ]; then
  mkdir -p "$HOME/.local/share/icons/hicolor/256x256/apps"
  cp "$APPDIR/resources/icon.png" "$HOME/.local/share/icons/hicolor/256x256/apps/stream.axiom.desktop.png"
fi
# Register for the common login/interactive shells, preserving existing contents.
for profile in .profile .bashrc .zshrc; do
  if ! grep -qF '# Axiom CLI PATH' "$HOME/$profile" 2>/dev/null; then
    cat >> "$HOME/$profile" <<'PROFILE'

# Axiom CLI PATH
case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) export PATH="$HOME/.local/bin:$PATH" ;; esac
PROFILE
  fi
done
mkdir -p "$HOME/.config/fish/conf.d"
if [ ! -e "$HOME/.config/fish/conf.d/axiom-cli.fish" ]; then
  printf '%s\n' '# Axiom CLI PATH' 'fish_add_path --path "$HOME/.local/bin"' > "$HOME/.config/fish/conf.d/axiom-cli.fish"
fi
echo 'Installed Axiom and axiomcli. Open a new terminal and run axiomcli.'
echo 'To upgrade, run the newer AppImage with --install again.'
