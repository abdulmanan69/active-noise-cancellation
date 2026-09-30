# ClearMic

Lightweight, accurate AI noise cancellation for any PC microphone. ClearMic removes keyboards, fans,
traffic, dogs and room noise from your microphone in real time and hands the clean voice to Zoom, Teams,
Meet, Discord or any other app. It works like Krisp, runs fully on your own computer and needs no account,
no cloud and no GPU.

- **Clear speech.** DeepFilterNet3, a full-band 48 kHz neural model, keeps consonants and natural tone
  instead of the muffled sound of classic noise gates.
- **Light.** One executable of about 30 MB and about 100 MB of RAM. Around 13 percent of one CPU core
  on a 2018 desktop Core i5, which is about 2 percent of the whole processor.
- **Private.** Audio never leaves the machine. No telemetry, no network connections.
- **One install.** The installer sets up the app and the virtual microphone driver together.
- **Robust.** Automatic recovery when a headset is unplugged, any sample rate, any channel count.
- **Manageable.** Silent installer, plain JSON settings, log file, headless mode and a command line.

## How it works

```
microphone -> [ low-cut -> DeepFilterNet3 -> voice gate -> limiter ] -> virtual microphone -> your meeting app
```

Windows has no built-in virtual microphone. ClearMic uses **VB-CABLE by VB-Audio**
(https://www.vb-cable.com) for that and installs it for you. VB-CABLE is donationware, all participations
are welcome: if you find it useful, please support its authors.

## Install

1. Run `ClearMic-Setup-1.0.0.exe` and approve the Windows administrator prompt. This installs ClearMic
   and the VB-CABLE virtual microphone driver in one go.
2. Restart Windows if the installer asks for it.
3. Start ClearMic. It finds your microphone and the virtual microphone automatically.
4. In your meeting app choose **CABLE Output (VB-Audio Virtual Cable)** as the microphone.

If Windows switched your speakers to "CABLE Input" after the driver was installed, ClearMic shows a
warning with a button that opens Sound settings so you can pick your real speakers again.

ClearMic also runs as a single `clearmic.exe` without the installer. It then offers an
"Install virtual microphone" button when the `vbcable` folder is next to the executable. Without a virtual
microphone the cleaned audio plays on your speakers, which is useful for a quick listen only.

## Using the app

| Control | What it does |
|---|---|
| Big switch | Turns noise cancellation on or off. Audio keeps flowing when off, so calls are not interrupted. |
| Strength | How much noise is removed. 85 percent is the default. 100 percent removes everything the model can. |
| Voice gate | Mutes the microphone completely between words. |
| Extra filter | Additional suppression for constant noise such as fans and air conditioning. |
| Low-cut filter | Removes rumble below 80 Hz before the model runs. Off by default, the model handles rumble. |
| Monitor | Plays the cleaned audio on your headphones so you can hear the result. |
| Buffer | Trade a little latency for stability on slow or busy machines. |

`Ctrl+Shift+M` toggles noise cancellation from anywhere. Closing the window keeps ClearMic in the system
tray. Right-click the tray icon to quit. Starting ClearMic a second time brings the running window forward.

## Measured quality

Real speech mixed with recorded background noise, cleaned with `clearmic --process` at full strength and
compared with the clean original. The score is SI-SDR in dB (higher is better; it drops when speech is
distorted, not only when noise remains).

| Noise level (input SNR) | Before ClearMic | After ClearMic |
|---|---|---|
| Very loud noise, 0 dB | 0.1 dB | 12.8 dB |
| Loud noise, 5 dB | 5.0 dB | 15.7 dB |
| Moderate noise, 10 dB | 10.0 dB | 18.1 dB |
| Light noise, 20 dB | 20.0 dB | 23.5 dB |

Clean speech without noise passes through unchanged (32.7 dB). The test suite repeats this kind of check
on a bundled speech clip, so a build that damages speech fails `cargo test`.

Processing delay is 40 ms for the model plus the device buffers, about 80 ms inside ClearMic with the
default buffer. Measured on a Core i5-8500T: 1.3 ms of processing per 10 ms of audio, no dropouts.

## Command line

```
clearmic                         start the desktop app
clearmic --minimized             start hidden in the system tray
clearmic --headless              run the engine without a window (uses saved settings)
clearmic --list-devices          print capture and render endpoints
clearmic --process IN.wav OUT.wav [--strength N] [--post-filter] [--gate] [--no-hpf]
clearmic --bench [SECONDS]       measure processing speed on this CPU
clearmic --selftest [SECONDS]    open the real devices, run muted, report health (exit code 0 = ok)
clearmic --export-icon [PATH]    write the application icon as .ico
```

ClearMic is a windowed program, so `cmd.exe` and PowerShell do not wait for it by default. Use
`start /wait clearmic --bench` in `cmd.exe` or `Start-Process clearmic -ArgumentList '--bench' -Wait -NoNewWindow`
in PowerShell when a script needs the output or the exit code.

`--process` cleans a recording offline. The output has the same length and sample rate as the input and is
aligned to it, so the two files can be compared directly.

## Deployment for IT teams

- **Silent install:**
  `ClearMic-Setup-1.0.0.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /TASKS="vbcable,autostart"`.
  Uninstall with `unins000.exe /VERYSILENT`; it offers to remove the driver it installed.
- **Driver prompt:** the first time a VB-Audio driver is installed, Windows can ask the user to trust the
  publisher. For unattended rollouts add the "VB-Audio Software" certificate to Trusted Publishers by
  group policy first.
- **VB-CABLE licence:** VB-Audio allows shipping VB-CABLE inside another installer when end users can see
  that it is VB-CABLE and can donate. For company-wide rollouts where employees cannot do that, buy
  VB-CABLE licences per seat at https://vb-audio.com/Services/licensing.htm. An installer without the
  driver is built with `.\build.ps1 -Installer -NoDriver`.
- **Settings:** `%LOCALAPPDATA%\clearmic\ClearMic\config\settings.json`. Copy a prepared file to
  pre-configure users. Unknown keys are ignored and missing keys use defaults.
- **Logs:** `clearmic.log` in the same folder, rotated at 5 MB.
- **Health check:** `clearmic --selftest` opens the real devices with the output muted and reports
  dropouts, CPU load and latency. `clearmic --bench 5` reports CPU headroom. `clearmic --list-devices`
  reports whether the virtual microphone is present.
- **Requirements:** Windows 10 or 11, 64-bit. Any x86-64 CPU from the last ten years. About 100 MB of RAM.
  A graphics driver with OpenGL 2.0 for the window; on remote desktops without one use `--headless`.

## Build from source

Requirements: Rust (GNU toolchain) and, for the installer, Inno Setup 6. Visual Studio is not needed.

```powershell
rustup toolchain install stable-x86_64-pc-windows-gnu
.\build.ps1              # release build
.\build.ps1 -Test        # run the test suite first
.\build.ps1 -Installer   # also build dist\ClearMic-Setup-1.0.0.exe (app + VB-CABLE driver)
```

`build.ps1` downloads two things once: GNU binutils from the w64devkit project, because the Rust GNU
toolchain needs `dlltool`, `as` and `gcc` to link against Windows system libraries, and the official
VB-CABLE driver pack from VB-Audio, checked against a pinned SHA-256.

The DeepFilterNet code and the `tract` inference library are pinned to the versions of the official
DeepFilterNet v0.5.6 release. Do not update them without re-running the tests and the speech measurement.

## Project layout

| Path | Contents |
|---|---|
| `src/dsp/` | Model wrapper, filters, voice gate and the per-frame pipeline |
| `src/audio/` | Device enumeration, capture and playback streams, sample-rate conversion |
| `src/engine.rs` | Real-time engine: audio thread, lock-free buffers, clock-drift control, recovery |
| `src/app/` | Window, tray icon, global shortcut and command line |
| `tests/pipeline.rs` | End-to-end signal and speech-quality tests |
| `installer/clearmic.iss` | Inno Setup script |

## Troubleshooting

- **Other people hear nothing.** Select "CABLE Output" as the microphone in the meeting app and check that
  the ClearMic output meter moves when you speak.
- **I hear nothing from my speakers after installing.** Windows made the virtual cable the default
  playback device. Pick your real speakers in Sound settings.
- **"Driver installed but not active".** Restart Windows once.
- **Robotic or choppy sound.** Raise the buffer to "Safe" in Advanced. Run `clearmic --bench` and check that
  CPU load is well below 100 percent.
- **Voice sounds thin.** Lower the strength to 70 percent and turn off the extra filter.
- **The microphone changed.** Pick it in the Microphone list or press Refresh devices.

## License

MIT for ClearMic. See `THIRD-PARTY-NOTICES.md` for the components it builds on, including VB-CABLE.
