# Experimental voice forwarding (Windows to Linux)

Keep your microphone on your desk while a voice-capable program runs in an
existing Linux SSH session. Forward microphone input over SSH and play its reply
on your Windows output device. The main view shows **Audio: NOT CONFIGURED [V]**
until configured, then **Audio: OFF [V]** until you start. Press **V** on that machine in Ports, **Enter** to
start, and **S** to stop. The remote application still owns its voice session.

Audio is optional and **disabled by default**. Ordinary installation, opening
Ports, saved port auto-start, and network recovery never start a microphone.
Closing the Ports view does not explicitly stop audio. A terminal or agent host
may end its child job when closed; audio respects that boundary. Use **S** in
the audio panel or `ports audio stop` and check restoration before closing.

## Availability

The compiled **0.10.0-beta.4 Windows prerelease** includes the experimental client.
It is not in stable 0.9.1 or the default Workspace 0.10.0 bundle. Install the beta
without changing the stable command or data:

```powershell
$installer = Join-Path $env:TEMP 'ports-beta-0.10.0-beta.4-install.ps1'
Invoke-WebRequest -UseBasicParsing https://raw.githubusercontent.com/brant92good/port-forward-tui/v0.10.0-beta.4/install.ps1 -OutFile $installer
powershell -NoProfile -ExecutionPolicy Bypass -File $installer -Channel beta -Version 0.10.0-beta.4
$Ports = Join-Path $env:LOCALAPPDATA 'Programs\PortsBeta\bin\ports-beta.exe'
& $Ports
```

No Python, Cargo or Git is needed. Add or import an SSH machine in the beta before
configuring audio. Its separate data directory keeps the stable installation intact.
If an old instance owns a local port, stop that old forward before starting its
beta counterpart; only one installation should auto-open that mapping.

**The compatible Linux bridge must already be prepared.** This repository does
not distribute that endpoint or a complete remote voice setup. This experimental
release packages the Windows client; it does not install a server or qualify any
particular coding agent's voice mode. Linux/macOS Ports still supports ordinary
forwards and proxies; microphone capture is not implemented on those clients.

## Enable on Windows

This experimental feature needs system OpenSSH, FFmpeg with DirectShow, FFplay, and the
compatible user-session Linux `ssh-voice-bridge` endpoint already prepared.
FFmpeg is an external optional dependency; the Ports controller is compiled Rust.
No Windows Python or Rust toolchain is required to run the compiled app.

Enumerate capture devices without recording:

```powershell
ffmpeg -hide_banner -list_devices true -f dshow -i dummy
```

The enumeration normally exits nonzero. Choose your physical microphone,
not a virtual loopback device. Configure explicit paths and a saved machine:

```powershell
# Saved machine ID, name or SSH target in the preview:
$Machine = 'workbox'
# Use the exact output of `ssh workbox hostname`, not the SSH alias:
$ExpectedHost = 'linux-workstation'
$FFmpeg = (Get-Command ffmpeg.exe -ErrorAction Stop).Source
$FFplay = (Get-Command ffplay.exe -ErrorAction Stop).Source
& $Ports --machine $Machine audio configure `
  --microphone 'Microphone (USB Audio)' `
  --remote-script /home/dev/ssh-voice-bridge/bridge.py --expected-host $ExpectedHost `
  --ffmpeg $FFmpeg --ffplay $FFplay --json
```

Configuration does not open the microphone. FFmpeg/FFplay are optional external
dependencies; install them separately. Select headphones as Windows' default
playback device before starting.

```powershell
& $Ports --machine $Machine audio status --json
& $Ports --machine $Machine audio start
& $Ports --machine $Machine audio stop
& $Ports --machine $Machine audio disable
```

Prefer the helper script? [install-audio.ps1](../install-audio.ps1) is a separate,
optional source download, not a file in the binary ZIP. Download the exact release
version; without `-EnableAudio` it does nothing:

```powershell
$audioSetup = Join-Path $env:TEMP 'ports-audio-0.10.0-beta.4.ps1'
Invoke-WebRequest -UseBasicParsing https://raw.githubusercontent.com/brant92good/port-forward-tui/v0.10.0-beta.4/install-audio.ps1 -OutFile $audioSetup
powershell -NoProfile -ExecutionPolicy Bypass -File $audioSetup -EnableAudio -Ports $Ports -Machine $Machine `
  -Microphone 'Microphone (USB Audio)' `
  -RemoteScript /home/dev/ssh-voice-bridge/bridge.py -ExpectedHost $ExpectedHost `
  -FFmpeg $FFmpeg -FFplay $FFplay
```

It reuses the specified dependencies, writes the same device-local configuration,
and never downloads a media package, deploys a remote server or starts capture.

For a separately installed preview, substitute its full executable path and
`--data-dir` where appropriate. Configuration is device-local, per machine.
Start the bridge before the remote app opens its audio devices. If an app cached
an older device, select the forwarded device or reopen that app's voice session.
The remote app still handles speech recognition and replies.

## Transport and stopping

Both directions use signed 16-bit little-endian PCM at 48 kHz, mono. Native
binary pipe handles connect FFmpeg → SSH → FFplay. PowerShell only starts Ports;
audio never passes through its text pipeline. Diagnostics use separate bounded
logs; microphone samples and reply samples are not saved.

Stop closes microphone capture first, allows the Linux bridge to restore its
previous default source/sink, then closes owned SSH and playback processes.
Windows jobs contain all media processes and SSH ProxyCommand descendants.
If the worker crashes, those processes are killed; if the network fails,
remote restoration may remain unconfirmed until connectivity returns. The
panel reports that uncertainty. There is no automatic recording retry.

`streaming` means the bridge reported ready and local processes remain alive.
It does **not** prove that a microphone signal reached the application or that
the listener heard a reply. Physical Windows and actual `/voice` acceptance
are separate from Linux synthetic tests. Linux/macOS capture clients are not
implemented in this preview; ordinary port forwarding remains cross-platform.

## What was checked

On September 28, 2026, the installed **0.10.0-beta.2** transport sent physical
Windows microphone input as nonzero PCM samples
through the installed SSH transport to Ubuntu. A generated spoken reply was
played into the Linux reply sink; the Windows playback process received the
stream without reporting an error. Stop and restart completed, all owned audio
processes exited, and Linux's original default source and sink were restored.
This establishes transport behavior on that setup; it does not establish a
listener's subjective playback experience or compatibility with a particular
agent's voice mode.

That beta.2 native process suite covers binary bytes, duplicate starts, stopping during
startup and child cleanup after failure. In **beta.3**, hidden terminal tests using the installed executable cover the
visible OFF/disabled indicator, V controls and leaving the panel without capture.
[Audio lifecycle tests](../tests/native_audio.rs) |
[Terminal input tests](../tests/native_pty.rs).

### Windows Terminal Workspace

The default Workspace bundle keeps its published Ports version. Developers
using the optional integration source can select this preview explicitly with
`ports-preview.json`; its Ports menu, shortcuts and workspace tab then use the
same executable and isolated data. See the
[integration contract](https://github.com/brant92good/terminal-workspace/blob/feat/audio-preview-integration/docs/ports-audio-preview.md).
