# Audio forwarding (Windows-to-Linux preview)

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

Audio is on the `feat/audio-beta` source branch (0.10.0-beta.3), not in the
published 0.9.1 download or the Workspace 0.10.0 bundle. Normal Ports installation
still downloads a compiled app; trying this audio source currently needs Git
and a Rust toolchain:

```powershell
git clone --branch feat/audio-beta https://github.com/brant92good/port-forward-tui.git ports-audio
cd ports-audio
cargo build --locked --release
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build_native.ps1 -SkipRust -OutputDirectory target/release
$Ports = (Resolve-Path .\target\release\ports.exe).Path
& $Ports
```

The Windows helper build above is needed for returning to existing Terminal tabs.
Add or import an SSH machine in this isolated preview before configuring audio.
Its separate beta data directory keeps the existing Ports installation intact.
If an old instance owns a local port, stop that old forward before starting its
preview counterpart; only one installation should auto-open that mapping.

**The compatible Linux bridge must already be prepared.** This repository does
not yet distribute that endpoint or a complete voice installer. A versioned
optional audio package remains release work.

## Enable on Windows

This preview needs system OpenSSH, FFmpeg with DirectShow, FFplay, and the
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
.\install-audio.ps1 -EnableAudio -Ports $Ports `
  -Machine $Machine -Microphone 'Microphone (USB Audio)' `
  -RemoteScript /home/dev/ssh-voice-bridge/bridge.py -ExpectedHost $ExpectedHost
```

Without `-EnableAudio` this installer is a no-op. It reuses FFmpeg/FFplay already
on PATH (or accepts `-FFmpeg` / `-FFplay` paths); it does not silently download a
media package or deploy a remote server. Select headphones as Windows' default
playback device before starting.

```powershell
& $Ports --machine $Machine audio status --json
& $Ports --machine $Machine audio start
& $Ports --machine $Machine audio stop
& $Ports --machine $Machine audio disable
```

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
