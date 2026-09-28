# Audio forwarding (Windows preview)

Use your Windows microphone and headphones with a voice-capable program in an
existing Linux SSH session. Press **V** on that machine in Ports, **Enter** to
start, and **S** to stop. The remote application still owns its voice session.

Audio is optional and **disabled by default**. Ordinary installation, opening
Ports, saved port auto-start, and network recovery never start a microphone.
Closing the Ports view does not explicitly stop audio. A terminal or agent host
may end its child job when closed; audio respects that boundary. Use **S** in
the audio panel or `ports audio stop` and check restoration before closing.

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
.\install-audio.ps1 -EnableAudio -Ports C:\Tools\Ports\ports.exe `
  -Machine workbox -Microphone 'Microphone (USB Audio)' `
  -RemoteScript /home/dev/ssh-voice-bridge/bridge.py -ExpectedHost workbox
```

Without `-EnableAudio` this installer is a no-op. It reuses FFmpeg/FFplay already
on PATH (or accepts `-FFmpeg` / `-FFplay` paths); it does not silently download a
media package or deploy a remote server. Select headphones as Windows' default
playback device before starting.

```powershell
ports --machine workbox audio status --json
ports --machine workbox audio start
ports --machine workbox audio stop
ports --machine workbox audio disable
```

For a separately installed preview, substitute its full executable path and
`--data-dir` where appropriate. Configuration is device-local, per machine.
Start the bridge before opening `/voice`; reopen voice if it cached an older
audio device. No separate API chatbot or conversation is created.

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
