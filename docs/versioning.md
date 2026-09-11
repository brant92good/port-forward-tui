# Versions, channels and safe beta installation

The ordinary `ports` installation stays on **0.9.1**. The SOCKS candidate is
**0.10.0-beta.1** and is selected explicitly. Neither installer asks GitHub for
`latest`, and a beta is never an implicit update to stable.

| Item | Purpose |
| --- | --- |
| Commit | Exact source snapshot |
| Branch | Work that is still changing and being reviewed |
| Version | A product identity such as `0.9.1` or `0.10.0-beta.1` |
| Tag | Immutable `v` plus that exact version, pointing to qualified source |
| Release | Checked binary assets attached once to that tag |
| Channel | Explicit stable or beta installation and compatibility policy |

Use `MAJOR.MINOR.PATCH` for stable versions and `MAJOR.MINOR.PATCH-beta.N` for
beta, where N starts at 1. A failed tagged candidate gets a new version. Never
move a tag or replace release assets. GitHub's prerelease/latest flags are
separate metadata; they do not override the installer's exact default version.

## Install beta explicitly

Use these commands only after the source/tag and actual HTTPS qualification
jobs for this exact beta are green. Normal installs require no Python or Cargo.

Windows PowerShell 5.1 or 7:

```powershell
$installer = Join-Path $env:TEMP 'ports-beta-0.10.0-beta.1-install.ps1'
Invoke-WebRequest -UseBasicParsing https://raw.githubusercontent.com/brant92good/port-forward-tui/v0.10.0-beta.1/install.ps1 -OutFile $installer
& $installer -Channel beta -Version 0.10.0-beta.1
& "$env:LOCALAPPDATA\Programs\PortsBeta\bin\ports-beta.exe" --version
& "$env:LOCALAPPDATA\Programs\PortsBeta\bin\ports-beta.exe"
```

Linux or macOS **beta**:

```sh
curl -fsSL https://raw.githubusercontent.com/brant92good/port-forward-tui/v0.10.0-beta.1/install.sh |
  PORTS_CHANNEL=beta PORTS_VERSION=0.10.0-beta.1 sh
"${XDG_DATA_HOME:-$HOME/.local/share}/ports-beta-install/bin/ports-beta" --version
"${XDG_DATA_HOME:-$HOME/.local/share}/ports-beta-install/bin/ports-beta"
```

The displayed full path is the beta command; it is intentionally not added to
PATH. Archives retain the `ports` payload name for checksum compatibility; the
installer renames only the installed executable to `ports-beta`. The Windows
helpers remain inside the isolated beta bin directory. An optional custom
`-InstallDir` or `PORTS_INSTALL_DIR` must be a separate empty or beta-owned
directory. Beta refuses stable-owned destinations and symbolic-link/junction
aliases. Supplying a beta version without its beta channel is an error.

Stable installer calls still default to `0.9.1` and the original paths:
`%LOCALAPPDATA%\Programs\Ports` on Windows and
`${XDG_DATA_HOME:-$HOME/.local/share}/ports-install` on Unix. Their existing
opt-out PATH behavior is retained. Beta's ownership marker differs from stable's,
so neither channel can overwrite the other's owned directory.

Installation does not import favorites or start controllers. The beta runtime
owns its separate data marker and protocol-2 controller namespace. Do not reuse
stable `--data-dir` paths, endpoints or lock files. Use the beta's explicit
metadata-only import into a new beta directory when its independent tests are
qualified. This writes the beta copy; the stable source remains unchanged.

## Qualification and immutable records

1. Review the branch, run source tests and installer fixtures on all five targets.
   Cargo.toml and Cargo.lock must agree. A tag must exactly equal `v` plus Cargo's
   version, and the corresponding release notes must exist.
2. Run native unit/terminal tests, formatting, Clippy, notices, package and
   fresh/update installer checks. Windows beta runs protocol-rejection checks;
   it does not claim historical-controller interoperability. Linux runs real
   OpenSSH local and SOCKS tests. Keep platform and input-path limits explicit.
3. Create the immutable beta tag only after source qualification. The tag workflow
   reruns its required matrix, validates every archive/sidecar, and freezes
   `release-record.json` with source commit, version/channel, hashes, test matrix
   and compatibility policy. The aggregate `SHA256SUMS` also hashes that record.
4. Publish once with `--verify-tag --prerelease --latest=false`. The workflow uses
   `gh release create`, with no upload-clobber or edit path. Existing releases
   cause creation to fail rather than replacing bytes. See the official
   [GitHub CLI release creation contract](https://cli.github.com/manual/gh_release_create).
5. Download through the versioned public installer and test actual HTTPS
   fresh/update, checksum refusal, data preservation and channel isolation.
   These later receipts are separate workflow artifacts; the already-published
   release record records that they were not yet observed at its creation.
6. Independent review covers actual results, not only workflow labels. Record
   publication and then the owner's isolated installation separately in the
   existing private request ledger. The release record is evidence for that
   ledger, not another task tracker.

Source receipts bind the commit actually checked out, including a PR merge
commit when appropriate. A source-CI artifact is not an installation claim, and
a prerelease upload is not proof that the subsequent HTTPS tests passed.

## Rollback and stable promotion

To return to stable use the existing `ports` command/path and its original data;
no stable reinstall is needed. Stop only beta-owned forwards when desired using
the beta's exact machine IDs. Removing beta binaries must not remove either
channel's saved data, and must not kill a stable controller.

An explicit earlier beta version can be reinstalled into the beta-owned directory
after checking that version's schema compatibility. Installer rollback tests use
compatible inert fixtures; they do not prove every future data downgrade safe.
Do not copy the beta's sticky schema-2 files over stable favorites. Keeping stable
data unchanged is the reliable fallback.

Promoting a feature to stable needs an explicit reviewed compatibility/import
policy, exact release evidence and a recorded owner decision. It cannot be done
by removing `-beta.1` from a filename or pointing an unversioned download at it.

On September 12 the already-qualified baseline labels were reconciled: Ports
`v0.9.1` and Workspace `v0.10.0` are stable/latest. Their tag targets, asset
identities and hashes were preserved, and Workspace's obsolete “prerelease”
title was corrected. This did not promote the SOCKS beta. Future baseline-label
changes require the same exact identity comparison and independent review;
the beta workflow never performs them automatically.
