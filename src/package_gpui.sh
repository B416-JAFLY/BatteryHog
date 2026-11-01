#!/bin/bash
# Build a self-contained GPUI app. Installs nothing in /Applications.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/src/version.sh"
CARGO="${CARGO:-cargo}"
PROFILE="${PROFILE:-release}"
APP_NAME="${APP_NAME:-Battery Hog GPUI}"
DIST="${DIST:-$ROOT/dist/gpui}"
APP="$DIST/$APP_NAME.app"
C="$APP/Contents"
rm -rf "$APP"
mkdir -p "$C/MacOS" "$C/Resources" "$C/Helpers"
"$CARGO" build --locked --manifest-path "$ROOT/gpui/Cargo.toml" --profile "$PROFILE"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/gpui/target}"
PROFILE_DIR="$PROFILE"; if [ "$PROFILE" = "dev" ]; then PROFILE_DIR=debug; fi
cp "$TARGET_DIR/$PROFILE_DIR/batteryhog-gpui" "$C/MacOS/BatteryHog"
SWIFT_TARGET="${SWIFT_TARGET:-$(uname -m)-apple-macosx11.0}"
xcrun swiftc -parse-as-library -O -whole-module-optimization -target "$SWIFT_TARGET" \
    "$ROOT/src/Localization.swift" "$ROOT/src/SystemSupport.swift" \
    "$ROOT/src/AgentGateStore.swift" "$ROOT/src/BatteryHogStores.swift" \
    "$ROOT/src/WorkloadDetector.swift" "$ROOT/src/BatteryHogBackend.swift" \
    "$ROOT/src/GPUIService.swift" -framework AppKit -framework UserNotifications \
    -o "$C/MacOS/batteryhog-service"
xcrun swiftc -parse-as-library -O -target "$SWIFT_TARGET" \
    "$ROOT/src/SystemSupport.swift" "$ROOT/src/AgentGateStore.swift" "$ROOT/src/BatteryHogGate.swift" \
    -framework Foundation -o "$C/Helpers/batteryhog-gate"
cp "$ROOT/src/Info.plist" "$C/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleName $APP_NAME" "$C/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName $APP_NAME" "$C/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier ${BUNDLE_ID:-com.b416jafly.batteryhog.gpui}" "$C/Info.plist"
# GPUI requires macOS 10.15.7+; retain this app's supported macOS 11 minimum.
stamp_bundle_version "$C/Info.plist"
cp -R "$ROOT/locales" "$C/Resources/"
ICONSET="$ROOT/.build/gpui-icon.iconset"
mkdir -p "$ICONSET"
for side in 16 32 128 256 512; do
    sips -z "$side" "$side" "$ROOT/src/icon_1024.png" --out "$ICONSET/icon_${side}x${side}.png" >/dev/null
    double=$((side*2))
    sips -z "$double" "$double" "$ROOT/src/icon_1024.png" --out "$ICONSET/icon_${side}x${side}@2x.png" >/dev/null
done
python3 "$ROOT/src/make_icns.py" "$ICONSET" "$C/Resources/AppIcon.icns"
cp "$ROOT/docs/GPUI-LICENSE-APACHE" "$C/Resources/GPUI-LICENSE-APACHE.txt"
cp "$ROOT/LICENSE" "$C/Resources/BatteryHog-LICENSE.txt"
printf 'APPL????' > "$C/PkgInfo"
for binary in "$C/MacOS/BatteryHog" "$C/MacOS/batteryhog-service" "$C/Helpers/batteryhog-gate"; do
    codesign --force --sign - "$binary"
done
codesign --force --sign - "$APP"
codesign --deep --verify --strict "$APP"
echo "Built: $APP"
