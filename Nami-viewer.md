# Nami-viewer 保守メモ（開発者向け）

NamiVision の実装・リリース・紹介サイトを保守するための実務メモ。
`common rules document` のルール6に基づくプロジェクト固有ドキュメントであり、
公開 README には出さない内部情報（手順・落とし穴）をここに集約する。

- アプリ名: **NamiVision**（リポジトリ名は `Nami-viewer`）
- リポジトリ: https://github.com/oriyu90/Nami-viewer
- 紹介サイト: https://studio-rizi.pages.dev/projects/nami-viewer/
- 配布: GitHub Releases（`v0.1.0` 以降、ポータブル zip のみ）
- ライセンス: MIT / Author: `Yuki_Orita`

---

## 1. 構成

```text
Nami_vison/
├── src/                 Rust コア（Tauri 2 バックエンド）
│   ├── audio/           デコード（Symphonia）・再生（cpal）・解析
│   ├── analysis.rs      96 対数帯域 + 帯域波形の合成
│   ├── beats.rs         拍グリッド・オンセット検出・CSV 生成
│   ├── project.rs       プロジェクト JSON / 設定 JSON
│   ├── commands.rs      IPC コマンド
│   ├── selftest.rs      --selftest の統合テスト
│   └── main.rs          CLI・WebView2 起動
├── ui/                  フロントエンド（ビルド不要の ES Modules + Canvas）
│   ├── js/              core / state / menus / dialogs / i18n
│   ├── styles/          tokens.css / app.css
│   └── mock/            ブラウザ用モックバックエンド
├── tests/frontend/      node:test（UI ロジック）
├── docs/                01 技術調査・設計計画書 / 02 設計仕様書 / 03 QA レポート
├── scripts/package.ps1  ポータブル zip 生成
├── fixtures/            テスト用音源（リポジトリには含めない）
└── dist/                リリース成果物（git 管理外）
```

`ui/` はビルド工程を持たない。`tauri.conf.json` の `frontendDist: "ui"` がそのまま配信される。
`ui/mock/mock-backend.js` を使えば Tauri なしでブラウザから UI を確認できる。

---

## 2. バージョン上げチェックリスト

リリース時は次をすべて同じ値に揃える。

1. `Cargo.toml` の `version`
2. `tauri.conf.json` の `version`（`productName` は `NamiVision` のまま）
3. `README.md` / `README.ja.md` の見出し `# NamiVision x.y.z`
4. `docs/01_技術調査・設計計画書.md` P7 と `docs/02_設計仕様書.md` §9 の zip 名
5. `docs/03_QAレポート.md` の対象バージョン・テスト件数
6. 紹介サイト `website/projects/nami-viewer/{index,en/index,zh/index,pt/index}.html` の
   バージョン表記・ダウンロード URL・`NamiVision_x.y.z_windows-x64.zip` のサイズ表記
7. `website/content.js` の `projects` にある `releaseVersion` / `releaseDate`
   （初回リリース情報のみ。更新時は `manualNews` に `UPDATE` を追加する）
8. `dist/` のファイル名と GitHub Release のタグ（`vx.y.z`）

`releaseDate` は `YYYY.MM.DD`。PROJECT カードの `releaseDate` は初回リリース日のまま固定し、
更新のたびに書き換えない（`PROJECT-CARD-RULES.md`）。

---

## 3. ビルドとパッケージ

前提: Rust 1.77 以降 + MSVC Build Tools、Node.js はテストのみで使用。

```powershell
# 開発ビルド（WebView2 で起動）
cargo run

# リリース exe
cargo build --release
# target\release\nami-vision.exe

# ポータブル zip（README / docs / LICENSE を同梱）
powershell -ExecutionPolicy Bypass -File scripts\package.ps1
# dist\NamiVision_x.y.z_windows-x64.zip と .sha256 を生成
```

`package.ps1` は `cargo build --release` を実行してから staging する。
zip 内のファイル名は必ず `/` 区切り（.NET Framework の `\` 混入を避けるため手動でエントリを作成）。
`portable.txt` が同梱されているため、展開先の `data\settings.json` に設定が保存される。

### リリース手順

```powershell
# 1. テスト
cargo test
node --test tests/frontend
.\target\release\nami-vision.exe --selftest --selftest-dir .\selftest

# 2. zip を作り、ハッシュを控える
powershell -ExecutionPolicy Bypass -File scripts\package.ps1

# 3. 展開して起動確認（--selftest が PASS すること）
Expand-Archive dist\NamiVision_x.y.z_windows-x64.zip $env:TEMP\nami-check
& "$env:TEMP\nami-check\NamiVision_x.y.z_windows-x64\NamiVision.exe" --selftest --selftest-dir "$env:TEMP\nami-check\st"

# 4. タグとリリース
git tag vx.y.z; git push origin vx.y.z
gh release create vx.y.z dist\NamiVision_x.y.z_windows-x64.zip `
  --title "NamiVision x.y.z" --notes-file <notes.md>
```

アセット URL は
`https://github.com/oriyu90/Nami-viewer/releases/download/vx.y.z/NamiVision_x.y.z_windows-x64.zip`。
サイトのダウンロードリンクはこの形式で直リンクしているため、**タグ名とファイル名を変えたら
4 言語すべての HTML を更新する**。

---

## 4. テスト

| 種別 | コマンド | 目安 |
|---|---|---|
| Rust 単体・結合 | `cargo test` | 75 件 |
| フロントエンド | `node --test tests/frontend` | 33 件 |
| 統合セルフテスト | `NamiVision.exe --selftest --selftest-dir <dir>` | 12 チェック |

`--selftest` は UI を起動せず、5 秒以上のクリック音源を生成して
デコード → 帯域解析 → 拍検出 → CSV 構造 → プロジェクト JSON 往復 → 短尺拒否まで確認する。
レポートは `<dir>/selftest_report.json`。CI では使っていないため、リリース前にローカルで必ず実行する。

`fixtures/` と `shots/` は `.gitignore` 対象。テスト音源は selftest が生成する。

---

## 5. データ形式

### CSV（帯域レーン右クリック → 拍のずれを CSV 出力）

- 1 拍 1 行、UTF-8 BOM + CRLF（Excel 直開き用）
- 列: `band_index,band_label,frequency_range_hz,beat_index,bar,beat_in_bar,`
  `expected_time_s,onset_time_s,deviation_ms,deviation_beats,onset_strength,status`
- `deviation_ms` 正 = 拍より遅い / 負 = 拍より早い
- 未検出の拍は測定値を空にし `status=undetected` で残す（設定で除外可）
- ファイル名: `<音源名>_band<番号>_<ラベル>_beats.csv`

### プロジェクト JSON / 設定 JSON

- `schema: "namivision.project"` / `"namivision.settings"`、`version: 1`
- 未知の schema や未来の version は読み込みを拒否して既定値に戻す（クラッシュしない）
- 設定の保存先: 通常 `%APPDATA%\com.namivision.app\settings.json`、
  `--portable` または `portable.txt` がある場合は `<exe>\data\settings.json`

### CLI

```text
NamiVision.exe [--open <audio>] [--project <file>] [--portable]
NamiVision.exe --selftest [--selftest-dir <dir>]
```

---

## 6. 既知の制約・注意

- **拍検出はヒューリスティック**。オンセット強度の閾値で決めており、複雑な音源では
  未検出の拍が残る。クリック音源でも先頭拍が未検出になりやすい（23/24 が典型）。
  CSV の `undetected` 行と設定の感度で調整する前提であり、BPM 推定機能は持たない。
- 対応形式は mp3 / wav / flac / ogg。長さは 5 秒以上 3600 秒以下。
  5 秒未満は `ERR_AUDIO_TOO_SHORT` で拒否する。
- WebView2 ランタイムが必須。無い場合は起動時に導入手順を表示する。
- **Windows 11 は未検証**（Windows 10 22H2 で実機確認）。Win11 は WebView2 同梱のため
  動作する見込みだが、リリースノートでは「Windows 10 22H2 で検証済み」と正直に書く。
- ポータブル版は書き込み可能な場所に展開する。`Program Files` 配下でも起動はするが、
  `portable.txt` があると設定が `data\` に落ちるため書き込み権限に注意。
- 音源を移動・削除したプロジェクトを開くとバナーで通知し、波形は空のまま起動する。
- 最小ウィンドウ 900×560。UI 倍率 150% では実質 600×373 相当になるため、
  狭い画面ではレーン数と表示範囲を絞る。

---

## 7. 紹介サイトの更新

公開ファイルは `oriyu90/studio-rizi` の `website/projects/nami-viewer/` が一次情報。
アプリ側リポジトリにサイトのソースは置かない（`common rules` ルール3）。

```powershell
gh repo clone oriyu90/studio-rizi $env:TEMP\studio-rizi
cd $env:TEMP\studio-rizi
# website/projects/nami-viewer/{index,en/index,zh/index,pt/index}.html を更新
npm run build   # generate:seo + validate + count-files
git add -A; git commit -m "Update NamiVision ..."; git push origin main
```

- 4 言語は独立した静的 URL。`canonical` / `hreflang` / `og:*` / JSON-LD を全ページで揃える。
- 画像は `website/projects/nami-viewer/assets/` に置き、本文からは
  ルート相対 `/projects/nami-viewer/assets/...` で参照する（`/en/` 配下でも解決させるため）。
  OGP 画像だけは絶対 URL が必要。
- `npm run build` の `count-files` は**同一ハッシュの公開ファイルをエラーにする**。
  同じ画像を別名で複製しない。
- `projects/manifest.json` と `website/content.js` は同じコミットで更新する。
- サイトのスクリーンショットは `shots/real-*.png` から生成する。差し替えたら
  `screen-dark` / `screen-light` の `width` / `height` 属性も実寸に合わせる
  （ずれると CLS が出る）。

---

## 8. トラブルシューティング

| 症状 | 原因と対処 |
|---|---|
| 起動時に真っ白 | WebView2 不在、または `ui/` の配信失敗。`cargo run` でログ確認 |
| 音が出ない | 出力デバイス変更後に再起動。cpal のストリームは起動時に固定 |
| CSV が文字化け | BOM が落ちている。`beats.rs` の `write_csv` を通さず手加工していないか確認 |
| 拍グリッドが表示されない | BPM 未設定。マスターレーン右クリック → テンポ設定が必要 |
| 帯域 CSV メニューが無効 | 同上。テンポ未設定時は仕様として無効化される |
| zip のハッシュが合わない | `package.ps1` は毎回 zip を作り直す。`.sha256` も同時に更新される |
| サイトで 404 | `manifest.json` の `hosted: true` と `website/projects/<slug>/index.html` の有無を確認 |
