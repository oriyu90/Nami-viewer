# Third-party notices

NamiVision is built with the following open-source components. This list covers the
main direct dependencies; transitive dependencies are distributed under their own
licenses (MIT / Apache-2.0 / BSD / ISC / MPL-2.0).

| Component | Version | License | Use |
|---|---|---|---|
| Tauri (`tauri`, `tauri-build`) | 2.x | MIT OR Apache-2.0 | Application shell, IPC, asset embedding |
| wry / tao (via Tauri) | 0.5x / 0.3x | MIT OR Apache-2.0 | WebView2 window integration |
| Symphonia (`symphonia`) | 0.5 | MPL-2.0 | MP3 / WAV / FLAC / Ogg Vorbis decoding |
| rustfft | 6.x | MIT OR Apache-2.0 | FFT for spectral analysis |
| cpal | 0.16 | Apache-2.0 | WASAPI audio output |
| rfd | 0.15 | MIT | Native file dialogs |
| dirs | 6.x | MIT OR Apache-2.0 | Standard data directory resolution |
| serde / serde_json | 1.x | MIT OR Apache-2.0 | JSON serialization |
| windows-sys | 0.59 | MIT OR Apache-2.0 | WebView2 presence check, message box |

## Microsoft WebView2

NamiVision uses the Microsoft Edge WebView2 Runtime, which is distributed by Microsoft
under its own terms and is not bundled with this application. Windows 11 includes it by
default; on Windows 10 it is normally installed through Microsoft Edge updates.

<https://developer.microsoft.com/microsoft-edge/webview2/>

## Fonts

No web fonts are bundled or downloaded. The UI uses the operating system's Segoe UI
Variable / Segoe UI and Cascadia Mono / Consolas fonts.

## Source code of MPL-2.0 components

Symphonia is used unmodified as a Cargo dependency. Its source is available at
<https://github.com/pdeljanov/Symphonia> and via crates.io. If you redistribute a
modified version of Symphonia, the MPL-2.0 requires you to make that modified source
available.
