# CSE Trader desktop app (Windows, macOS, Linux)

The desktop app is the same Tauri app as the Android one, with the same
screens, structure markers, alerts, history, paper and real portfolios and
US stocks, laid out for a wide window:

- tabs in a left sidebar;
- a stock, alert or settings sheet opens as a side panel, so the list stays
  visible and you can click from stock to stock (Esc closes the panel);
- below about 1100 px wide it falls back to the phone layout.

## Running in the background

Closing the window hides it to the **tray** (Windows notification area, macOS
menu bar, Linux status area). CSE and US prices keep refreshing and alerts
show as **system notifications**. Click the tray icon to bring the window
back; **Quit** in the tray menu stops it. Opening the app again while it is
running just shows the window. Desktops without a tray (some Linux setups)
quit on close instead.

Checks run only while the app is running (open or in the tray). There is no
scheduled check while it is quit, unlike Android. The phone's check
frequency buttons don't apply here.

## Differences from Android

- **ATrad:** *Place in ATrad* copies the order ticket to the clipboard for
  ATrad's website; *Sync from ATrad* needs the Android app (it reads the
  ATrad phone app). Enter or import real trades on the Portfolio tab.
- **Own data:** the desktop keeps its own database (watch lists, portfolios,
  alert history), separate from the phone and from the `cse` CLI. Use
  Portfolio → export / import to move real trades across.
  Location: `%APPDATA%\lk.personal.csetrader` (Windows),
  `~/Library/Application Support/lk.personal.csetrader` (macOS),
  `~/.local/share/lk.personal.csetrader` (Linux).

## Getting the installers

GitHub Actions builds them (`.github/workflows/desktop.yml`) on every push
that changes the app, or by hand from **Actions → Desktop app → Run
workflow**. Open the finished run and download:

| Artifact | File |
|---|---|
| `cse-trader-windows` | `CSE Trader_<version>_x64-setup.exe` (or the `.msi`) |
| `cse-trader-macos` | `CSE Trader_<version>_universal.dmg` (Apple Silicon and Intel) |
| `cse-trader-linux` | `.deb` or `.AppImage` |

Pushing a tag such as `desktop-v0.9.7` also attaches them to a GitHub
release.

The installers are not code-signed, so the first launch warns:

- **Windows:** "Windows protected your PC" → *More info* → *Run anyway*.
- **macOS:** open the `.dmg`, drag CSE Trader to Applications, then
  right-click it → *Open* → *Open*. If macOS says the app "is damaged", run
  `xattr -cr "/Applications/CSE Trader.app"` in Terminal once.

## Building locally

Needs Rust, Node and the Tauri CLI (`npm install -g @tauri-apps/cli@2`), plus
on Linux `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev`.

```sh
cd app/src-tauri
tauri dev                 # run it
tauri build               # installers in target/release/bundle/
```
