# Third-party notices

ClearMic is built on open-source components. Their licenses permit commercial use and redistribution.

| Component | Purpose | License |
|---|---|---|
| [DeepFilterNet](https://github.com/Rikorose/DeepFilterNet) (model weights and `libDF`) | Neural noise suppression | MIT / Apache-2.0 |
| [tract](https://github.com/sonos/tract) | Neural network inference on the CPU | MIT / Apache-2.0 |
| [cpal](https://github.com/RustAudio/cpal) | Audio capture and playback (WASAPI) | Apache-2.0 |
| [rubato](https://github.com/HEnquist/rubato) | Sample-rate conversion | MIT |
| [egui / eframe](https://github.com/emilk/egui) | User interface | MIT / Apache-2.0 |
| [tray-icon](https://github.com/tauri-apps/tray-icon), [global-hotkey](https://github.com/tauri-apps/global-hotkey) | Tray icon and keyboard shortcut | MIT / Apache-2.0 |
| [ringbuf](https://github.com/agerasev/ringbuf), [hound](https://github.com/ruuda/hound), [serde](https://serde.rs) | Buffers, WAV files, settings | MIT / Apache-2.0 |

DeepFilterNet citation: Schroeter, Rosenkranz, Escalante-B., Maier. "DeepFilterNet: Perceptually Motivated
Real-Time Speech Enhancement", INTERSPEECH 2023.

## VB-CABLE virtual audio driver

The ClearMic installer includes the unmodified VB-CABLE driver package from VB-Audio Software
and can install it for you. ClearMic uses it as the virtual microphone.

- Origin: VB-Audio Software, https://www.vb-cable.com
- VB-CABLE is donationware. All participations are welcome: if you find it useful, please
  support its authors at https://vb-audio.com/Services/licensing.htm
- Its own readme and licence are installed in the `vbcable` folder next to `clearmic.exe`.

VB-Audio permits distributing VB-CABLE with another application, including silent installation,
as long as end users can see that it is VB-CABLE by VB-Audio and are able to donate. When ClearMic
is rolled out inside a company or institution where employees are not in a position to do that,
the organisation has to buy VB-CABLE licences for its seats from the VB-Audio webshop.
