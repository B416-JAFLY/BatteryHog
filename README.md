# Battery Hog · GPUI

A native macOS battery monitor with a Rust GPUI interface and a Swift monitoring companion. The six pages cover live power and memory, ranked applications, development workloads, battery history and health, battery-life insights, and preferences. English and Simplified Chinese follow the ordered macOS application language preferences.

This implementation replaces the bundled WebKit/HTML interface. The GPUI app bundle contains no WebKit frontend, HTML, JavaScript, Electron, Node.js, Sparkle framework, or listening server. The Swift companion preserves the existing sampling engine, configurable menu-bar item, notifications, thermal-pressure attribution, guarded app quitting, Low Power Mode, and Agent Mode gate.

## Build and run

Requires an Apple Silicon Mac, macOS 11+, Xcode Command Line Tools, and a current stable [Rust toolchain](https://rustup.rs/). Development was validated with Rust 1.99.1. GPUI is pinned to the official `gpui` crate version 0.2.2 with a committed Cargo lockfile. Its `runtime_shaders` feature compiles the bundled Metal source at startup, so this build does not require the separate Xcode Metal command-line compiler.

```bash
git clone --branch feat/gpui https://github.com/B416-JAFLY/BatteryHog.git
cd BatteryHog
bash src/package_gpui.sh
open "dist/gpui/Battery Hog GPUI.app"
```

The script produces an optimized, ad-hoc signed `.app` without installing or replacing anything in `/Applications`. The first build downloads and compiles Rust dependencies. `PROFILE=dev` selects a faster development build. For an isolated trial use `BUNDLE_ID=com.b416jafly.batteryhog.gpui.preview bash src/package_gpui.sh`; the companion then uses the existing preview data directory. The default GPUI bundle ID is `com.b416jafly.batteryhog.gpui`; the stable build shares the existing BatteryHog settings/history directory for migration.

Keyboard shortcuts: **⌘1–⌘6** switch pages, **⌘R** refreshes, **⌘Q** quits. Tab/Shift-Tab move between controls; Return/Space activate a control. Closing the main window exits the app and its companion. The menu bar can open the main window or Workloads, toggle Low Power Mode, and quit eligible apps with native confirmation dialogs.

## Architecture and data

`gpui/src/ui.rs` renders GPUI elements and Metal-drawn charts. `gpui/src/bridge.rs` owns a signed bundled Swift child process and exchanges bounded JSON-line messages over inherited stdin/stdout pipes. Sampling and request processing run away from the GPUI render loop; no socket or HTTP server is opened. The companion exits on pipe EOF and is also terminated when the GUI bridge is dropped.

`src/GPUIService.swift` owns the AppKit status item and native confirmation/thermal notifications, and reuses the existing `BatteryHogBackend`, stores, and command gate. Destructive requests carry the complete process identity to the backend for revalidation. Unknown routes are rejected. Persisted settings are acknowledged only after the backend writes them successfully. Errors remain visible and Refresh can restart a failed companion.

History points retain null buckets and their positions across the complete requested interval. The axis uses actual history start/end timestamps in the Mac's local time zone; charging runs are marked beneath the curve. Live power samples retain their timestamps across a rolling 90-second window. On AC, reported battery draw is not a measurement of wall-adapter power. Estimated app impact remains a relative CPU-and-memory ranking, not per-app watts.

Monitoring stays local: `pmset`, `ioreg`, `system_profiler`, `vm_stat`, `sysctl`, `ps`, and `lsof` supply the same data as the original app. All network downloads occur during development/build. The GPUI app does not check for updates or send monitoring data. System-provided app names, paths, and diagnostic messages remain unchanged.

## Localization and verification

The GUI embeds `locales/en.json` and `locales/zh-Hans.json` directly; the Swift companion reads the same files from app resources. Named substitutions are performed once and English provides fallback values. GPUI draws app names as text rather than interpolating them into HTML. `BATTERY_HOG_LANGUAGE=en` or `zh-Hans` is a development override for both processes.

```bash
cargo fmt --manifest-path gpui/Cargo.toml --check
cargo test --locked --manifest-path gpui/Cargo.toml
python3 -m unittest discover -s tests -p 'test_*.py'
bash tests/run_native_tests.sh
bash tests/run_localization_tests.sh
BATTERY_HOG_TEST_SERVICE="$PWD/dist/gpui/Battery Hog GPUI.app/Contents/Helpers/BatteryHogService.app/Contents/MacOS/batteryhog-service" \
  python3 -m unittest discover -s tests -p test_gpui_service.py
```

The packaged service tests use temporary data directories and verify short-request delivery, settings persistence, both history ranges, rejected routes, and EOF shutdown. GPUI model tests cover sparse history, process ordering, language priority, plural forms, and literal placeholder values.

The pinned GPUI release does not expose the custom view contents to macOS VoiceOver. Keyboard navigation is implemented, but full screen-reader support requires a newer accessible GPUI release and another migration. Local builds are not notarized. Signing/notarization and a fork-specific update feed are separate release work.

The original WebKit implementation remains in the repository as a reference, with its existing build/test scripts and [legacy documentation](docs/legacy-webkit.md); `src/package_gpui.sh` exclusively builds the GPUI application.

## License

Battery Hog remains [MIT](LICENSE), © Luke Fairbanks. GPUI is Apache-2.0; its license is included in [docs/GPUI-LICENSE-APACHE](docs/GPUI-LICENSE-APACHE) and in the built bundle. Rust dependencies retain their respective licenses.
