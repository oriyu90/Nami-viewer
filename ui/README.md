# NamiVision UI

The NamiVision frontend: plain HTML, CSS and native ES modules. There is no
framework, bundler, npm dependency, external font or CDN asset, and no build
step. The Tauri v2 shell loads this directory directly as `frontendDist`.

```
ui/
├─ index.html            markup, SVG icon sprite, dialog shells
├─ favicon.svg           local favicon (avoids a network favicon request)
├─ package.json          `"type": "module"` so Node treats js/*.js as ESM
├─ styles/
│  ├─ tokens.css         colour, type, spacing and sizing tokens (dark/light)
│  └─ app.css            layout, components, canvas chrome, responsive rules
├─ js/
│  ├─ boot.js            classic synchronous script: theme/lang/scale before paint
│  ├─ core.js            DOM-free helpers: time, ticks, grid, view, CSV, defaults
│  ├─ i18n.js            ja/en dictionaries, `t`, `applyI18n`, number formatting
│  ├─ state.js           one app-state object + UI preference cache
│  ├─ canvas.js          waveform, band, ruler and grid renderer
│  ├─ menus.js           accessible context menu with viewport flipping
│  ├─ dialogs.js         tempo/settings/confirm controllers around <dialog>
│  └─ app.js             backend adapter and main controller
└─ mock/
   └─ mock-backend.js    browser-only IPC mock, synthetic 120 BPM demo track
```

## Running in a browser

ES modules and `fetch` need an HTTP origin, so `file://` is not supported.
Serve `ui/` with any static server, for example:

```powershell
cd ui
python -m http.server 8080
```

Then open <http://127.0.0.1:8080/>.

When `window.__TAURI__` is absent, `app.js` automatically imports
`mock/mock-backend.js`. The mock provides:

- `Open` → `demo_click_120bpm.wav`, a synthetic 12 s / 120 BPM click track
- deterministic four-band analysis and beat onsets
- simulated (silent) playback with a real position clock
- settings, project and last-audio persistence in `localStorage`
- project load with a deliberately missing audio path, plus relink
- CSV export as a browser download

Mock storage keys are `namivision.mock.settings`, `namivision.mock.project`,
`namivision.mock.projectPath` and `namivision.mock.lastAudio`. UI preferences
(theme, language, scale) use `namivision.ui`.

## Running in Tauri

`tauri.conf.json` points `build.frontendDist` at `ui` and enables
`withGlobalTauri`. When the bridge exists, the adapter uses:

- `window.__TAURI__.core.invoke` for IPC
- `window.__TAURI__.event.listen` for backend events
- `window.__TAURI__.webviewWindow.getCurrentWebview().onDragDropEvent`
  for native file drops

Build or run the native shell with the normal Tauri v2 workflow for this
repository. No frontend dev server or asset build is involved.

## State and persistence

`createAppState()` in `js/state.js` is the single mutable state object used by
the controller, renderer and dialogs. It holds audio metadata, analysis,
bands, tempo, view, playback, settings, selection, crossovers, project path and
dirty flags.

- UI preferences are written to `namivision.ui` and are also read by
  `boot.js` before CSS paints, so theme, language and scale do not flash.
- Settings are loaded/saved through the backend (`load_settings`,
  `save_settings`); the mock mirrors them in `localStorage`.
- Project round-trips use `save_project` / `load_project`. A loaded project
  whose audio cannot be found keeps its metadata and exposes a relink banner.
- All visible strings live in `js/i18n.js`; static markup uses `data-i18n`,
  `data-i18n-title`, `data-i18n-aria` and `data-i18n-placeholder` hooks.

## Tests

From the repository root:

```powershell
node --test tests/frontend/
```

The suite covers time formatting, tick selection, beat-grid generation
(including negative beats before a late bar start), view transforms and
clamping, CSV filename sanitisation, settings/project defaults, beat
deviation/CSV generation, and Japanese/English dictionary parity and
interpolation. `tests/frontend/index.js` is the directory entry point for
Node's test runner.

## Manual browser QA checklist

1. Empty state: no audio, disabled transport, drop zone, both themes.
2. `Open` loads `demo_click_120bpm.wav`; play/pause/stop and seek update.
3. Tempo dialog: BPM, meter, bar start, subdivision, tap tempo, grid/snap.
4. Settings: all five tabs, theme, language, scale, crossovers, output.
5. Switch ja/en and dark/light; check that no layout overflows.
6. Set 150% scale at 900 × 800; header, transport, status and dialogs fit.
7. Right-click the master lane and a band lane; check disabled items.
8. Export a band CSV while tempo is set; verify the download and BOM/CRLF.
9. Clear tempo and confirm CSV, grid and clear-tempo items are disabled.
10. Load the missing-audio project, then relink; confirm the banner clears.
11. Keep the console free of errors and warnings; confirm no failed requests.

The reference QA run was performed in Chrome 156 at 1440 × 900 and
900 × 800 with device scale factor 1.

## Known limitations

- Browser mode is a UI harness, not the product: playback is silent and the
  analysis is synthetic. It cannot exercise the Rust DSP or the audio device.
- The mock `pick_project` returns a built-in missing-audio project until a
  project has been saved; after that it loads the saved project.
- Native file pickers are replaced by the demo path; `open_path` and
  `reveal_file` are no-ops in the mock.
- At 900 × 800 with 150% scale the Help dialog scrolls internally. Controls
  remain reachable, but the full text is not visible at once.
- The header file chip truncates long file names with an ellipsis at narrow
  widths by design.
- UI scale is root-font scaling (`14px × scale`), not browser zoom; canvas
  sizing is recomputed from the resulting layout.
