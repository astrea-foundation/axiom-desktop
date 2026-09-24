#!/bin/sh
set -eu
[ "${1:-}" = --yes ] && [ "$#" = 1 ] || { echo 'Usage: uninstall.sh --yes (keeps account data)'; exit 1; }
payload=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
[ -f "$payload/axiom-install.json" ] || { echo 'Missing installation identity' >&2; exit 1; }
case "$payload" in
  /usr/local/lib/axiom-cli)
    for name in axiomcli axiom-proxy; do
      link="/usr/local/bin/$name"
      if [ -L "$link" ] && [ "$(readlink "$link")" = "$payload/bin/$name" ]; then rm -- "$link"; fi
    done
    rm -rf -- "$payload/bin" "$payload/licenses"
    rm -f -- "$payload/axiom-install.json" "$payload/uninstall.sh"
    rmdir -- "$payload"
    /usr/sbin/pkgutil --forget stream.axiom.cli
    ;;
  */versions/*)
    prefix=$(dirname -- "$(dirname -- "$payload")")
    [ "$(readlink -f "$prefix/current")" = "$payload" ] || { echo 'Run the current installation uninstaller' >&2; exit 1; }
    bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
    for name in axiomcli axiom-proxy; do
      link="$bin_dir/$name"
      if [ -L "$link" ] && [ "$(readlink "$link")" = "$prefix/bin/$name" ]; then rm -- "$link"; fi
    done
    for version in "$prefix"/versions/*; do
      [ -d "$version" ] && [ -f "$version/axiom-install.json" ] || continue
      rm -rf -- "$version/bin" "$version/licenses"
      rm -f -- "$version/axiom-install.json" "$version/uninstall.sh"
      rmdir -- "$version"
    done
    rm -- "$prefix/bin/axiomcli" "$prefix/bin/axiom-proxy" "$prefix/current"
    rmdir -- "$prefix/bin" "$prefix/versions" "$prefix"
    ;;
  *) echo 'Unknown installation root' >&2; exit 1;;
esac
printf 'Removed AxiomCLI and its proxy. Account data was kept.\n'
