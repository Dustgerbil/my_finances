#!/bin/bash

set -euo pipefail

SIGNING_IDENTITY="My Finances Local Code Signing"
BUNDLE_IDENTIFIER="com.stuartg.my-finances"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TAURI_CONFIG="$PROJECT_ROOT/src-tauri/tauri.conf.json"
APP_BUNDLE="$PROJECT_ROOT/src-tauri/target/release/bundle/macos/My Finances.app"

fail() {
  echo "error: $*" >&2
  exit 1
}

command -v node >/dev/null 2>&1 || fail "Node.js is required."
command -v npm >/dev/null 2>&1 || fail "npm is required."

CONFIGURED_IDENTITY="$(
  node --input-type=module -e '
    import { readFileSync } from "node:fs";
    const config = JSON.parse(readFileSync(process.argv[1], "utf8"));
    process.stdout.write(config.bundle?.macOS?.signingIdentity ?? "");
  ' "$TAURI_CONFIG"
)"

if [[ "$CONFIGURED_IDENTITY" != "$SIGNING_IDENTITY" ]]; then
  fail "tauri.conf.json must use signing identity \"$SIGNING_IDENTITY\"."
fi

IDENTITY_SHA1="$(
  security find-identity -v -p codesigning |
    awk -v identity="\"$SIGNING_IDENTITY\"" 'index($0, identity) { print $2; exit }'
)"

if [[ -z "$IDENTITY_SHA1" ]]; then
  fail "Signing identity \"$SIGNING_IDENTITY\" is not visible. Run this script from a normal Terminal, or grant the task access to the macOS login Keychain."
fi

EXPECTED_ROOT="$(printf '%s' "$IDENTITY_SHA1" | tr '[:upper:]' '[:lower:]')"

echo "Using signing identity: $SIGNING_IDENTITY ($IDENTITY_SHA1)"
echo "Building production app..."

cd "$PROJECT_ROOT"
npm run tauri build

[[ -d "$APP_BUNDLE" ]] || fail "Tauri did not produce $APP_BUNDLE."

echo "Verifying code signature..."
codesign --verify --deep --strict --verbose=2 "$APP_BUNDLE"

SIGNATURE_INFO="$(codesign -dvvv "$APP_BUNDLE" 2>&1)"
DESIGNATED_REQUIREMENT="$(codesign -d -r- "$APP_BUNDLE" 2>&1)"

if grep -Fq "Signature=adhoc" <<<"$SIGNATURE_INFO"; then
  fail "The app is ad-hoc signed."
fi

grep -Fq "Authority=$SIGNING_IDENTITY" <<<"$SIGNATURE_INFO" ||
  fail "The app was not signed by \"$SIGNING_IDENTITY\"."

grep -Fq "Identifier=$BUNDLE_IDENTIFIER" <<<"$SIGNATURE_INFO" ||
  fail "The signed bundle identifier is not $BUNDLE_IDENTIFIER."

grep -Fq "identifier \"$BUNDLE_IDENTIFIER\"" <<<"$DESIGNATED_REQUIREMENT" ||
  fail "The designated requirement does not contain $BUNDLE_IDENTIFIER."

grep -Fq "certificate root = H\"$EXPECTED_ROOT\"" <<<"$DESIGNATED_REQUIREMENT" ||
  fail "The designated requirement does not use the expected certificate."

echo
echo "Signed app is ready:"
echo "  $APP_BUNDLE"
echo
echo "Designated requirement:"
echo "  $DESIGNATED_REQUIREMENT"
echo
echo "You can now move My Finances.app into /Applications."
