#!/bin/sh
# Download the native app; no Python, Git or Rust compiler is used.
set -eu

download() {
    if command -v curl >/dev/null 2>&1; then curl --proto '=https' --tlsv1.2 -fsSL --retry 2 "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then wget -q "$1" -O "$2"
    else printf '%s\n' 'Install curl or wget and try again.' >&2; return 1; fi
}
checksum() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d ' ' -f 1
    elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d ' ' -f 1
    else printf '%s\n' 'A SHA-256 tool (sha256sum or shasum) is required.' >&2; return 1; fi
}
add_path_line() {
    if [ ! -f "$1" ] || ! grep -Fqx "$line" "$1"; then
        printf '\n%s\n' "$line" >> "$1"
    fi
}
main() {
    version=${PORTS_VERSION:-0.8.1}
    case "$version" in ''|*[!A-Za-z0-9.-]*) printf '%s\n' 'Invalid release version.' >&2; return 1;; esac
    install_root=${PORTS_INSTALL_DIR:-${XDG_DATA_HOME:-"$HOME/.local/share"}/ports-install}
    case "$install_root" in /*) ;; *) printf '%s\n' 'PORTS_INSTALL_DIR must be absolute.' >&2; return 1;; esac
    if [ -d "$install_root" ] && [ ! -f "$install_root/.ports-installer" ] && [ -n "$(ls -A "$install_root")" ]; then
        printf '%s\n' "Choose an empty install directory: $install_root contains other files." >&2; return 1
    fi
    if [ -f "$install_root/.ports-installer" ] && [ "$(cat "$install_root/.ports-installer")" != port-forward-tui ]; then
        printf '%s\n' 'This install directory belongs to another application.' >&2; return 1
    fi
    case "$(uname -s)/$(uname -m)" in
        Darwin/arm64) target=aarch64-apple-darwin;;
        Darwin/x86_64) target=x86_64-apple-darwin;;
        Linux/x86_64) target=x86_64-unknown-linux-musl;;
        Linux/aarch64|Linux/arm64) target=aarch64-unknown-linux-musl;;
        *) printf '%s\n' 'No prebuilt release is available for this operating system/CPU.' >&2; return 1;;
    esac
    bundle=${PORTS_BUNDLE:-https://github.com/brant92good/port-forward-tui/releases/download/v$version/ports-$target.tar.gz}
    expected=${PORTS_SHA256:-}
    mkdir -p "$install_root"
    stage=$(mktemp -d "$install_root/.install.XXXXXXXX")
    trap 'case "$stage" in "$install_root"/.install.*) rm -rf -- "$stage";; esac' 0
    case "$bundle" in
        https://*) download "$bundle" "$stage/archive.tar.gz"; if [ -z "$expected" ]; then download "$bundle.sha256" "$stage/checksum"; expected=$(cut -d ' ' -f 1 "$stage/checksum" | tr -d '\r\n'); fi;;
        *) if [ -f "$bundle" ] && [ -n "$expected" ]; then cp "$bundle" "$stage/archive.tar.gz"; else printf '%s\n' 'Use an HTTPS bundle URL, or a local file with PORTS_SHA256.' >&2; return 1; fi;;
    esac
    case "$expected" in ''|*[!a-fA-F0-9]*) printf '%s\n' 'Invalid release checksum.' >&2; return 1;; esac
    [ "${#expected}" -eq 64 ] || { printf '%s\n' 'Invalid release checksum length.' >&2; return 1; }
    actual=$(checksum "$stage/archive.tar.gz")
    [ "$actual" = "$(printf '%s' "$expected" | tr 'A-F' 'a-f')" ] || { printf '%s\n' 'Download checksum mismatch. Existing installation preserved.' >&2; return 1; }
    command -v tar >/dev/null 2>&1 || { printf '%s\n' 'Install tar and try again.' >&2; return 1; }
    tar -tzf "$stage/archive.tar.gz" > "$stage/entries"
    [ "$(wc -l < "$stage/entries" | tr -d ' ')" = 2 ] || { printf '%s\n' 'Unexpected archive entries.' >&2; return 1; }
    while IFS= read -r entry; do
        case "$entry" in ports|SHA256SUMS) ;; *) printf '%s\n' 'Unexpected archive path.' >&2; return 1;; esac
    done < "$stage/entries"
    tar -xzf "$stage/archive.tar.gz" -C "$stage" ports SHA256SUMS
    [ -f "$stage/ports" ] && [ ! -L "$stage/ports" ] && [ -f "$stage/SHA256SUMS" ] && [ ! -L "$stage/SHA256SUMS" ] || { printf '%s\n' 'Expected regular binary and checksum files.' >&2; return 1; }
    internal=$(awk '$2 == "ports" {print $1}' "$stage/SHA256SUMS")
    [ "$internal" = "$(checksum "$stage/ports")" ] || { printf '%s\n' 'Bundled binary checksum mismatch.' >&2; return 1; }
    chmod 755 "$stage/ports"
    [ "$("$stage/ports" --version)" = "ports $version" ] || { printf '%s\n' 'The downloaded app could not run or has the wrong version.' >&2; return 1; }
    mkdir -p "$install_root/bin"
    if [ -f "$install_root/bin/ports" ]; then cp -p "$install_root/bin/ports" "$install_root/bin/ports.previous"; fi
    mv -f "$stage/ports" "$install_root/bin/ports"
    printf '%s\n' port-forward-tui > "$install_root/.ports-installer"
    printf '%s\n' "$version" > "$install_root/version"
    if [ "${PORTS_NO_PATH:-0}" != 1 ]; then
        # Generate one sourceable PATH fragment; do not alter unrelated shell setup.
        quoted=$(printf '%s' "$install_root/bin" | sed "s/'/'\\\\''/g")
        printf "case :\"\${PATH-}\": in *:'%s':*) ;; *) export PATH='%s':\"\${PATH-}\";; esac\n" "$quoted" "$quoted" > "$install_root/env"
        quoted_root=$(printf '%s' "$install_root" | sed "s/'/'\\\\''/g")
        line=". '$quoted_root/env' # ports"
        case "${SHELL:-/bin/sh}" in
            */zsh) profile="${ZDOTDIR:-$HOME}/.zshrc";;
            */bash)
                # Login Bash reads only the first existing login profile, while
                # interactive non-login Bash reads .bashrc. Cover both without
                # creating .bash_profile and hiding an existing .profile.
                add_path_line "$HOME/.bashrc"
                profile="$HOME/.profile"
                for candidate_profile in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
                    if [ -r "$candidate_profile" ]; then profile=$candidate_profile; break; fi
                done;;
            */fish) profile=''; mkdir -p "$HOME/.config/fish/conf.d"; printf "fish_add_path '%s'\n" "$quoted" > "$HOME/.config/fish/conf.d/ports.fish";;
            *) profile="$HOME/.profile";;
        esac
        if [ -n "$profile" ]; then add_path_line "$profile"; fi
    fi
    printf '\n%s\n' "Installed Ports $version. Run ports."
    printf 'Command: %s/bin/ports\n' "$install_root"
    printf '%s\n' 'Open a new terminal if the command is not found. Press I to import hosts or A to add one.'
}
main "$@"
