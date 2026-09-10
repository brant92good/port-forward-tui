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
    version=${PORTS_VERSION:-0.9.0}
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
    count=$(wc -l < "$stage/entries" | tr -d ' ')
    legacy=$(printf '%s\n' "$version" | awk -F '[.-]' '{ if ($1 ~ /^[0-9]+$/ && $2 ~ /^[0-9]+$/ && $3 ~ /^[0-9]+$/ && $1+0 == 0 && ($2+0 < 8 || ($2+0 == 8 && $3+0 <= 1))) print 1; else print 0; }')
    files='ports'
    if [ "$count" = 4 ]; then files='ports LICENSE.txt THIRD_PARTY_NOTICES.txt'
    elif [ "$count" = 2 ] && [ "$legacy" = 1 ]; then :
    else printf '%s\n' 'Unexpected archive entries or missing required license notices.' >&2; return 1; fi
    [ "$(sort -u "$stage/entries" | wc -l | tr -d ' ')" = "$count" ] || { printf '%s\n' 'Duplicate archive entries.' >&2; return 1; }
    while IFS= read -r entry; do
        case "$entry" in ports|SHA256SUMS|LICENSE.txt|THIRD_PARTY_NOTICES.txt) ;; *) printf '%s\n' 'Unexpected archive path.' >&2; return 1;; esac
    done < "$stage/entries"
    # files is a fixed allowlist, never archive-supplied shell text.
    tar -xzf "$stage/archive.tar.gz" -C "$stage" $files SHA256SUMS
    for file in $files SHA256SUMS; do
        [ -f "$stage/$file" ] && [ ! -L "$stage/$file" ] || { printf '%s\n' 'Expected regular bundle files.' >&2; return 1; }
    done
    awk -v count="$((count - 1))" 'NF != 2 || length($1) != 64 || $1 ~ /[^a-f0-9]/ || ($2 != "ports" && $2 != "LICENSE.txt" && $2 != "THIRD_PARTY_NOTICES.txt") || seen[$2]++ { exit 1 } END { if (NR != count) exit 1 }' "$stage/SHA256SUMS" || { printf '%s\n' 'Invalid bundled checksum index.' >&2; return 1; }
    for file in $files; do
        internal=$(awk -v name="$file" '$2 == name {print $1}' "$stage/SHA256SUMS")
        [ "$internal" = "$(checksum "$stage/$file")" ] || { printf '%s\n' 'Bundled file checksum mismatch.' >&2; return 1; }
    done
    chmod 755 "$stage/ports"
    [ "$("$stage/ports" --version)" = "ports $version" ] || { printf '%s\n' 'The downloaded app could not run or has the wrong version.' >&2; return 1; }
    mkdir -p "$install_root/bin"
    mkdir "$stage/previous"
    for file in $files; do
        if [ -e "$install_root/bin/$file" ] || [ -L "$install_root/bin/$file" ]; then
            [ -f "$install_root/bin/$file" ] && [ ! -L "$install_root/bin/$file" ] || { printf '%s\n' 'An owned install path is not a regular file.' >&2; return 1; }
            cp -p "$install_root/bin/$file" "$stage/previous/$file"
        fi
    done
    if [ -f "$install_root/bin/ports" ]; then cp -p "$install_root/bin/ports" "$install_root/bin/ports.previous"; fi
    changed=''
    for file in $files; do
        if ! mv -f "$stage/$file" "$install_root/bin/$file"; then
            for previous in $changed; do
                if [ -f "$stage/previous/$previous" ]; then mv -f "$stage/previous/$previous" "$install_root/bin/$previous"
                else rm -f "$install_root/bin/$previous"; fi
            done
            return 1
        fi
        changed="$file $changed"
    done
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
