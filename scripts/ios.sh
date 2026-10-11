#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

# Tauri's Xcode subprocess does not consistently inherit DEVELOPER_DIR.
# Keep the tool selection local to this command, without requiring sudo or
# changing the user's system-wide xcode-select setting.
export DEVELOPER_DIR="${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}"
if [[ ! -d "$DEVELOPER_DIR" ]]; then
  echo "Install Xcode or set DEVELOPER_DIR to its Contents/Developer directory." >&2
  exit 1
fi
ios_tools=$(mktemp -d "${TMPDIR:-/tmp}/my-finances-ios-tools.XXXXXX")
trap 'rm -rf "$ios_tools"' EXIT
for tool in xcodebuild xcrun; do
  printf '#!/bin/bash\nexport DEVELOPER_DIR=%q\nexec /usr/bin/%s "$@"\n' "$DEVELOPER_DIR" "$tool" > "$ios_tools/$tool"
  chmod +x "$ios_tools/$tool"
done
# swift-rs 1.0.7 expects SwiftPM's native build layout and passes iOS compiler
# flags that the new Xcode 27 default swiftbuild engine ignores. Scope this
# compatibility setting to this build; do not patch installed dependencies.
printf '#!/bin/bash\nexport DEVELOPER_DIR=%q\nif [[ "${1:-}" == build ]]; then\n  shift\n  exec "$DEVELOPER_DIR/Toolchains/XcodeDefault.xctoolchain/usr/bin/swift" build --build-system native --jobs 2 "$@"\nfi\nexec "$DEVELOPER_DIR/Toolchains/XcodeDefault.xctoolchain/usr/bin/swift" "$@"\n' "$DEVELOPER_DIR" > "$ios_tools/swift"
chmod +x "$ios_tools/swift"
# Each Cargo build script can start its own Swift compiler pool. Bound both
# levels to avoid multiplying CPU and memory use during a cold mobile build.
printf '#!/bin/bash\nexport CARGO_BUILD_JOBS=2\nexec %q "$@"\n' "$(command -v cargo)" > "$ios_tools/cargo"
chmod +x "$ios_tools/cargo"
export PATH="$ios_tools:$PATH"
npm run tauri -- ios "$@"
