# Read-only iOS app with automatic sync

Decision date: 2026-09-26. Status: implemented; simulator startup and cached display verified. Real-device sync verification pending.

## Agreed scope

Automatic sync is required in the first release. A manual import workflow is not
the first milestone. Reuse Tauri, React, and the Rust financial calculations.
The iPhone displays dashboard totals, account histories, change periods,
repayment details, Other Accounts, and Retired Accounts. It cannot edit financial
data, fetch from Akahu, restore backups, or change retirement/ignore settings.

## Data flow

1. The Mac continues managing its existing SQLite database and Akahu credentials.
2. After successful financial changes, startup, or changes arriving from another
   Mac, it publishes a consistent SQLite snapshot to
   `my_finances/mobile/read-only.db` in the existing iCloud Drive folder.
   Publish only when iCloud Drive is available; report unavailable or failed
   publication separately from successful financial updates. Retry failed
   publication without repeating the financial operation.
3. On the iPhone, the user selects that `mobile` folder once through Apple's
   document picker. A native bridge saves a security-scoped bookmark so it can
   access subsequent versions without repeated imports. Folder access also
   avoids tying the bookmark to a file that the Mac replaces atomically.
4. On launch, foreground entry, and periodically while active, the bridge
   coordinates a read through Apple's file APIs and downloads the file if needed.
   It stages the received snapshot inside the app's private storage.
5. Rust validates the staged snapshot, then atomically replaces the private cache
   while holding the existing local database lock. Queries open that cache
   read-only. Emit `db-changed` only after successful promotion.

The Mac publishes an independent SQLite snapshot, not a raw copy made while its
live database is changing. Stage snapshot generation outside iCloud, close all
connections, then atomically publish the completed single file. Never copy SQLite
journal files. Export only required financial tables and allowlisted settings
(including retirement flags and source refresh time); omit machine-local paths,
backup configuration, and credentials. Preserve stored historic virtual totals.

This is automatic after the one-time folder connection. iCloud controls delivery
latency; the app does not promise instant updates or continuous execution while
the phone app is suspended. The Mac must have fetched/published newer balances
before the phone can show them. A manual "Check for updates" action may supplement
automatic refresh but is not required for ordinary updates.

## Read-only and failure boundaries

- iOS registers only financial query commands and the narrow local sync commands.
  Hide editing controls and routes, including account restoration from retirement.
- iOS startup never opens the financial cache read-write, migrates it, seeds
  totals, merges conflict copies, runs backups, or accesses Akahu credentials.
- Cache replacement and local connection preferences are permitted local writes;
  the iPhone never modifies the shared snapshot or the Mac database.
- Validate SQLite integrity, the expected tables/columns, and a snapshot format
  version before promotion. Reject unsupported versions with a useful message.
- Serialize downloads/promotions; never allow an older overlapping request to
  replace a newer result. Compare completed snapshot content to avoid unnecessary
  refresh events. A deliberate Mac restore must be able to publish a new snapshot
  containing older financial history.
- Keep the last validated cache on download failure, malformed data, unavailable
  iCloud, or revoked folder access. Do not show stale data as freshly updated.
- Display source data time separately from last successful sync time, plus
  connecting, checking, offline/cached, permission-needed, and failure states.
- Reconnect explicitly if bookmark resolution or folder access fails. Cache and
  bookmark storage remain private to the app; do not log balances or bookmark data.

## Native integration and dependency approval

The installed `tauri-plugin-dialog` 2.7.1 supports iOS file selection, but its
Swift source has no persistent bookmark management or coordinated sync lifecycle.
Do not modify that installed dependency or treat a returned path as durable access.

Add a repository-owned `tauri-plugin-finance-sync` bridge with a Swift package
using UIKit/Foundation. Wire it as an iOS-only local Cargo dependency and use the
official `tauri-plugin` build helper with its `build` feature. The helper is already
present transitively in the local Cargo registry, but declaring a new dependency
was explicitly approved by the user. Reuse existing Tauri/Serde dependencies;
no third-party cloud service or database schema change is proposed.

Folder access through the system document picker is the proposed route, rather
than a new app-owned CloudKit container. Persistent access with the user's free
Personal Team must be verified on a real iPhone before claiming compatibility.
If signing or entitlement restrictions block this route, report the exact failure
before changing architecture or asking for paid enrollment.

## Tooling checked on 2026-09-26

- Xcode 27.0, build 27A266a, is installed at `/Applications/Xcode.app`.
- The iOS 27.0 simulator runtime is installed; Xcode first-launch checks pass.
- System `xcode-select` still selects Command Line Tools. `scripts/ios.sh` provides
  local wrappers for Xcode commands because Tauri drops a plain `DEVELOPER_DIR`
  override in a subprocess; no administrator password or global switch is needed.
- Rust device and simulator targets are installed. XcodeGen, libimobiledevice,
  and CocoaPods were installed from Homebrew core for Tauri's iOS tooling.
- The generated Apple project is in `src-tauri/gen/apple`. The app targets iOS 15
  or later, the minimum supported by the installed Xcode.
- Existing uncommitted retired-account changes must be preserved.

## Implementation and acceptance sequence

1. Add the native bridge and iOS project; verify folder selection, bookmark
   persistence, coordinated reads, and foreground refresh using synthetic data.
2. Add Mac snapshot publication and staged-cache validation, with focused tests
   for consistent exports, rejected snapshots, retry behaviour, offline retention,
   and attempted iOS write commands. Do not use the live database for tests.
3. Add the iOS read-only startup/command profile and sync status, adapting the
   existing screens for phone sizes without changing the Mac editing workflow.
4. Run the full Rust suite, frontend build, formatting checks, and an actual iOS
   simulator build. Verify empty, cached, failed, reconnect, and updated states.
5. Verify Mac-to-iPhone iCloud delivery on a signed Personal Team device: select
   once, receive later updates, relaunch, go offline, resume, and revoke/reconnect
   access. Confirm source files remain unchanged after phone usage.

Do not call automatic sync complete based on a simulator build alone. Real iCloud
delivery and Personal Team provisioning need device verification. No production
Mac installation, Apple agreement acceptance, paid enrollment, or publication is
part of this implementation step.

## Build and connect

```sh
# From the nested my_finances application directory:
bash scripts/ios.sh build --debug --target aarch64-sim --no-sign --ci

# For a physical iPhone, add your Apple Account in Xcode, select your Personal
# Team for the app target, and use the team ID with Tauri. Do not commit it:
APPLE_DEVELOPMENT_TEAM=YOUR_TEAM_ID bash scripts/ios.sh dev
```

Run an updated Mac build to begin publishing. Publication is checked every 15
seconds and retried after failures; its status is visible under **iPhone sync**.
The iPhone checks every 30 seconds while visible and on return to the foreground.
Connect **iCloud Drive → my_finances → mobile**, then browse cached accounts even
when offline. The initial app has no financial data until the first valid sync.

The Mac publisher and the iOS cache logic remain in `src-tauri/src/lib.rs` in
accordance with repository conventions. The native bridge is isolated under
`plugins/tauri-plugin-finance-sync`; its API is callable from Rust, not directly
from the frontend. `src/sync.tsx` obtains the runtime's read-only flag before any
financial screen mounts. The iOS command handler registers only queries and sync
commands; the desktop command handler retains editing and Akahu operations.

## Verification notes

- All 22 Rust tests pass, including snapshot allowlisting, source immutability,
  read-only queries, invalid-cache rejection, and atomic replacement retention.
- Frontend production build, desktop Tauri debug build, and iOS simulator build pass.
- The iOS 27 simulator opens the connection screen and displays a synthetic saved
  account cache after relaunch with no folder connected. Automatic sync returns
  the expected connection-needed message while retaining the cache. No real
  financial data was used.
- Folder picker interaction, bookmark persistence, reconnect, and real iCloud
  delivery remain unverified: Device Hub accessibility automation timed out.
  Verify these on a signed physical iPhone before calling sync ready for use.
- A repeated Tauri simulator build may fail at the final rename if the previous
  generated `build/arm64-sim/My Finances.app` is present. Move that generated
  bundle aside and rebuild; do not remove source files.
- The Apple project includes a static `TaoSceneDelegate` scene manifest, required
  to avoid an iOS 27 startup trap. Keep it in both `project.yml` and `Info.plist`.
- Xcode 27 SwiftPM builds use the native engine through the local wrapper; this
  preserves the SDK flags expected by Tauri without modifying dependencies.

## References

- [Apple: Providing access to directories](https://developer.apple.com/documentation/uikit/providing-access-to-directories)
- [Apple: UIDocumentPickerViewController](https://developer.apple.com/documentation/uikit/uidocumentpickerviewcontroller)
- [Tauri: Mobile plugin development](https://v2.tauri.app/develop/plugins/develop-mobile/)

- [Apple: UIKit scene lifecycle migration](https://developer.apple.com/documentation/technotes/tn3187-migrating-to-the-uikit-scene-based-life-cycle)
- [Tauri: iOS 27 static scene configuration issue](https://github.com/tauri-apps/tauri/issues/15719)
