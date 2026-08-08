# AGENTS.md — My Finances

Guide for AI coding agents working in this repository. **Read this first.**

## What this is

A local desktop app to review and manage personal finances. A button fetches current
account balances from [Akahu](https://akahu.io/) (a NZ open-banking aggregator), stores
them in a local SQLite DB, and shows summaries plus per-account history graphs.

Full product spec: [`spec.md`](spec.md).

## Repo layout

```
my_finances/                 <- Tauri + React + TypeScript app
  src/                       React 19 frontend (React Router v7, recharts)
    App.tsx                  Routes: / /config /settings /account/:id
    pages/
      Dashboard.tsx          Main screen: cards + "Refresh now" button
      AccountDetail.tsx      Per-account line chart + min/max + changes
      AccountsConfig.tsx     Toggle ignore, add manual accounts
      Settings.tsx           Akahu credentials + autofetch toggle
    types.ts                 Shared TS types (mirror the Rust structs)
  src-tauri/                 Rust backend
    src/lib.rs               ALL backend logic + Tauri commands (single file)
    Cargo.toml
    tauri.conf.json
my_accounts.py              (legacy) standalone Akahu script, not part of the app
spec.md                      Product spec & boundaries
```

> Backend is intentionally one file (`src-tauri/src/lib.rs`). Don't split unless asked.

## Tech stack

- **Frontend:** Vite 7, React 19, TypeScript, recharts, react-router-dom v7
- **Desktop shell:** Tauri v2
- **Storage:** SQLite via `rusqlite` — opened/closed per operation (NOT a
  persistent connection), so iCloud can sync the file. Same-machine access
  serialized by `AppState.local_lock: Mutex<()>`.
- **HTTP:** `reqwest` (async) for Akahu calls
- **Secrets:** macOS keychain via `keyring` (falls back to env vars
  `AKAHU_APP_ID` / `AKAHU_USER_TOKEN`)
- **DB path (Phase 1, iCloud):** `~/Library/Mobile Documents/com~apple~CloudDocs/my_finances/my_finances.db`
  when iCloud Drive is present; falls back to `~/Library/Application Support/my_finances.db`
  otherwise. Resolution lives in `resolve_db_path()` (single source of truth).
- **Remote-change refresh:** `notify` crate watches the DB file; on change
  emits a `db-changed` Tauri event; frontend (Dashboard, AccountDetail)
  debounces and re-loads.
- **Cross-machine conflict handling (Phase 2A):** there is **no cross-machine
  lock**. When two machines write and iCloud produces a conflict copy (named
  like `my_finances (…).db`), `merge_conflict_copies()` (called from
  `open_db_rw` only — it writes) does a **lossless row-level union merge** of
  the conflict copy into the live DB via SQLite `ATTACH`, then archives the
  conflict file to `conflicts/`. Rules: `accounts`/`settings` union by key,
  keep-live on collision; `account_values` unions by `(account_id,
  recorded_date)`, keep-live on collision; `fetch_log` unions by `fetched_at`.
  Reads may be momentarily stale until the next write/startup merges —
  acceptable. The lossy last-writer-wins of Phase 1 is gone.
- **Idempotent writes (Phase 2A):** `fetch_akahu_balances` now upserts per
  `(account, day)` — DELETE-then-INSERT on `account_values` and `fetch_log` —
  so a same-day re-fetch on another machine overwrites rather than duplicating.
  This keeps the merge lossless even when two machines fetch the same day
  without producing a conflict copy. (Cross-machine mutual exclusion / CloudKit
  lease is a future Phase 2B, not yet built.)
- **Migration:** on first launch with iCloud present, `migrate_legacy_into_icloud()`
  copies the legacy local DB into iCloud once (guarded by a `migrated_to_icloud`
  settings row); the old local file is kept as a backup.
- **Backups (sub-phase 1):** point-in-time snapshots of the whole DB via
  `VACUUM INTO` + gzip → `.db.gz` files named `my_finances-YYYYMMDD-HHmmSS.db.gz`.
  Stored **locally** in `~/Library/Application Support/my_finances/backups/`
  (default) — deliberately NOT in iCloud, so they survive even if the live DB is
  lost. Per-machine, not synced. Frequency `off`/`daily`/`weekly`/`monthly` is
  user-configurable; on-launch `maybe_backup_on_launch` runs one if due
  (mirrors the autofetch pattern). Retention is "keep last N" (daily=30,
  weekly=12, monthly=24); `off` does not prune. Settings keys: `backup_frequency`,
  `last_backup_at` (UTC ISO-8601), `backup_dir` (optional custom absolute path).
  Restore is available from Settings using `tauri-plugin-dialog`: it validates
  the selected `.db.gz`, creates a fresh safety backup, and rolls back if the
  replacement cannot be installed. A custom-dir picker remains future work.

## Run / build

```bash
cd my_finances
npm install            # first time
npm run tauri dev      # dev (hot reload, opens window)
npm run tauri build    # production: builds .app (and tries .dmg — see note)
```

Build output: `my_finances/src-tauri/target/release/bundle/macos/My Finances.app`.

> **DMG bundling is disabled** (`bundle.targets = "app"` in `tauri.conf.json`).
> Previously `targets = "all"` failed on the AppleScript icon-positioning
> step (`Finder got an error ... Can't set item ... (-10006)`); the `.app`
> always built fine. Re-enable `"all"` only if a `.dmg` is actually needed.
> When debugging an existing DB, inspect with:
> `sqlite3 ~/Library/Application\ Support/my_finances.db`

### macOS production signing

Production builds are signed with the self-signed identity
`My Finances Local Code Signing`, configured by
`bundle.macOS.signingIdentity` in `src-tauri/tauri.conf.json`.

- Run `npm run tauri build` with permission to access the macOS login Keychain.
- Sandboxed commands can incorrectly report `0 valid identities found` even
  when the signing identity is installed and valid.
- If signing fails with `My Finances Local Code Signing: no identity found`,
  rerun the same build with elevated/outside-sandbox Keychain access.
- Do not remove `signingIdentity`, create another certificate, or fall back to
  ad-hoc signing in response to the sandbox-only failure.
- A failed signing attempt can leave an ad-hoc-signed `.app` in the bundle
  directory. Do not treat that artifact as a successful production build.
- After a successful build, verify the bundle with:

  ```bash
  codesign --verify --deep --strict --verbose=2 \
    "src-tauri/target/release/bundle/macos/My Finances.app"
  codesign -d -r- \
    "src-tauri/target/release/bundle/macos/My Finances.app"
  ```

- The designated requirement must contain `com.stuartg.my-finances` and the
  certificate root, rather than only a `cdhash`.
- The missing-notarization warning is expected for this local self-signed build.

## Data model (SQLite)

- `accounts(id PK, name, type, institution, currency, is_manual, is_ignored)`
- `account_values(id PK, account_id FK, balance REAL, recorded_date TEXT 'YYYY-MM-DD')`
- `fetch_log(id PK, fetched_at TEXT 'YYYY-MM-DD')` — one row per successful fetch day
- `settings(key PK, value TEXT)` — keys: `autofetch` (`"true"`|`"false"`),
  `migrated_to_icloud` (one-time migration marker), `backup_frequency`
  (`off`|`daily`|`weekly`|`monthly`), `last_fetch_at` (UTC ISO-8601 precise
  successful-fetch time), `last_backup_at` (UTC ISO-8601),
  `backup_dir` (optional custom backups path). All are add-as-needed key/value
  rows — no schema change is needed to add a new setting.

### Virtual accounts (important!)

Three synthetic rows live in `accounts` with ids `__total__`,
`__assets_total__`, `__liabilities_total__`. They have no Akahu backing — their
values are **derived** by summing real account balances. They appear as cards on
the dashboard (Total, Assets & Savings subtotal, Liabilities subtotal) and each
has its own history graph.

**One value per day per account is the intended model** (spec: "saves no more
than once a day"). Automatic fetches early-return `"Already fetched today"` if
a row exists in `fetch_log` for today. A user-requested forced refresh bypasses
that guard, but replaces the existing same-day values rather than adding rows.
Manual-account writes likewise replace an existing value for the same day.

Virtual totals do NOT early-return — `recompute_total()` is called from
`fetch_akahu_balances`, `toggle_ignore_account`, `add_manual_account_value`, and
`seed_total_if_missing` (startup). To keep them at one-row-per-day, always go
through **`upsert_daily_total()`** (DELETEs the same-day row, then INSERTs) —
never raw INSERTs. Forgetting this re-introduces duplicate-day rows that make the
graphs zig-zag vertically and look "stale". (See history fix, 2026-06-18.)

## Backend commands (`#[tauri::command]`, registered in `run()`)

| Command | Purpose |
|---|---|
| `fetch_akahu_balances` | Trigger Akahu refresh, fetch accounts, store NZD-only, recompute totals. `force=false` is once/day; `force=true` replaces today's snapshot. |
| `get_accounts_summary` | Cards data, incl. virtual accounts (`id LIKE '__'`). |
| `get_account_history` | Per-account values for the line chart. |
| `get_account_changes` | 30d/180d/360d/all-time change chips (lookback by recorded_date). |
| `get_all_accounts_config` | All real accounts (excl. virtual) for config screen. |
| `toggle_ignore_account` | Flip `is_ignored`; recomputes totals. Virtual accounts rejected. |
| `add_manual_account` | Insert manual account row. |
| `add_manual_account_value` | Insert today's value for a manual account; recomputes totals. |
| `has_akahu_credentials` / `save_akahu_credentials` | keychain read/write. |
| `get_autofetch_setting` / `set_autofetch_setting` | settings table (autofetch). |
| `get_backup_settings` | Returns frequency, last_backup_at, backup_dir, is_default_dir. |
| `get_data_health` | Returns DB integrity/path, iCloud status, last fetch/backup, row counts, and pending conflict-copy count. |
| `set_backup_frequency` | Sets `backup_frequency` setting (`off`/`daily`/`weekly`/`monthly`). |
| `do_backup_now` | Force a `.db.gz` snapshot now; prunes; updates `last_backup_at`. |
| `restore_backup` | Validates and restores a `.db.gz` after taking a safety backup; rolls back on failure. |
| `maybe_backup_on_launch` | Backs up only if a frequency is set and one is due (called on Dashboard load). |
| `open_backups_folder` | Opens the backups dir in Finder via the opener plugin. |
| `get_app_version` | build timestamp. |

## Key behaviors & gotchas

- **Currency filter:** only NZD accounts are stored; non-NZD is silently dropped.
  Akahu accounts with no currency value are assumed to be NZD.
- **Autofetch:** when enabled, Dashboard auto-fetches once on first load if
  credentials exist (guarded by a `useRef` so it doesn't loop). This uses
  `force=false`; the always-visible manual "Refresh now" action uses
  `force=true` to replace today's snapshot with current Akahu data. After the
  Akahu refresh job completes, the backend polls `/v1/accounts` until each
  refreshable NZD account's `refreshed.balance` timestamp is no more than 65
  minutes old. If freshness cannot be verified within 30 seconds, it writes no
  account values or daily fetch marker, so the request can be retried.
- **Max-balance ⭐ badge:** shown on a card when `latest_balance == max_balance`.
- **Ignore:** ignored accounts are hidden from the dashboard *and* excluded from
  virtual total sums.
- **Changes lookback:** `get_account_changes` returns `None` for a fixed period
  when the account's earliest recorded_date is after the lookback date (not
  enough history yet). The all-time change compares the latest balance with the
  earliest recorded balance.
- **Frontend↔backend types must stay in sync:** `src/types.ts` mirrors the Rust
  structs in `lib.rs`. When changing a struct, update both.
- All Tauri commands are invoked from the frontend via `@tauri-apps/api/core`'s
  `invoke`. Argument names are camelCase on the TS side, snake_case on the Rust
  side (Tauri auto-converts).
- **DB connection discipline (iCloud sync correctness):** connections are
  opened and closed **per operation** via `with_db_rw`/`with_db_ro` (or
  `open_db_rw`/`open_db_ro`). Never hold a connection open across an `await`
  or for the app's lifetime — a held-open file handle blocks iCloud from
  syncing the DB. `fetch_akahu_balances` deliberately drops its lock during the
  network refresh and only re-acquires for the write.
- **`journal_mode=DELETE` is mandatory** (pinned in `init_db`). Never enable
  WAL — its `-wal`/`-shm` sidecars sync independently across iCloud and corrupt
  the DB.
- **Writes must be idempotent per (account, day)** (Phase 2A): real-account
  value inserts in `fetch_akahu_balances` use DELETE-then-INSERT keyed on
  `(account_id, recorded_date)`, as do manual-account value writes, mirroring
  `upsert_daily_total()` for virtuals.
  Never raw-INSERT a same-day row — it breaks the lossless merge when two
  machines fetch the same day.
- **Never recompute historical virtual totals.** `recompute_total(conn, today)`
  derives the total from each account's *latest* balance by `recorded_date`
  (using `id` only to break same-day ties), not a date-specific balance.
  Calling it for a past date would store
  today's total under that past date and corrupt the graph. The merge relies on
  unioning existing per-date total rows, not recomputing them.

## Project boundaries (from spec.md — respect these)

- ⚠️ **Ask first** before: database schema changes, adding dependencies.
- 🚫 **Never:** commit secrets, edit `node_modules/`, modify CI config.

## When you change something

- If you touch the SQLite schema, bump/migrate carefully — there's no migration
  framework; `init_db` only does `CREATE TABLE IF NOT EXISTS`.
- After backend Rust changes, rebuild: `npm run tauri dev` or `npm run tauri build`.
- Frontend-only changes hot-reload under `npm run tauri dev`.
