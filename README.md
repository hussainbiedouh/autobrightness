# AutoBrightness

AutoBrightness is a lightweight Windows auto-brightness app built with **Tauri 2** and **Rust**. It samples ambient light from your webcam (silently, via Media Foundation), computes a target brightness, and applies it to supported displays.

> **Windows only.** This project uses Windows APIs (WMI, DDC/CI via WinAPI, Media Foundation, power status).

## Features

- **Ambient light sensing (camera):** Samples luminance from the default video capture device (RGB32, low resolution, periodic sampling). Supports manual exposure lock when available (`locked` mode) with a `coarse` fallback when manual exposure cannot be verified.
- **Display control:**
  - **Internal/laptop panels:** WMI (`WmiMonitorBrightnessMethods`) with readback verification.
  - **External monitors:** DDC/CI (VCP `0x10`) when supported, with per-display trim.
  - **Unsupported DDC displays:** Reported as `Unsupported (DDC/CI unavailable)` with HRESULT/details. Probed once per session and cached (no retry loop). **No software/gamma dimming fallback.**
- **Adaptive control:** Baseline-anchored target curve with EMA smoothing, deadband, and step limiting to avoid rapid jumps.
- **Power-aware:** Can pause automation on battery below a configurable threshold (with safeguards for bogus battery readings).
- **Tray-first desktop app:** Frameless window, minimize/close-to-tray behavior, tray menu (Show / Pause-Resume / Quit), tooltip status, single-instance focus.
- **Autostart:** Optional "Start with Windows" toggle (HKCU Run, launches minimized).
- **Local settings:** Stored at `%APPDATA%\AutoBrightness\settings.json`.

## Requirements

- Windows 10/11
- A webcam (for ambient sampling) — if unavailable/busy, the app holds the last target and reports status
- For internal panel control: WMI monitor brightness support
- For external monitor control: DDC/CI enabled/supported by the monitor and connection path

## Build from source

### Prerequisites

- [Rust](https://rustup.rs/) (stable) with MSVC toolchain on Windows
- Visual Studio Build Tools / C++ build tools (for Windows MSVC target)

### Build

```powershell
cargo build --release --manifest-path src-tauri/Cargo.toml
```

The release binary will be at:

- `src-tauri\target\release\auto-brightness.exe`

The UI is static (`ui/`) and bundled via Tauri (`frontendDist = "../ui"`).

## Run

- Launch `auto-brightness.exe` (or the built binary).
- Pass `--minimized` to start hidden to tray (used for autostart).
- Use **Calibrate** to anchor current ambient light and current average brightness as baselines.
- Adjust **Interval**, **Sensitivity**, per-display **Trim**, and battery pause settings in the UI.

## Settings

Settings are persisted to:

- `%APPDATA%\AutoBrightness\settings.json`

Key fields include sampling interval, sensitivity, per-display trims, autostart, battery pause options, baselines, and paused state. Invalid/unsafe persisted values are sanitized on load.

## Notes & limitations

- DDC/CI support varies by monitor, cable, GPU, and OSD settings (many monitors ship with DDC/CI disabled).
- The app does **not** implement gamma-ramp/software dimming as a fallback.
- Camera-based ambient estimation can be affected by camera placement, auto-exposure behavior (coarse mode), and scene content.

## Project structure

- `src-tauri/` — Rust/Tauri backend (engine worker, camera/MF sampling, WMI/DDC brightness, power, settings)
- `ui/` — Static frontend (HTML/CSS/JS)
- `icon.ico` / `src-tauri/icons/` — App icons

## License

MIT — see [LICENSE](LICENSE).
