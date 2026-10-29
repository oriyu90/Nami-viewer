# NamiVision 0.1.0

NamiVision is a Windows desktop tool for inspecting an audio file as a DAW-style
waveform plus up to four frequency-band waveforms, and for measuring **how far each
band's onset drifts from a musical beat grid** derived from BPM, meter and bar start.
Results are exported as CSV. It doubles as a simple music player.

- Rust core, HTML/CSS/Canvas UI on WebView2
- Portable: a single executable, no installer
- Japanese / English, dark / light theme, UI scale 80–150%

**Author:** Yuki_Orita (折田悠希 / おりたゆうき) ·
**Repository:** https://github.com/oriyu90/Nami-viewer ·
**Website:** https://studio-rizi.pages.dev/projects/nami-viewer/

---

## 1. Requirements

| Item | Requirement |
|---|---|
| OS | Windows 11 / Windows 10 22H2 or later (64-bit) |
| Runtime | Microsoft Edge WebView2 Runtime (included with Windows 11; normally present on Windows 10 via Edge updates) |
| Disk | ~30 MB |
| Install | None — unzip and run |

If WebView2 is missing, NamiVision shows instructions at startup.
Install the Evergreen Bootstrapper from
[Microsoft Edge WebView2](https://developer.microsoft.com/microsoft-edge/webview2/).

---

## 2. Getting started

1. Unzip anywhere (read-only locations such as `Program Files` are fine).
2. Run `NamiVision.exe`.
3. Drop an audio file (**mp3 / wav / flac / ogg**, **5 seconds or longer**) onto the
   window, or use **Open** in the header.
4. `Space` plays/pauses, `S` stops and returns to the start.

### Layout

```
┌──────────────────────────────────────────────────────────────┐
│ NamiVision   song.flac 03:33      [Open] [Project] [Settings]│
├──────────────────────────────────────────────────────────────┤
│ ▶ ■  ──────●────────────────  00:12.34 / 03:33.45  🔊 128BPM│
├──────────────────────────────────────────────────────────────┤
│ 0:00      0:05      0:10      0:15        ← time ruler       │
│ MASTER ▁▂▅█▇▅▂▁  (full waveform + bar/beat grid)            │
│ LOW    ▁▁▃█▂▁▁   (20–250 Hz)                                 │
│ LOW-MID ▁▂▁▅▃▁   (250–1,000 Hz)                              │
│ MID-HIGH ▁▁▂▁▃▁  (1,000–4,000 Hz)                            │
│ HIGH   ▁▂▁▁▂▁▁   (4,000–16,000 Hz)                           │
├──────────────────────────────────────────────────────────────┤
│ 44.1 kHz · stereo · 213.45 s · zoom 96 px/s                  │
└──────────────────────────────────────────────────────────────┘
```

---

## 3. Tempo setup (BPM, meter, bar start)

Right-click the **MASTER** lane → **Tempo settings…**

- **BPM** 20–400. Type a value or tap `Tap` four or more times.
- **Meter** 2 / 3 / 4 / 6 / 8 beats per bar.
- **Bar start** in seconds, or click **Use playhead position**.
- **Show grid** draws bar, beat and subdivision lines.
- **Snap** makes the bar-start flag snap to beats while dragging.

The ruler then shows bar numbers and beats, and an amber **"1" flag** marks the bar start.
Drag that flag inside the lane for 1 ms adjustments. You can also right-click the MASTER
lane → **Set bar start here**.

---

## 4. Exporting beat deviations as CSV

1. Set the tempo first (BPM, meter, bar start).
2. Right-click a **band lane** (second row or lower).
3. Choose **Export beat deviations as CSV**.
   - While the tempo is unset this item is disabled.
4. Pick a destination.

### How the deviation is computed

For beat index `k` (0 = bar start):

```
T_k = barStart + k × (60 / BPM)
```

The band's onset strength is searched within `±0.45 beat` of `T_k`; the maximum is
refined by parabolic interpolation to `t_onset`.

```
deviation_ms    = (t_onset − T_k) × 1000
deviation_beats = (t_onset − T_k) / (60 / BPM)
```

- **Positive = later than the beat**
- **Negative = earlier than the beat**
- Beats without a clear onset are marked `undetected` (optionally omitted).

### CSV columns

| Column | Meaning |
|---|---|
| `band_index` | 1-based band number |
| `band_label` | `Low` / `Low-Mid` / `Mid-High` / `High` |
| `frequency_range_hz` | e.g. `20-250` |
| `beat_index` | beat number with 0 at bar start (may be negative) |
| `bar` | bar number (1 at bar start) |
| `beat_in_bar` | 1-based beat within the bar |
| `expected_time_s` | nominal beat time (s) |
| `onset_time_s` | detected onset time (s) |
| `deviation_ms` | deviation in ms (positive = late) |
| `deviation_beats` | deviation in beats |
| `onset_strength` | onset strength 0–1 |
| `status` | `detected` / `undetected` |

CSV is written as **UTF-8 with BOM and CRLF** so Excel opens it directly.
Delimiter and decimal places are configurable.

---

## 5. Projects (JSON)

Use **Project → Save / Load**.

Stored: audio path, name and duration; tempo; zoom/scroll/selected lane/follow; band
edges and colors; playhead and volume; language, theme and UI scale.

Audio is not embedded. If the file is missing on load, all settings are restored and a
**Relink audio** banner appears; relinking keeps your tempo setup.

---

## 6. Settings

Settings apply immediately and are saved automatically.

| Section | Options |
|---|---|
| General | Language, theme, UI scale (80–150%) |
| Playback | Volume, waveform gain, follow playhead |
| Analysis | FFT size (1024/2048/4096), band count (1–4), band edges, onset sensitivity |
| Output | CSV delimiter (`,` / `;`), decimals (0–6), include undetected rows |
| About | Version, data folder, portable state |

Storage:

- Default: `%APPDATA%\com.namivision.app\settings.json`
- Portable: place `portable.txt` next to the exe, or launch with `--portable`, to use
  `<exe folder>\data\settings.json`

---

## 7. Shortcuts

| Key | Action |
|---|---|
| `Space` | Play / pause |
| `S` | Stop (return to start) |
| `←` / `→` | Seek 1 s (`Shift` = 5 s) |
| `Home` / `End` | Seek to start / end |
| `+` / `-` | Zoom in / out |
| `Ctrl` + wheel | Zoom around the cursor |
| Wheel | Horizontal scroll |
| `Esc` | Close menu / dialog |

---

## 8. Troubleshooting

| Symptom | Fix |
|---|---|
| WebView2 message at startup | Install the [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) |
| "Audio must be at least 5 seconds" | Files shorter than 5 s are not analyzed |
| No sound | Check the Windows output device. Analysis and CSV still work |
| CSV export is disabled | Set BPM and meter from the MASTER lane's context menu first |
| Missing audio | Use **Relink audio** after loading the project |
| Reset settings | Delete `settings.json` in the folder shown under Settings → About |

---

## 9. Building from source

Requires Rust 1.77+ and MSVC Build Tools. Node.js is not needed.

```powershell
cargo build --release
# target\release\nami-vision.exe
```

Headless self-test:

```powershell
cargo run --release -- --selftest --selftest-dir .\selftest
```

Tests:

```powershell
cargo test
node --test tests/frontend
```

---

## 10. License

MIT License (`LICENSE.txt`). See `THIRD_PARTY_NOTICES.md` for the main open-source
components.
