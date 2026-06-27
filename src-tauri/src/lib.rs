use chrono::Local;
use chrono::Utc;
use tauri_plugin_opener::OpenerExt;
use rusqlite::Connection;
use notify::Watcher;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Emitter, Manager, State};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// State held by the Tauri app for the lifetime of the process.
///
/// The database is opened and closed per-operation rather than kept open
/// persistently. A held-open file handle blocks the iCloud daemon from
/// swapping in remote updates to the synced DB file, so each command opens a
/// short-lived `Connection`, does its work, and drops it. The `local_lock`
/// serializes same-machine access so only one connection is open at a time on
/// this machine (SQLite file locks are local-filesystem only and do not cross
/// machines — see Phase 2 for the cross-machine lease).
struct AppState {
    db_path: PathBuf,
    local_lock: Mutex<()>,
    /// Filesystem watcher on the DB path. Kept in state so it lives for the
    /// app's lifetime; dropping it would stop watching for remote iCloud
    /// updates.
    watcher: Mutex<notify::RecommendedWatcher>,
}

// ---------------------------------------------------------------------------
// Data models
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Account {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub account_type: String,
    pub institution: String,
    pub currency: String,
    pub is_manual: bool,
    pub is_ignored: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AccountValue {
    pub id: i64,
    pub account_id: String,
    pub balance: f64,
    pub recorded_date: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AccountSummary {
    pub account: Account,
    pub latest_balance: Option<f64>,
    pub latest_date: Option<String>,
    pub max_balance: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AccountHistory {
    pub account: Account,
    pub values: Vec<AccountValue>,
    pub current_balance: Option<f64>,
    pub min_balance: Option<f64>,
    pub max_balance: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct AddManualAccountRequest {
    pub name: String,
    pub account_type: String,
    pub institution: String,
}

#[derive(Debug, Deserialize)]
pub struct AddManualValueRequest {
    pub account_id: String,
    pub balance: f64,
}

/// Return value for `delete_last_manual_value`: describes the row that was
/// removed, so the UI can confirm what was undone.
#[derive(Debug, Serialize)]
pub struct DeletedValue {
    pub balance: f64,
    pub recorded_date: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct AccountChange {
    pub period: String,
    pub previous_balance: Option<f64>,
    pub previous_date: Option<String>,
    pub change_amount: Option<f64>,
    pub change_percent: Option<f64>,
}

// ---------------------------------------------------------------------------
// Database setup
// ---------------------------------------------------------------------------

fn init_db(conn: &Connection) {
    conn.execute_batch(
        "
        -- Pin DELETE journal mode. WAL would create -wal/-shm sidecars
        -- that sync independently across iCloud and corrupt the DB. This is
        -- a guardrail — we are already on delete mode, but set it explicitly
        -- so a re-open on a fresh machine or a future change can't flip it.
        PRAGMA journal_mode=DELETE;
        PRAGMA synchronous=NORMAL;
        PRAGMA locking_mode=NORMAL;

        CREATE TABLE IF NOT EXISTS accounts (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            type TEXT NOT NULL DEFAULT '',
            institution TEXT NOT NULL DEFAULT '',
            currency TEXT NOT NULL DEFAULT 'NZD',
            is_manual INTEGER NOT NULL DEFAULT 0,
            is_ignored INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS account_values (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            account_id TEXT NOT NULL,
            balance REAL NOT NULL,
            recorded_date TEXT NOT NULL,
            FOREIGN KEY (account_id) REFERENCES accounts(id)
        );

        CREATE TABLE IF NOT EXISTS fetch_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            fetched_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_account_values_account_id
            ON account_values(account_id);

        CREATE INDEX IF NOT EXISTS idx_account_values_date
            ON account_values(recorded_date);
        ",
    )
    .expect("Failed to initialize database");
}

// ---------------------------------------------------------------------------
// iCloud-synced DB path resolution, conflict reconciliation, open helpers
// ---------------------------------------------------------------------------

/// Resolve where the SQLite DB lives.
///
/// Prefers iCloud Drive (CloudDocs) so the same dataset is available on every
/// Mac signed into the same iCloud account:
///   ~/Library/Mobile Documents/com~apple~CloudDocs/my_finances/my_finances.db
///
/// Falls back to the legacy local path when iCloud Drive isn't available
/// (not signed in, no CloudDocs dir, or the iCloud daemon hasn't created it):
///   ~/Library/Application Support/my_finances.db
///
/// This is the single place the iCloud decision lives; upgrading to the app's
/// private ubiquity container (entitlement-gated) is a one-function change.
fn resolve_db_path() -> PathBuf {
    // Detect iCloud Drive (CloudDocs) presence. We check the CloudDocs *root*,
    // not the `my_finances` subfolder — the subfolder is created by us (below)
    // or by `migrate_legacy_into_icloud`, so checking for it would be a
    // chicken-and-egg bug: it would never exist on first run, the app would
    // fall back to the local path, and migration (which creates it) would
    // never be invoked with an iCloud path.
    if let Some(home) = dirs_next::home_dir() {
        let icloud_root = home
            .join("Library")
            .join("Mobile Documents")
            .join("com~apple~CloudDocs");
        if icloud_root.is_dir() {
            let icloud_dir = icloud_root.join("my_finances");
            // Ensure the app subfolder exists so the live DB and conflict
            // copies have somewhere to live.
            std::fs::create_dir_all(&icloud_dir).ok();
            return icloud_dir.join("my_finances.db");
        }
    }

    // Fallback: legacy local Application Support path (used when iCloud Drive
    // is not signed in or the CloudDocs folder is absent).
    let db_dir = dirs_next::data_dir().unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&db_dir).ok();
    db_dir.join("my_finances.db")
}

/// Legacy local DB path (used only for one-time migration into iCloud).
fn legacy_local_db_path() -> PathBuf {
    let db_dir = dirs_next::data_dir().unwrap_or_else(|| PathBuf::from("."));
    db_dir.join("my_finances.db")
}

/// Detect iCloud conflict copies next to the live DB and **row-level union-merge**
/// them into the live DB, then archive the conflict copy into `conflicts/`.
///
/// When two offline machines both write and reconnect (or two online machines
/// race past the iCloud sync window), iCloud produces conflict copies named
/// like `my_finances (stuartg's Mac 2).db`. Phase 1 resolved these lossily
/// (newest-mtime-wins, loser archived). Phase 2A merges losslessly: every row
/// present in either DB is preserved in the live DB. On collisions the live
/// row wins; the conflict copy is archived whole to `conflicts/` so no data is
/// ever thrown away.
///
/// This runs on a read-write connection (it writes), so it is called from
/// `open_db_rw` only — never from the read path. Reads may be momentarily stale
/// until the next write or startup triggers a merge; that is acceptable.
fn merge_conflict_copies(conn: &Connection, live_path: &std::path::Path) {
    let dir = match live_path.parent() {
        Some(d) => d,
        None => return,
    };
    let live_name = match live_path.file_name().and_then(|s| s.to_str()) {
        Some(n) => n.to_string(),
        None => return,
    };
    // Match files that start with the live stem and end with .db but are not
    // the live file itself (iCloud names conflicts `my_finances (…).db`).
    let stem = live_name.trim_end_matches(".db");

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let conflicts_dir = dir.join("conflicts");

    for entry in entries.flatten() {
        let name = match entry.file_name().to_str() {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !name.ends_with(".db") || name == live_name || !name.starts_with(stem) {
            continue;
        }
        let conflict_path = entry.path();

        // Union-merge this conflict copy into the live DB. Only archive on
        // success: if the merge failed, leave the file in place so the next
        // write retries. Archiving unmerged data would lose it.
        if let Err(e) = merge_one_conflict(conn, &conflict_path) {
            eprintln!(
                "[my_finances] Failed to merge conflict '{}': {}. Leaving it in place; will retry next write.",
                name, e
            );
            continue;
        }
        eprintln!(
            "[my_finances] Merged iCloud conflict '{}' into live DB (lossless union).",
            name
        );

        std::fs::create_dir_all(&conflicts_dir).ok();
        let ts = chrono::Local::now().format("%Y%m%dT%H%M%S").to_string();
        let mut archive_name = format!("{}-{}.db", stem, ts);
        let mut archive_path = conflicts_dir.join(&archive_name);
        // Disambiguate if multiple conflicts archive within the same second.
        let mut suffix = 2;
        while archive_path.exists() {
            archive_name = format!("{}-{}-{}.db", stem, ts, suffix);
            archive_path = conflicts_dir.join(&archive_name);
            suffix += 1;
        }
        if let Err(e) = std::fs::rename(&conflict_path, &archive_path) {
            eprintln!(
                "[my_finances] Could not archive conflict '{}' to conflicts/{}: {}",
                name, archive_name, e
            );
        }
    }
}

/// Union one conflict copy into the live connection via `ATTACH`. Every table
/// is merged by its natural key; on key collision the live row is kept. The
/// caller archives the conflict file afterward.
///
/// Note on virtual totals: `__total__`/`__assets_total__`/`__liabilities_total__`
/// rows live in `account_values` and are unioned like every other row (live's
/// value for a given date wins on collision; conflict's is added where live has
/// none). We deliberately do NOT recompute historical totals here: `recompute_total`
/// derives the total from each account's *latest* balance (MAX(id) across all
/// dates), not a date-specific balance, so recomputing a historical date would
/// store today's total under that past date and corrupt the graph. Today's total
/// is recomputed fresh by the normal fetch/toggle/manual write paths that call
/// this merge via `open_db_rw`.
fn merge_one_conflict(conn: &Connection, conflict_path: &std::path::Path) -> Result<(), rusqlite::Error> {
    let path_str = conflict_path.to_str().unwrap_or("");
    // Safety: if a previous iteration failed after ATTACH but before DETACH,
    // `other` would still be attached and our ATTACH below would error. Detach
    // first (ignore the error when nothing is attached).
    let _ = conn.execute("DETACH DATABASE other", []);
    conn.execute("ATTACH DATABASE ?1 AS other", rusqlite::params![path_str])?;

    // accounts: union by id, keep live on collision.
    conn.execute(
        "INSERT OR IGNORE INTO accounts
           SELECT id, name, type, institution, currency, is_manual, is_ignored
           FROM other.accounts",
        [],
    )?;

    // account_values: union by (account_id, recorded_date). On collision keep
    // live. Same-day real-account values come from the same Akahu source so are
    // identical; virtual totals are preserved per-date as described above.
    conn.execute(
        "INSERT INTO account_values (account_id, balance, recorded_date)
           SELECT av.account_id, av.balance, av.recorded_date
           FROM other.account_values av
           WHERE NOT EXISTS (
             SELECT 1 FROM account_values lv
             WHERE lv.account_id = av.account_id AND lv.recorded_date = av.recorded_date
           )",
        [],
    )?;

    // fetch_log: union by fetched_at.
    conn.execute(
        "INSERT INTO fetch_log (fetched_at)
           SELECT fetched_at FROM other.fetch_log fl
           WHERE NOT EXISTS (SELECT 1 FROM fetch_log WHERE fetched_at = fl.fetched_at)",
        [],
    )?;

    // settings: union by key, keep live on collision.
    conn.execute("INSERT OR IGNORE INTO settings SELECT key, value FROM other.settings", [])?;

    conn.execute("DETACH DATABASE other", [])?;
    Ok(())
}

/// Open the DB read-write, ensuring schema and virtual totals exist, and
/// merging any iCloud conflict copies into the live DB. Opens a fresh
/// connection each call — callers must drop it to release the file handle so
/// iCloud can sync.
fn open_db_rw(path: &std::path::Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|e| format!("Failed to open database: {}", e))?;
    init_db(&conn);
    // Merge runs on the open RW connection (it writes). Skipped if no conflicts
    // are present (cheap read_dir scan).
    merge_conflict_copies(&conn, path);
    seed_total_if_missing(&conn);
    Ok(conn)
}

/// Open the DB read-only for pure-read commands. Does not create schema and
/// does NOT merge (the merge writes; reads must not). A read may be momentarily
/// stale if a conflict just appeared and no write/startup has merged yet —
/// acceptable, the next write or launch reconciles.
fn open_db_ro(path: &std::path::Path) -> Result<Connection, String> {
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    Connection::open_with_flags(path, flags)
        .map_err(|e| format!("Failed to open database read-only: {}", e))
}

/// Acquire the same-machine lock and hand a fresh read-write connection to a
/// closure. The lock + connection are released when the closure returns.
fn with_db_rw<T, F>(state: &AppState, f: F) -> Result<T, String>
where
    F: FnOnce(&Connection) -> Result<T, String>,
{
    let _guard = state.local_lock.lock().map_err(|e| e.to_string())?;
    let conn = open_db_rw(&state.db_path)?;
    f(&conn)
}

/// Acquire the same-machine lock and hand a fresh read-only connection to a
/// closure. The lock + connection are released when the closure returns.
fn with_db_ro<T, F>(state: &AppState, f: F) -> Result<T, String>
where
    F: FnOnce(&Connection) -> Result<T, String>,
{
    let _guard = state.local_lock.lock().map_err(|e| e.to_string())?;
    let conn = open_db_ro(&state.db_path)?;
    f(&conn)
}

/// One-time migration: copy the legacy local DB into iCloud on first launch
/// after this change ships. Leaves the old file in place as a backup.
///
/// Guarded by a `migrated_to_icloud` row in the `settings` table so it only
/// runs once per iCloud account (the marker syncs with the DB).
fn migrate_legacy_into_icloud(icloud_path: &std::path::Path) {
    if icloud_path.exists() {
        // iCloud copy already present (another machine migrated, or already done).
        return;
    }
    let legacy = legacy_local_db_path();
    if !legacy.exists() {
        return;
    }
    // iCloud chosen but no iCloud copy yet and a legacy local DB exists -> copy.
    if let Some(parent) = icloud_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::copy(&legacy, icloud_path) {
        Ok(_) => {
            // Mark migrated on the new copy so this never re-runs/clobbers.
            if let Ok(conn) = Connection::open(icloud_path) {
                let _ = conn.execute(
                    "INSERT OR REPLACE INTO settings (key, value) VALUES ('migrated_to_icloud', 'true')",
                    [],
                );
            }
            eprintln!(
                "[my_finances] Migrated legacy DB from {} into iCloud at {} (old file kept as backup)",
                legacy.display(),
                icloud_path.display()
            );
        }
        Err(e) => {
            eprintln!(
                "[my_finances] Migration copy failed ({} -> {}): {}. Falling back; will retry next launch.",
                legacy.display(),
                icloud_path.display(),
                e
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Akahu API helpers
// ---------------------------------------------------------------------------

const KEYRING_SERVICE: &str = "my-finances-akahu";

fn get_akahu_headers() -> Result<(String, String), String> {
    // Try macOS keychain first
    let kr = keyring_core::Entry::new(KEYRING_SERVICE, "akahu_app_id").map_err(|e| e.to_string())?;
    let app_id = kr.get_password().ok();
    let kr = keyring_core::Entry::new(KEYRING_SERVICE, "akahu_user_token").map_err(|e| e.to_string())?;
    let user_token = kr.get_password().ok();

    match (app_id, user_token) {
        (Some(id), Some(token)) => Ok((id, token)),
        _ => {
            // Fall back to env vars
            let id = std::env::var("AKAHU_APP_ID")
                .map_err(|_| "Akahu credentials not found. Go to Settings to add them.".to_string())?;
            let token = std::env::var("AKAHU_USER_TOKEN")
                .map_err(|_| "Akahu credentials not found. Go to Settings to add them.".to_string())?;
            Ok((id, token))
        }
    }
}

#[derive(Debug, Deserialize)]
struct AkahuApiResponse<T> {
    success: bool,
    items: Option<Vec<T>>,
    item: Option<T>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AkahuAccount {
    #[serde(rename = "_id")]
    id: String,
    name: String,
    #[serde(rename = "type")]
    account_type: Option<String>,
    connection: Option<AkahuConnection>,
    balance: Option<AkahuBalance>,
}

#[derive(Debug, Deserialize)]
struct AkahuConnection {
    institution: Option<AkahuInstitution>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AkahuInstitution {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AkahuBalance {
    current: Option<f64>,
    currency: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AkahuRefreshItem {
    #[serde(rename = "_id")]
    id: String,
    status: Option<String>,
}

async fn trigger_akahu_refresh(
    app_id: &str,
    user_token: &str,
) -> Result<Option<String>, String> {
    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.akahu.io/v1/refresh")
        .header("Authorization", format!("Bearer {}", user_token))
        .header("X-Akahu-Id", app_id)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("HTTP error triggering refresh: {}", e))?;

    // Check HTTP status first
    let status = resp.status();
    let body_text = resp.text().await.map_err(|e| format!("Failed to read refresh response: {}", e))?;

    if !status.is_success() {
        return Err(format!("Akahu refresh returned HTTP {}: {}", status.as_u16(), truncate_str(&body_text, 200)));
    }

    let data: AkahuApiResponse<AkahuRefreshItem> =
        serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse refresh response: {}. Body: {}", e, truncate_str(&body_text, 200)))?;

    if !data.success {
        return Err(data.message.unwrap_or_else(|| "Unknown error".to_string()));
    }

    // If there's no item, there's nothing to refresh — that's fine
    Ok(data.item.map(|r| r.id))
}

async fn wait_for_refresh(
    app_id: &str,
    user_token: &str,
    refresh_id: &str,
    timeout_secs: u64,
) -> Result<bool, String> {
    let client = reqwest::Client::new();
    let start = std::time::Instant::now();

    loop {
        if start.elapsed().as_secs() >= timeout_secs {
            return Ok(false);
        }

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let resp = client
            .get(format!("https://api.akahu.io/v1/refresh/{}", refresh_id))
            .header("Authorization", format!("Bearer {}", user_token))
            .header("X-Akahu-Id", app_id)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| format!("HTTP error checking refresh: {}", e))?;

        let status = resp.status();
        let body_text = resp.text().await.map_err(|e| format!("Failed to read refresh status: {}", e))?;

        if !status.is_success() {
            return Err(format!("Refresh status HTTP {}: {}", status.as_u16(), truncate_str(&body_text, 200)));
        }

        let data: AkahuApiResponse<AkahuRefreshItem> =
            serde_json::from_str(&body_text)
                .map_err(|e| format!("Failed to parse refresh status: {}. Body: {}", e, truncate_str(&body_text, 200)))?;

        if !data.success {
            return Err(data.message.unwrap_or_else(|| "Unknown error".to_string()));
        }

        match data.item.and_then(|r| r.status) {
            Some(ref status) if status == "COMPLETED" => return Ok(true),
            Some(ref status) if status == "ERROR" => return Err("Refresh failed with error".to_string()),
            _ => {} // still in progress, loop
        }
    }
}

async fn fetch_akahu_accounts(
    app_id: &str,
    user_token: &str,
) -> Result<Vec<AkahuAccount>, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://api.akahu.io/v1/accounts")
        .header("Authorization", format!("Bearer {}", user_token))
        .header("X-Akahu-Id", app_id)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("HTTP error fetching accounts: {}", e))?;

    let status = resp.status();
    let body_text = resp.text().await.map_err(|e| format!("Failed to read accounts response: {}", e))?;

    if !status.is_success() {
        return Err(format!("Akahu accounts returned HTTP {}: {}", status.as_u16(), truncate_str(&body_text, 200)));
    }

    let data: AkahuApiResponse<AkahuAccount> =
        serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse accounts response: {}. Body: {}", e, truncate_str(&body_text, 200)))?;

    if !data.success {
        return Err(data.message.unwrap_or_else(|| "Unknown error".to_string()));
    }

    Ok(data.items.unwrap_or_default())
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
async fn fetch_akahu_balances(state: State<'_, AppState>) -> Result<String, String> {
    let (app_id, user_token) = get_akahu_headers()?;

    // Check if we already fetched today (short read; lock released before the network await).
    let today = Local::now().format("%Y-%m-%d").to_string();
    {
        let already_fetched = with_db_ro(&state, |db| {
            let already: bool = db
                .query_row(
                    "SELECT COUNT(*) > 0 FROM fetch_log WHERE fetched_at = ?1",
                    rusqlite::params![today],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?;
            Ok(already)
        })?;
        if already_fetched {
            return Ok("Already fetched today".to_string());
        }
    }

    // Trigger refresh (skip if nothing to refresh)
    if let Some(refresh_id) = trigger_akahu_refresh(&app_id, &user_token).await? {
        let completed = wait_for_refresh(&app_id, &user_token, &refresh_id, 120).await?;
        if !completed {
            return Err("Refresh timed out".to_string());
        }
    }

    // Fetch accounts
    let accounts = fetch_akahu_accounts(&app_id, &user_token).await?;

    // Filter NZD only
    let nzd_accounts: Vec<&AkahuAccount> = accounts
        .iter()
        .filter(|a| {
            a.balance
                .as_ref()
                .and_then(|b| b.currency.as_deref())
                .unwrap_or("NZD")
                == "NZD"
        })
        .collect();

    // Save to database (tight write critical section; no lock held during the network above).
    with_db_rw(&state, |db| {
        for acc in &nzd_accounts {
            let inst_name = acc
                .connection
                .as_ref()
                .and_then(|c| c.institution.as_ref())
                .and_then(|i| i.name.as_deref())
                .or_else(|| {
                    acc.connection
                        .as_ref()
                        .and_then(|c| c.name.as_deref())
                })
                .unwrap_or("Unknown");

            let acc_type = acc.account_type.as_deref().unwrap_or("Unknown");
            let balance = acc
                .balance
                .as_ref()
                .and_then(|b| b.current)
                .unwrap_or(0.0);

            // Upsert account
            db.execute(
                "INSERT INTO accounts (id, name, type, institution, currency, is_manual, is_ignored)
                 VALUES (?1, ?2, ?3, ?4, 'NZD', 0, 0)
                 ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name,
                    type = excluded.type,
                    institution = excluded.institution
                 ",
                rusqlite::params![acc.id, acc.name, acc_type, inst_name],
            )
            .map_err(|e| e.to_string())?;

            // Upsert today's account value (idempotent per (account, day)) so a
            // same-day re-fetch on another machine overwrites rather than
            // duplicating — keeps the merge lossless even without a lock.
            db.execute(
                "DELETE FROM account_values WHERE account_id = ?1 AND recorded_date = ?2",
                rusqlite::params![acc.id, today],
            )
            .map_err(|e| e.to_string())?;
            db.execute(
                "INSERT INTO account_values (account_id, balance, recorded_date)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![acc.id, balance, today],
            )
            .map_err(|e| e.to_string())?;
        }

        // ---- Virtual total account ----
        recompute_total(db, &today).map_err(|e| e.to_string())?;
        // ---- End virtual total ----

        // Log the fetch (idempotent: same-day re-fetch on another machine
        // must not add a duplicate fetch_log row, which would make the
        // fetch-log UNION merge misbehave).
        db.execute(
            "DELETE FROM fetch_log WHERE fetched_at = ?1",
            rusqlite::params![today],
        )
        .map_err(|e| e.to_string())?;
        db.execute(
            "INSERT INTO fetch_log (fetched_at) VALUES (?1)",
            rusqlite::params![today],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    })?;

    Ok(format!("Fetched {} NZD accounts", nzd_accounts.len()))
}

#[tauri::command]
fn get_accounts_summary(state: State<'_, AppState>) -> Result<Vec<AccountSummary>, String> {
    with_db_ro(&state, |db| {
        let mut stmt = db
            .prepare(
                "SELECT a.id, a.name, a.type, a.institution, a.currency, a.is_manual, a.is_ignored,
                        av.balance, av.recorded_date,
                        (SELECT MAX(balance) FROM account_values WHERE account_id = a.id) AS max_balance
                 FROM accounts a
                 LEFT JOIN (
                     SELECT account_id, balance, recorded_date
                     FROM account_values
                     WHERE id IN (
                         SELECT MAX(id) FROM account_values GROUP BY account_id
                     )
                 ) av ON a.id = av.account_id
                 WHERE a.is_ignored = 0
                 ORDER BY CASE WHEN a.id = '__total__' THEN 0
                               WHEN a.id = '__assets_total__' THEN 1
                               WHEN a.id = '__liabilities_total__' THEN 2
                               ELSE 3 END,
                          a.institution, a.name",
            )
            .map_err(|e| e.to_string())?;

        let summaries = stmt
            .query_map([], |row| {
                Ok(AccountSummary {
                    account: Account {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        account_type: row.get(2)?,
                        institution: row.get(3)?,
                        currency: row.get(4)?,
                        is_manual: row.get::<_, i32>(5)? != 0,
                        is_ignored: row.get::<_, i32>(6)? != 0,
                    },
                    latest_balance: row.get(7)?,
                    latest_date: row.get(8)?,
                    max_balance: row.get(9)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        Ok(summaries)
    })
}

#[tauri::command]
fn get_account_history(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<AccountHistory, String> {
    with_db_ro(&state, |db| {
        // Get account info
        let account = db
            .query_row(
                "SELECT id, name, type, institution, currency, is_manual, is_ignored
                 FROM accounts WHERE id = ?1",
                rusqlite::params![account_id],
                |row| {
                    Ok(Account {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        account_type: row.get(2)?,
                        institution: row.get(3)?,
                        currency: row.get(4)?,
                        is_manual: row.get::<_, i32>(5)? != 0,
                        is_ignored: row.get::<_, i32>(6)? != 0,
                    })
                },
            )
            .map_err(|e| format!("Account not found: {}", e))?;

        // Get values ordered by date
        let mut stmt = db
            .prepare(
                "SELECT id, account_id, balance, recorded_date
                 FROM account_values
                 WHERE account_id = ?1
                 ORDER BY recorded_date ASC",
            )
            .map_err(|e| e.to_string())?;

        let values: Vec<AccountValue> = stmt
            .query_map(rusqlite::params![account_id], |row| {
                Ok(AccountValue {
                    id: row.get(0)?,
                    account_id: row.get(1)?,
                    balance: row.get(2)?,
                    recorded_date: row.get(3)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        let current_balance = values.last().map(|v| v.balance);
        let min_balance = values.iter().map(|v| v.balance).fold(f64::INFINITY, f64::min);
        let max_balance = values.iter().map(|v| v.balance).fold(f64::NEG_INFINITY, f64::max);

        Ok(AccountHistory {
            account,
            values,
            current_balance,
            min_balance: if min_balance.is_finite() {
                Some(min_balance)
            } else {
                None
            },
            max_balance: if max_balance.is_finite() {
                Some(max_balance)
            } else {
                None
            },
        })
    })
}

#[tauri::command]
fn get_all_accounts_config(state: State<'_, AppState>) -> Result<Vec<Account>, String> {
    with_db_ro(&state, |db| {
        let mut stmt = db
            .prepare(
                "SELECT id, name, type, institution, currency, is_manual, is_ignored
                 FROM accounts
                 WHERE id NOT IN ('__total__', '__assets_total__', '__liabilities_total__')
                 ORDER BY is_manual, institution, name",
            )
            .map_err(|e| e.to_string())?;

        let accounts = stmt
            .query_map([], |row| {
                Ok(Account {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    account_type: row.get(2)?,
                    institution: row.get(3)?,
                    currency: row.get(4)?,
                    is_manual: row.get::<_, i32>(5)? != 0,
                    is_ignored: row.get::<_, i32>(6)? != 0,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        Ok(accounts)
    })
}

#[tauri::command]
fn toggle_ignore_account(state: State<'_, AppState>, account_id: String) -> Result<bool, String> {
    if account_id.starts_with("__") {
        return Err("Cannot ignore virtual accounts".to_string());
    }

    let today = Local::now().format("%Y-%m-%d").to_string();
    let new_value = with_db_rw(&state, |db| {
        // Toggle the is_ignored flag
        db.execute(
            "UPDATE accounts SET is_ignored = CASE WHEN is_ignored = 0 THEN 1 ELSE 0 END
             WHERE id = ?1",
            rusqlite::params![account_id],
        )
        .map_err(|e| e.to_string())?;

        // Return the new value
        let new_value: bool = db
            .query_row(
                "SELECT is_ignored != 0 FROM accounts WHERE id = ?1",
                rusqlite::params![account_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;

        // Recalculate the virtual total
        recompute_total(db, &today).ok();

        Ok(new_value)
    })?;

    Ok(new_value)
}

#[tauri::command]
fn add_manual_account(
    state: State<'_, AppState>,
    request: AddManualAccountRequest,
) -> Result<Account, String> {
    let account = with_db_rw(&state, |db| {
        // Generate a unique ID for the manual account
        let id = format!("manual_{}", uuid::Uuid::new_v4());

        db.execute(
            "INSERT INTO accounts (id, name, type, institution, currency, is_manual, is_ignored)
             VALUES (?1, ?2, ?3, ?4, 'NZD', 1, 0)",
            rusqlite::params![id, request.name, request.account_type, request.institution],
        )
        .map_err(|e| e.to_string())?;

        Ok(Account {
            id,
            name: request.name,
            account_type: request.account_type,
            institution: request.institution,
            currency: "NZD".to_string(),
            is_manual: true,
            is_ignored: false,
        })
    })?;

    Ok(account)
}

#[tauri::command]
fn add_manual_account_value(
    state: State<'_, AppState>,
    request: AddManualValueRequest,
) -> Result<(), String> {
    let today = Local::now().format("%Y-%m-%d").to_string();
    with_db_rw(&state, |db| {
        // Verify it's a manual account
        let is_manual: bool = db
            .query_row(
                "SELECT is_manual != 0 FROM accounts WHERE id = ?1",
                rusqlite::params![request.account_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Account not found: {}", e))?;

        if !is_manual {
            return Err("Can only add values to manual accounts".to_string());
        }

        db.execute(
            "INSERT INTO account_values (account_id, balance, recorded_date)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![request.account_id, request.balance, today],
        )
        .map_err(|e| e.to_string())?;

        // Recalculate the virtual total
        recompute_total(db, &today).ok();

        Ok(())
    })?;

    Ok(())
}

/// Delete the most-recently-added value row for a manual account (the row with
/// the highest `id` for that account — i.e. the last one the user entered).
/// Used to undo a typo. Recomputes virtual totals afterwards so cards/graphs
/// reflect the deletion. Returns the deleted row so the UI can confirm what
/// was removed, or `None` if the account had no values to delete.
#[tauri::command]
fn delete_last_manual_value(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<Option<DeletedValue>, String> {
    let today = Local::now().format("%Y-%m-%d").to_string();
    with_db_rw(&state, |db| {
        // Verify it's a manual account.
        let is_manual: bool = db
            .query_row(
                "SELECT is_manual != 0 FROM accounts WHERE id = ?1",
                rusqlite::params![account_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Account not found: {}", e))?;

        if !is_manual {
            return Err("Can only delete values from manual accounts".to_string());
        }

        // Find the most-recently-inserted row (MAX(id)) for this account.
        let row: Option<(i64, f64, String)> = db
            .query_row(
                "SELECT id, balance, recorded_date
                 FROM account_values
                 WHERE account_id = ?1
                 ORDER BY id DESC
                 LIMIT 1",
                rusqlite::params![account_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .ok();

        let Some((row_id, balance, recorded_date)) = row else {
            return Ok(None); // nothing to delete
        };

        db.execute(
            "DELETE FROM account_values WHERE id = ?1",
            rusqlite::params![row_id],
        )
        .map_err(|e| e.to_string())?;

        // Recalculate the virtual total so cards/graphs reflect the deletion.
        recompute_total(db, &today).ok();

        Ok(Some(DeletedValue {
            balance,
            recorded_date,
        }))
    })
}

#[tauri::command]
fn get_account_changes(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<Vec<AccountChange>, String> {
    with_db_ro(&state, |db| {
        // Get the latest value for this account
        let current_balance: Option<f64> = db
            .query_row(
                "SELECT balance
                 FROM account_values
                 WHERE account_id = ?1
                 ORDER BY recorded_date DESC
                 LIMIT 1",
                rusqlite::params![account_id],
                |row| row.get(0),
            )
            .ok();

        // Find the earliest recorded date for this account
        let earliest_date: Option<String> = db
            .query_row(
                "SELECT recorded_date
                 FROM account_values
                 WHERE account_id = ?1
                 ORDER BY recorded_date ASC
                 LIMIT 1",
                rusqlite::params![account_id],
                |row| row.get(0),
            )
            .ok();

        let today = Local::now().format("%Y-%m-%d").to_string();

        let periods: Vec<(&str, i64)> = vec![
            ("30d", 30),
            ("180d", 180),
            ("360d", 360),
        ];

        let mut results = Vec::new();

        for (label, days) in periods {
            let lookback_date = chrono::NaiveDate::parse_from_str(&today, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.checked_sub_signed(chrono::Duration::days(days)))
                .map(|d| d.format("%Y-%m-%d").to_string());

            let (prev_balance, prev_date): (Option<f64>, Option<String>) =
                if let Some(ref lb) = lookback_date {
                    // Check if the earliest measurement is after the lookback date
                    let before_measurements = earliest_date
                        .as_ref()
                        .map(|e| e.as_str() > lb.as_str())
                        .unwrap_or(true);

                    if before_measurements {
                        (None, None)
                    } else {
                        // Get the value closest to (≤) the lookback date
                        db.query_row(
                            "SELECT balance, recorded_date
                             FROM account_values
                             WHERE account_id = ?1 AND recorded_date <= ?2
                             ORDER BY recorded_date DESC
                             LIMIT 1",
                            rusqlite::params![account_id, lb],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .unwrap_or((None, None))
                    }
                } else {
                    (None, None)
                };

            let change = match (current_balance, prev_balance) {
                (Some(cur), Some(prev)) if prev != 0.0 => {
                    let amount = cur - prev;
                    // Use the magnitude of the previous balance as the base so
                    // that liabilities (stored as negative balances) show a
                    // percentage whose sign matches the dollar change: paying
                    // down a debt is an improvement (positive change), not a
                    // negative one. For assets (prev > 0) `prev.abs()` is a
                    // no-op, so their displayed percentage is unchanged.
                    let percent = (amount / prev.abs()) * 100.0;
                    (Some(amount), Some(percent))
                }
                (Some(cur), Some(prev)) => (Some(cur - prev), None),
                _ => (None, None),
            };

            results.push(AccountChange {
                period: label.to_string(),
                previous_balance: prev_balance,
                previous_date: prev_date,
                change_amount: change.0,
                change_percent: change.1,
            });
        }

        Ok(results)
    })
}

// ---------------------------------------------------------------------------
// Keychain credential commands
// ---------------------------------------------------------------------------

#[tauri::command]
fn has_akahu_credentials() -> Result<bool, String> {
    let kr = keyring_core::Entry::new(KEYRING_SERVICE, "akahu_app_id").map_err(|e| e.to_string())?;
    let has_app_id = kr.get_password().is_ok();
    let kr = keyring_core::Entry::new(KEYRING_SERVICE, "akahu_user_token").map_err(|e| e.to_string())?;
    let has_token = kr.get_password().is_ok();
    Ok(has_app_id && has_token)
}

#[tauri::command]
fn save_akahu_credentials(app_id: String, user_token: String) -> Result<(), String> {
    let kr = keyring_core::Entry::new(KEYRING_SERVICE, "akahu_app_id").map_err(|e| e.to_string())?;
    kr.set_password(&app_id).map_err(|e| format!("Failed to save App ID: {}", e))?;
    let kr = keyring_core::Entry::new(KEYRING_SERVICE, "akahu_user_token").map_err(|e| e.to_string())?;
    kr.set_password(&user_token).map_err(|e| format!("Failed to save User Token: {}", e))?;
    Ok(())
}

#[tauri::command]
fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
fn get_autofetch_setting(state: State<'_, AppState>) -> Result<bool, String> {
    with_db_ro(&state, |db| {
        let value: String = db
            .query_row(
                "SELECT value FROM settings WHERE key = 'autofetch'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "false".to_string());
        Ok(value == "true")
    })
}

#[tauri::command]
fn set_autofetch_setting(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    with_db_rw(&state, |db| {
        db.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('autofetch', ?1)",
            rusqlite::params![if enabled { "true" } else { "false" }],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Backups (sub-phase 1)
// ---------------------------------------------------------------------------
//
// Backups are point-in-time snapshots of the whole database, produced via
// `VACUUM INTO` (a clean, compacted copy) and then gzipped to `.db.gz`.
// They are stored LOCALLY (Application Support by default), deliberately NOT
// in iCloud alongside the live DB — a backup's job is to survive data loss,
// which means living on a different failure-domain than the primary. They are
// per-machine and not synced. Retention is "keep last N" where N depends on
// the configured frequency.

const BACKUP_PREFIX: &str = "my_finances-";
const BACKUP_SUFFIX: &str = ".db.gz";

/// Settings surfaced to the frontend.
#[derive(Debug, Serialize, Deserialize, Clone)]
struct BackupSettings {
    frequency: String, // "off" | "daily" | "weekly" | "monthly"
    last_backup_at: Option<String>, // ISO-8601 UTC, or None if never
    backup_dir: String, // resolved absolute path actually in use
    is_default_dir: bool, // true when no custom dir set (backup_dir is the default)
}

/// Resolve the backups directory. Honors a `backup_dir` setting if present and
/// usable; otherwise falls back to the default `<app_support>/backups/`.
/// Always returns a path (the default) even if the custom dir is invalid —
/// callers that need it usable should `create_dir_all` before writing.
fn resolve_backups_dir(state: &AppState) -> PathBuf {
    // Try the custom setting first.
    if let Ok(custom) = with_db_ro(state, |db| {
        let v: Option<String> = db
            .query_row(
                "SELECT value FROM settings WHERE key = 'backup_dir'",
                [],
                |row| row.get(0),
            )
            .ok();
        Ok(v)
    }) {
        if let Some(p) = custom {
            let pb = PathBuf::from(&p);
            if pb.is_absolute() {
                return pb;
            }
        }
    }
    // Default: <app support>/my_finances/backups/
    let dir = dirs_next::data_dir().unwrap_or_else(|| PathBuf::from("."));
    dir.join("my_finances").join("backups")
}

/// Number of backups to retain for a given frequency.
fn backup_keep_count(frequency: &str) -> usize {
    match frequency {
        "daily" => 30,
        "weekly" => 12,
        "monthly" => 24,
        _ => 0, // "off" — don't prune
    }
}

/// Seconds between backups for a given frequency.
fn backup_interval_secs(frequency: &str) -> Option<i64> {
    match frequency {
        "daily" => Some(86_400),
        "weekly" => Some(604_800),
        "monthly" => Some(2_592_000), // 30 days
        _ => None, // "off"
    }
}

/// Snapshot the live DB to `target` via `VACUUM INTO` (clean, compacted copy).
/// Opens the source read-only; VACUUM INTO does not mutate the source.
fn snapshot_db(live_path: &std::path::Path, target: &std::path::Path) -> Result<(), String> {
 let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
        | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(live_path, flags)
        .map_err(|e| format!("Failed to open DB for snapshot: {}", e))?;
    let target_str = target
        .to_str()
        .ok_or_else(|| "Backup temp path is not valid UTF-8".to_string())?;
    conn.execute_batch(&format!("VACUUM INTO '{}';", target_str.replace('"', "")))
        .map_err(|e| format!("VACUUM INTO failed: {}", e))?;
    Ok(())
}

/// Gzip a source file into `target` (a `.db.gz`), then delete the source.
fn gzip_file(src: &std::path::Path, target: &std::path::Path) -> Result<(), String> {
    let input = fs::read(src).map_err(|e| format!("Failed to read snapshot: {}", e))?;
    let out = fs::File::create(target)
        .map_err(|e| format!("Failed to create backup file: {}", e))?;
    let mut enc = GzEncoder::new(out, Compression::default());
    enc.write_all(&input)
        .map_err(|e| format!("Failed to compress backup: {}", e))?;
    enc.finish()
        .map_err(|e| format!("Failed to finalize backup: {}", e))?;
    let _ = fs::remove_file(src);
    Ok(())
}

/// Delete the oldest backups beyond the keep-count for `frequency`.
/// Only touches files matching `my_finances-*.db.gz`. Never deletes anything
/// when frequency is "off".
fn prune_backups(dir: &std::path::Path, frequency: &str) {
    let keep = backup_keep_count(frequency);
    if keep == 0 {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    // Collect (path, mtime) for files matching our backup naming.
    let mut found: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    for entry in entries.flatten() {
        let name = match entry.file_name().to_str() {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !name.starts_with(BACKUP_PREFIX) || !name.ends_with(BACKUP_SUFFIX) {
            continue;
        }
        let mtime = match entry.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        found.push((entry.path(), mtime));
    }
    // Newest first by mtime.
    found.sort_by(|a, b| b.1.cmp(&a.1));
    for (path, _) in found.into_iter().skip(keep) {
        let _ = fs::remove_file(&path);
    }
}

/// Core backup routine. Snapshots the live DB, gzips it into the backups dir,
/// records `last_backup_at`, and prunes. Used by both `do_backup_now` (forced)
/// and `maybe_backup_on_launch` (interval-gated).
///
/// The DB is opened read-only for the snapshot (no lock contention with a
/// simultaneous fetch); `local_lock` is taken only briefly to read/write the
/// `last_backup_at` setting so the file copy+gzip don't block reads.
fn run_backup(state: &AppState) -> Result<String, String> {
    let dir = resolve_backups_dir(state);
    fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create backups dir: {}", e))?;

    let ts = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let temp_db = dir.join(format!("{}{}.tmp", BACKUP_PREFIX, ts));
    let gz_path = dir.join(format!("{}{}{}", BACKUP_PREFIX, ts, BACKUP_SUFFIX));

    // Snapshot the live DB (read-only open of the source).
    snapshot_db(&state.db_path, &temp_db)?;
    // Compress and remove the temp.
    gzip_file(&temp_db, &gz_path)?;

    // Read frequency for pruning.
    let frequency = with_db_ro(state, |db| {
        let v: String = db
            .query_row(
                "SELECT value FROM settings WHERE key = 'backup_frequency'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "off".to_string());
        Ok(v)
    })?;
    prune_backups(&dir, &frequency);

    // Record last_backup_at (UTC ISO-8601).
    let now_iso = Utc::now().to_rfc3339();
    with_db_rw(state, |db| {
        db.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('last_backup_at', ?1)",
            rusqlite::params![now_iso],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })?;

    let name = gz_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    eprintln!("[my_finances] Backup written: {}", name);
    Ok(name)
}

#[tauri::command]
fn get_backup_settings(state: State<'_, AppState>) -> Result<BackupSettings, String> {
    let (frequency, last_backup_at, custom_dir) = with_db_ro(&state, |db| {
        let frequency: String = db
            .query_row(
                "SELECT value FROM settings WHERE key = 'backup_frequency'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "off".to_string());
        let last_backup_at: Option<String> = db
            .query_row(
                "SELECT value FROM settings WHERE key = 'last_backup_at'",
                [],
                |row| row.get(0),
            )
            .ok();
        let custom_dir: Option<String> = db
            .query_row(
                "SELECT value FROM settings WHERE key = 'backup_dir'",
                [],
                |row| row.get(0),
            )
            .ok();
        Ok((frequency, last_backup_at, custom_dir))
    })?;

    let (backup_dir, is_default_dir) = match custom_dir {
        Some(p) => {
            let pb = PathBuf::from(&p);
            if pb.is_absolute() {
                (p, false)
            } else {
                (resolve_backups_dir(&state).to_string_lossy().to_string(), true)
            }
        }
        None => (
            resolve_backups_dir(&state).to_string_lossy().to_string(),
            true,
        ),
    };

    Ok(BackupSettings {
        frequency,
        last_backup_at,
        backup_dir,
        is_default_dir,
    })
}

#[tauri::command]
fn set_backup_frequency(state: State<'_, AppState>, frequency: String) -> Result<(), String> {
    let f = match frequency.as_str() {
        "off" | "daily" | "weekly" | "monthly" => frequency,
        _ => return Err(format!("Invalid backup frequency: {}", frequency)),
    };
    with_db_rw(&state, |db| {
        db.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('backup_frequency', ?1)",
            rusqlite::params![f],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

#[tauri::command]
fn do_backup_now(state: State<'_, AppState>) -> Result<String, String> {
    run_backup(&state)
}

/// On-launch check: if a frequency is set and enough time has elapsed since
/// the last backup (or there has never been one), run a backup. No-ops when
/// frequency is "off" or not yet due. Called from the frontend on first load,
/// mirroring the autofetch pattern.
#[tauri::command]
fn maybe_backup_on_launch(state: State<'_, AppState>) -> Result<bool, String> {
    let frequency = with_db_ro(&state, |db| {
        let v: String = db
            .query_row(
                "SELECT value FROM settings WHERE key = 'backup_frequency'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "off".to_string());
        Ok(v)
    })?;
    let interval = match backup_interval_secs(&frequency) {
        Some(s) => s,
        None => return Ok(false), // "off"
    };

    let last_iso: Option<String> = with_db_ro(&state, |db| {
        let v: Option<String> = db
            .query_row(
                "SELECT value FROM settings WHERE key = 'last_backup_at'",
                [],
                |row| row.get(0),
            )
            .ok();
        Ok(v)
    })?;

    let due = match last_iso {
        None => true, // never backed up -> due immediately
        Some(s) => match chrono::DateTime::parse_from_rfc3339(&s) {
            Ok(t) => (Utc::now() - t.with_timezone(&Utc)).num_seconds() >= interval,
            Err(_) => true, // unparseable timestamp -> treat as due
        },
    };

    if !due {
        return Ok(false);
    }
    run_backup(&state)?;
    Ok(true)
}

/// Open the backups folder in Finder using the already-present opener plugin.
#[tauri::command]
fn open_backups_folder(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<(), String> {
    let dir = resolve_backups_dir(&state);
    fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create backups dir: {}", e))?;
    app.opener()
        .open_path(dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| format!("Failed to open folder: {}", e))
}

fn recompute_total(conn: &Connection, today: &str) -> Result<(), rusqlite::Error> {
    // Ensure all virtual total account rows exist
    for (id, name) in [
        ("__total__", "Total"),
        ("__assets_total__", "Assets Total"),
        ("__liabilities_total__", "Liabilities Total"),
    ] {
        conn.execute(
            "INSERT OR IGNORE INTO accounts (id, name, type, institution, currency, is_manual, is_ignored)
             VALUES (?1, ?2, '', '', 'NZD', 0, 0)",
            rusqlite::params![id, name],
        )?;
    }

    // Overall total (all non-ignored)
    let total: f64 = conn.query_row(
        "SELECT COALESCE(SUM(balance), 0)
         FROM (
             SELECT balance
             FROM account_values
             WHERE id IN (SELECT MAX(id) FROM account_values GROUP BY account_id)
               AND account_id IN (
                   SELECT id FROM accounts WHERE is_ignored = 0 AND id NOT IN ('__total__', '__assets_total__', '__liabilities_total__')
               )
         )",
        [],
        |row| row.get(0),
    )?;
    upsert_daily_total(conn, "__total__", total, today)?;

    // Assets total (non-ignored accounts with balance >= 0)
    let assets: f64 = conn.query_row(
        "SELECT COALESCE(SUM(balance), 0)
         FROM (
             SELECT balance
             FROM account_values
             WHERE id IN (SELECT MAX(id) FROM account_values GROUP BY account_id)
               AND account_id IN (
                   SELECT id FROM accounts WHERE is_ignored = 0 AND id NOT IN ('__total__', '__assets_total__', '__liabilities_total__')
               )
               AND balance >= 0
         )",
        [],
        |row| row.get(0),
    )?;
    upsert_daily_total(conn, "__assets_total__", assets, today)?;

    // Liabilities total (non-ignored accounts with balance < 0)
    let liabilities: f64 = conn.query_row(
        "SELECT COALESCE(SUM(balance), 0)
         FROM (
             SELECT balance
             FROM account_values
             WHERE id IN (SELECT MAX(id) FROM account_values GROUP BY account_id)
               AND account_id IN (
                   SELECT id FROM accounts WHERE is_ignored = 0 AND id NOT IN ('__total__', '__assets_total__', '__liabilities_total__')
               )
               AND balance < 0
         )",
        [],
        |row| row.get(0),
    )?;
    upsert_daily_total(conn, "__liabilities_total__", liabilities, today)?;

    Ok(())
}

/// Insert (or replace) the single value row for a virtual total account on a
/// given day. This keeps one value per day so the history graph stays clean
/// even when `recompute_total` is invoked multiple times in a day (fetch,
/// toggle-ignore, manual value add, startup seed).
fn upsert_daily_total(
    conn: &Connection,
    account_id: &str,
    balance: f64,
    today: &str,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "DELETE FROM account_values WHERE account_id = ?1 AND recorded_date = ?2",
        rusqlite::params![account_id, today],
    )?;
    conn.execute(
        "INSERT INTO account_values (account_id, balance, recorded_date)
         VALUES (?1, ?2, ?3)",
        rusqlite::params![account_id, balance, today],
    )?;
    Ok(())
}

fn seed_total_if_missing(conn: &Connection) {
    let today = Local::now().format("%Y-%m-%d").to_string();

    // Check if all three virtual accounts have at least one value; recompute if any are missing
    let all_have_values: bool = conn
        .query_row(
            "SELECT (
                (SELECT COUNT(*) FROM account_values WHERE account_id = '__total__') > 0
                AND (SELECT COUNT(*) FROM account_values WHERE account_id = '__assets_total__') > 0
                AND (SELECT COUNT(*) FROM account_values WHERE account_id = '__liabilities_total__') > 0
            )",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if all_have_values {
        return;
    }

    recompute_total(conn, &today).ok();
}

// ---------------------------------------------------------------------------
// App entry point
// ---------------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Load .env file if it exists
    dotenvy::dotenv().ok();

    // Initialize the macOS keychain store (ignored on other platforms)
    keyring::use_named_store("keychain").ok();

    // Determine database path (iCloud Drive CloudDocs when available, else local).
    let db_path = resolve_db_path();

    // One-time migration: copy the legacy local DB into iCloud on first launch.
    // (No-op when not using iCloud, when the iCloud copy already exists, or when
    // there's no legacy local DB to copy from.)
    migrate_legacy_into_icloud(&db_path);

    // Open once at startup to ensure schema exists / migrate, then close so the
    // file handle is released and iCloud can sync the new version.
    {
        let _conn = open_db_rw(&db_path).expect("Failed to initialize database");
        // _conn dropped here — file handle released.
    }

    // Filesystem watcher: emit a `db-changed` event whenever the DB file is
    // modified (including by the iCloud daemon replacing it after a remote
    // write). The frontend listens and re-loads. Created here with an empty
    // handler; re-bound to the AppHandle in `setup()` once it exists.
    let watcher = notify::recommended_watcher(move |_res: Result<notify::Event, notify::Error>| {
        // Replaced in setup() below — this closure is a placeholder.
    })
    .expect("Failed to create file watcher");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup({
            let db_path = db_path.clone();
            move |app| {
                // Rebuild the watcher with a handler that emits a Tauri event.
                let app_handle = app.handle().clone();
                let mut watcher =
                    notify::recommended_watcher(move |_res: Result<notify::Event, notify::Error>| {
                        // Debounce in the frontend; just emit on every event.
                        let _ = app_handle.emit("db-changed", ());
                    })
                    .expect("Failed to create file watcher");
                watcher
                    .watch(&db_path, notify::RecursiveMode::NonRecursive)
                    .expect("Failed to watch database file");

                // Stash the live watcher in managed state, replacing the placeholder.
                let state = app.state::<AppState>();
                *state.watcher.lock().unwrap() = watcher;
                Ok(())
            }
        })
        .manage(AppState {
            db_path,
            local_lock: Mutex::new(()),
            watcher: Mutex::new(watcher),
        })
        .invoke_handler(tauri::generate_handler![
            fetch_akahu_balances,
            get_accounts_summary,
            get_account_history,
            get_account_changes,
            get_all_accounts_config,
            toggle_ignore_account,
            add_manual_account,
            add_manual_account_value,
            delete_last_manual_value,
            has_akahu_credentials,
            save_akahu_credentials,
            get_app_version,
            get_autofetch_setting,
            set_autofetch_setting,
            get_backup_settings,
            set_backup_frequency,
            do_backup_now,
            maybe_backup_on_launch,
            open_backups_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}