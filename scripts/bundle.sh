#!/usr/bin/env bash
# Build RustPaste.app, optionally sign and notarize it.
#
#   scripts/bundle.sh                       # ad-hoc signed app in dist/
#   SIGN_ID="Developer ID Application: Name (TEAMID)" scripts/bundle.sh
#   SIGN_ID=... NOTARY_PROFILE=my-profile scripts/bundle.sh   # + notarize & staple
#
# NOTARY_PROFILE is a keychain profile created with:
#   xcrun notarytool store-credentials my-profile --apple-id ... --team-id ...
set -euo pipefail
cd "$(dirname "$0")/.."

APP="dist/RustPaste.app"
cargo build --release
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp target/release/rustpaste "$APP/Contents/MacOS/rustpaste"
cp packaging/Info.plist "$APP/Contents/Info.plist"

if [[ -n "${SIGN_ID:-}" ]]; then
  codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$APP"
else
  echo "No SIGN_ID set: using ad-hoc signature (fine for local use)."
  codesign --force --sign - "$APP"
fi

if [[ -n "${SIGN_ID:-}" && -n "${NOTARY_PROFILE:-}" ]]; then
  ditto -c -k --keepParent "$APP" dist/RustPaste.zip
  xcrun notarytool submit dist/RustPaste.zip --keychain-profile "$NOTARY_PROFILE" --wait
  xcrun stapler staple "$APP"
fi
echo "Built $APP"
