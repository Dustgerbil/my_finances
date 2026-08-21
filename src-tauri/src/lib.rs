use chrono::{DateTime, Datelike, Duration, Local, Months, NaiveDate, Utc};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use notify::Watcher;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

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
    pub interest_rate: Option<f64>,
    pub repayment_amount: Option<f64>,
    pub repayment_frequency: Option<u32>,
    pub repayment_frequency_unit: Option<String>,
    pub rate_expiry: Option<String>,
    pub initial_next_repayment_date: Option<String>,
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
    pub previous_balance: Option<f64>,
    pub latest_date: Option<String>,
    pub max_balance: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AccountHistory {
    pub account: Account,
    pub values: Vec<AccountValue>,
    pub current_balance: Option<f64>,
    pub previous_balance: Option<f64>,
    pub min_balance: Option<f64>,
    pub min_balance_date: Option<String>,
    pub max_balance: Option<f64>,
    pub max_balance_date: Option<String>,
    pub next_repayment_date: Option<String>,
    pub estimated_repayment_date: Option<String>,
    pub repayment_estimate_status: Option<String>,
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

#[derive(Debug, Deserialize)]
pub struct UpdateLiabilityRepaymentRequest {
    pub account_id: String,
    pub interest_rate: Option<f64>,
    pub repayment_amount: Option<f64>,
    pub repayment_frequency: Option<u32>,
    pub repayment_frequency_unit: Option<String>,
    pub rate_expiry: Option<String>,
    pub initial_next_repayment_date: Option<String>,
}

type RepaymentDateCalculation = (Option<String>, Option<String>, Option<String>);

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

    // Additive migration for liability repayment details. SQLite's
    // CREATE TABLE IF NOT EXISTS does not add columns to an existing DB, so
    // each nullable column is introduced independently and safely on launch.
    for (column, definition) in [
        ("interest_rate", "REAL"),
        ("repayment_amount", "REAL"),
        ("repayment_frequency", "INTEGER"),
        ("repayment_frequency_unit", "TEXT"),
        ("rate_expiry", "TEXT"),
        ("initial_next_repayment_date", "TEXT"),
    ] {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('accounts') WHERE name = ?1)",
                rusqlite::params![column],
                |row| row.get(0),
            )
            .expect("Failed to inspect accounts schema");
        if !exists {
            conn.execute(
                &format!("ALTER TABLE accounts ADD COLUMN {} {}", column, definition),
                [],
            )
            .expect("Failed to migrate liability repayment fields");
        }
    }
}

fn add_repayment_intervals(
    anchor: NaiveDate,
    frequency: u32,
    unit: &str,
    intervals: u64,
) -> Option<NaiveDate> {
    match unit {
        "days" | "weeks" => {
            let unit_days = if unit == "weeks" { 7_i64 } else { 1_i64 };
            let days = i64::from(frequency)
                .checked_mul(unit_days)?
                .checked_mul(i64::try_from(intervals).ok()?)?;
            anchor.checked_add_signed(Duration::days(days))
        }
        "months" => {
            let months = u64::from(frequency).checked_mul(intervals)?;
            anchor.checked_add_months(Months::new(u32::try_from(months).ok()?))
        }
        _ => None,
    }
}

/// Advance the user-entered first repayment date to the first scheduled date
/// on or after `today`. Each result is calculated from the original anchor so
/// short months do not permanently shift a month-end schedule.
fn current_next_repayment_date(
    anchor: NaiveDate,
    frequency: u32,
    unit: &str,
    today: NaiveDate,
) -> Option<NaiveDate> {
    if frequency == 0 || anchor >= today {
        return (frequency > 0).then_some(anchor);
    }

    let intervals = match unit {
        "days" | "weeks" => {
            let step_days = i64::from(frequency) * if unit == "weeks" { 7 } else { 1 };
            let elapsed_days = (today - anchor).num_days();
            u64::try_from((elapsed_days + step_days - 1) / step_days).ok()?
        }
        "months" => {
            let elapsed_months = i64::from(today.year() - anchor.year()) * 12
                + i64::from(today.month())
                - i64::from(anchor.month());
            let mut intervals = u64::try_from(elapsed_months.max(0)).ok()? / u64::from(frequency);
            if add_repayment_intervals(anchor, frequency, unit, intervals)? < today {
                intervals += 1;
            }
            intervals
        }
        _ => return None,
    };

    add_repayment_intervals(anchor, frequency, unit, intervals)
}

fn calculate_repayment_dates(
    account: &Account,
    current_balance: Option<f64>,
    today: NaiveDate,
) -> RepaymentDateCalculation {
    let Some(balance) = current_balance.filter(|balance| *balance < 0.0) else {
        return (None, None, None);
    };

    let anchor = account
        .initial_next_repayment_date
        .as_deref()
        .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok());
    let next_date = match (
        anchor,
        account.repayment_frequency,
        account.repayment_frequency_unit.as_deref(),
    ) {
        (Some(anchor), Some(frequency), Some(unit)) => {
            current_next_repayment_date(anchor, frequency, unit, today)
        }
        _ => None,
    };
    let next_date_string = next_date.map(|date| date.format("%Y-%m-%d").to_string());

    let (Some(annual_rate_percent), Some(payment), Some(frequency), Some(unit), Some(next_date)) = (
        account.interest_rate,
        account.repayment_amount,
        account.repayment_frequency,
        account.repayment_frequency_unit.as_deref(),
        next_date,
    ) else {
        return (
            next_date_string,
            None,
            Some(
                "Add the interest rate, repayment amount, frequency and initial next repayment date to estimate repayment."
                    .to_string(),
            ),
        );
    };

    let annual_rate = annual_rate_percent / 100.0;
    if !annual_rate.is_finite() || annual_rate < 0.0 || !payment.is_finite() || payment <= 0.0 {
        return (
            next_date_string,
            None,
            Some("The repayment values must be positive valid numbers.".into()),
        );
    }
    let period_rate = match unit {
        "days" => annual_rate * f64::from(frequency) / 365.0,
        "weeks" => annual_rate * f64::from(frequency) * 7.0 / 365.0,
        "months" => annual_rate * f64::from(frequency) / 12.0,
        _ => {
            return (
                next_date_string,
                None,
                Some("Invalid repayment frequency unit.".into()),
            )
        }
    };
    let principal = balance.abs();
    let payment_count = if period_rate == 0.0 {
        (principal / payment).ceil()
    } else {
        let first_period_interest = principal * period_rate;
        if payment <= first_period_interest {
            return (
                next_date_string,
                None,
                Some("The repayment does not cover the interest charged each period.".into()),
            );
        }
        (-(1.0 - period_rate * principal / payment).ln() / period_rate.ln_1p()).ceil()
    };

    if !payment_count.is_finite() || payment_count < 1.0 || payment_count > f64::from(u32::MAX) {
        return (
            next_date_string,
            None,
            Some("The repayment date could not be estimated from these values.".into()),
        );
    }
    let payoff_date = add_repayment_intervals(next_date, frequency, unit, payment_count as u64 - 1);
    match payoff_date {
        Some(date) => (
            next_date_string,
            Some(date.format("%Y-%m-%d").to_string()),
            None,
        ),
        None => (
            next_date_string,
            None,
            Some("The estimated repayment date is outside the supported date range.".into()),
        ),
    }
}

fn calculate_liabilities_total_repayment_dates(
    conn: &Connection,
    today: NaiveDate,
) -> Result<RepaymentDateCalculation, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.name, a.type, a.institution, a.currency, a.is_manual, a.is_ignored,
                a.interest_rate, a.repayment_amount, a.repayment_frequency,
                a.repayment_frequency_unit, a.rate_expiry, a.initial_next_repayment_date,
                av.balance
         FROM accounts a
         JOIN (
             SELECT account_id, balance
             FROM (
                 SELECT account_id, balance,
                        ROW_NUMBER() OVER (
                            PARTITION BY account_id
                            ORDER BY recorded_date DESC, id DESC
                        ) AS rn
                 FROM account_values
             )
             WHERE rn = 1
         ) av ON av.account_id = a.id
         WHERE a.is_ignored = 0
           AND a.id NOT IN ('__total__', '__assets_total__', '__liabilities_total__')
           AND av.balance < 0
         ORDER BY a.name",
    )?;

    let liabilities = stmt
        .query_map([], |row| {
            Ok((
                Account {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    account_type: row.get(2)?,
                    institution: row.get(3)?,
                    currency: row.get(4)?,
                    is_manual: row.get::<_, i32>(5)? != 0,
                    is_ignored: row.get::<_, i32>(6)? != 0,
                    interest_rate: row.get(7)?,
                    repayment_amount: row.get(8)?,
                    repayment_frequency: row.get(9)?,
                    repayment_frequency_unit: row.get(10)?,
                    rate_expiry: row.get(11)?,
                    initial_next_repayment_date: row.get(12)?,
                },
                row.get::<_, f64>(13)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    if liabilities.is_empty() {
        return Ok((None, None, None));
    }

    let mut next_dates = Vec::new();
    let mut payoff_dates = Vec::new();
    let mut incomplete_accounts = Vec::new();
    for (account, balance) in liabilities {
        let (next_date, payoff_date, _) = calculate_repayment_dates(&account, Some(balance), today);
        if let Some(next_date) = next_date {
            next_dates.push(next_date);
        }
        if let Some(payoff_date) = payoff_date {
            payoff_dates.push(payoff_date);
        } else {
            incomplete_accounts.push(account.name);
        }
    }

    let next_date = next_dates.into_iter().min();
    if incomplete_accounts.is_empty() {
        Ok((next_date, payoff_dates.into_iter().max(), None))
    } else {
        Ok((
            next_date,
            None,
            Some(format!(
                "Complete valid repayment details for: {}.",
                incomplete_accounts.join(", ")
            )),
        ))
    }
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
/// derives the total from each account's *latest* balance across all dates,
/// not a date-specific balance, so recomputing a historical date would
/// store today's total under that past date and corrupt the graph. Today's total
/// is recomputed fresh by the normal fetch/toggle/manual write paths that call
/// this merge via `open_db_rw`.
fn merge_one_conflict(
    conn: &Connection,
    conflict_path: &std::path::Path,
) -> Result<(), rusqlite::Error> {
    let path_str = conflict_path.to_str().unwrap_or("");
    // Safety: if a previous iteration failed after ATTACH but before DETACH,
    // `other` would still be attached and our ATTACH below would error. Detach
    // first (ignore the error when nothing is attached).
    let _ = conn.execute("DETACH DATABASE other", []);
    conn.execute("ATTACH DATABASE ?1 AS other", rusqlite::params![path_str])?;

    // accounts: union by id, keep live on collision. Conflict copies made by
    // an older app version do not have the repayment columns, so inspect the
    // attached schema before selecting them.
    let other_account_columns = {
        let mut stmt = conn.prepare("PRAGMA other.table_info(accounts)")?;
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        columns
    };
    let has_repayment_columns = [
        "interest_rate",
        "repayment_amount",
        "repayment_frequency",
        "repayment_frequency_unit",
        "rate_expiry",
        "initial_next_repayment_date",
    ]
    .iter()
    .all(|column| other_account_columns.iter().any(|other| other == column));
    if has_repayment_columns {
        conn.execute(
            "INSERT OR IGNORE INTO accounts
               (id, name, type, institution, currency, is_manual, is_ignored,
                interest_rate, repayment_amount, repayment_frequency,
                repayment_frequency_unit, rate_expiry, initial_next_repayment_date)
             SELECT id, name, type, institution, currency, is_manual, is_ignored,
                    interest_rate, repayment_amount, repayment_frequency,
                    repayment_frequency_unit, rate_expiry, initial_next_repayment_date
             FROM other.accounts",
            [],
        )?;
        // A conflict copy may contain repayment details for an account that
        // already exists in the live DB. Fill only live nulls so those details
        // are not lost while preserving the established keep-live rule when
        // both copies contain a value.
        conn.execute(
            "UPDATE accounts AS live SET
                interest_rate = COALESCE(interest_rate, (
                    SELECT interest_rate FROM other.accounts WHERE id = live.id)),
                repayment_amount = COALESCE(repayment_amount, (
                    SELECT repayment_amount FROM other.accounts WHERE id = live.id)),
                repayment_frequency = COALESCE(repayment_frequency, (
                    SELECT repayment_frequency FROM other.accounts WHERE id = live.id)),
                repayment_frequency_unit = COALESCE(repayment_frequency_unit, (
                    SELECT repayment_frequency_unit FROM other.accounts WHERE id = live.id)),
                rate_expiry = COALESCE(rate_expiry, (
                    SELECT rate_expiry FROM other.accounts WHERE id = live.id)),
                initial_next_repayment_date = COALESCE(initial_next_repayment_date, (
                    SELECT initial_next_repayment_date FROM other.accounts WHERE id = live.id))
             WHERE EXISTS (SELECT 1 FROM other.accounts WHERE id = live.id)",
            [],
        )?;
    } else {
        conn.execute(
            "INSERT OR IGNORE INTO accounts
               (id, name, type, institution, currency, is_manual, is_ignored)
             SELECT id, name, type, institution, currency, is_manual, is_ignored
             FROM other.accounts",
            [],
        )?;
    }

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
    conn.execute(
        "INSERT OR IGNORE INTO settings SELECT key, value FROM other.settings",
        [],
    )?;

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
    let flags =
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
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
    let kr =
        keyring_core::Entry::new(KEYRING_SERVICE, "akahu_app_id").map_err(|e| e.to_string())?;
    let app_id = kr.get_password().ok();
    let kr =
        keyring_core::Entry::new(KEYRING_SERVICE, "akahu_user_token").map_err(|e| e.to_string())?;
    let user_token = kr.get_password().ok();

    match (app_id, user_token) {
        (Some(id), Some(token)) => Ok((id, token)),
        _ => {
            // Fall back to env vars
            let id = std::env::var("AKAHU_APP_ID").map_err(|_| {
                "Akahu credentials not found. Go to Settings to add them.".to_string()
            })?;
            let token = std::env::var("AKAHU_USER_TOKEN").map_err(|_| {
                "Akahu credentials not found. Go to Settings to add them.".to_string()
            })?;
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
    status: Option<String>,
    connection: Option<AkahuConnection>,
    balance: Option<AkahuBalance>,
    refreshed: Option<AkahuRefreshed>,
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
struct AkahuRefreshed {
    balance: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AkahuRefreshItem {
    #[serde(rename = "_id")]
    id: String,
    status: Option<String>,
}

async fn trigger_akahu_refresh(app_id: &str, user_token: &str) -> Result<Option<String>, String> {
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
    let body_text = resp
        .text()
        .await
        .map_err(|e| format!("Failed to read refresh response: {}", e))?;

    if !status.is_success() {
        return Err(format!(
            "Akahu refresh returned HTTP {}: {}",
            status.as_u16(),
            truncate_str(&body_text, 200)
        ));
    }

    let data: AkahuApiResponse<AkahuRefreshItem> =
        serde_json::from_str(&body_text).map_err(|e| {
            format!(
                "Failed to parse refresh response: {}. Body: {}",
                e,
                truncate_str(&body_text, 200)
            )
        })?;

    if !data.success {
        return Err(data.message.unwrap_or_else(|| "Unknown error".to_string()));
    }

    // A request may return no job when Akahu decides there is nothing to do.
    // The caller still validates the account balance timestamps before saving.
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
        let body_text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read refresh status: {}", e))?;

        if !status.is_success() {
            return Err(format!(
                "Refresh status HTTP {}: {}",
                status.as_u16(),
                truncate_str(&body_text, 200)
            ));
        }

        let data: AkahuApiResponse<AkahuRefreshItem> =
            serde_json::from_str(&body_text).map_err(|e| {
                format!(
                    "Failed to parse refresh status: {}. Body: {}",
                    e,
                    truncate_str(&body_text, 200)
                )
            })?;

        if !data.success {
            return Err(data.message.unwrap_or_else(|| "Unknown error".to_string()));
        }

        match data.item.and_then(|r| r.status) {
            Some(ref status) if status == "COMPLETED" => return Ok(true),
            Some(ref status) if status == "ERROR" => {
                return Err("Refresh failed with error".to_string())
            }
            _ => {} // still in progress, loop
        }
    }
}

async fn wait_for_refresh_with_timeout<F>(
    refresh_wait: F,
    timeout_secs: u64,
) -> Result<bool, String>
where
    F: std::future::Future<Output = Result<bool, String>>,
{
    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), refresh_wait).await {
        Ok(result) => result,
        Err(_) => Ok(false),
    }
}

async fn fetch_akahu_accounts(app_id: &str, user_token: &str) -> Result<Vec<AkahuAccount>, String> {
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
    let body_text = resp
        .text()
        .await
        .map_err(|e| format!("Failed to read accounts response: {}", e))?;

    if !status.is_success() {
        return Err(format!(
            "Akahu accounts returned HTTP {}: {}",
            status.as_u16(),
            truncate_str(&body_text, 200)
        ));
    }

    let data: AkahuApiResponse<AkahuAccount> = serde_json::from_str(&body_text).map_err(|e| {
        format!(
            "Failed to parse accounts response: {}. Body: {}",
            e,
            truncate_str(&body_text, 200)
        )
    })?;

    if !data.success {
        return Err(data.message.unwrap_or_else(|| "Unknown error".to_string()));
    }

    Ok(data.items.unwrap_or_default())
}

const MAX_ACCEPTABLE_BALANCE_AGE_MINUTES: i64 = 65;
const REFRESH_JOB_TIMEOUT_SECS: u64 = 120;
const ACCOUNT_FRESHNESS_TIMEOUT_SECS: u64 = 30;
const ACCOUNT_FRESHNESS_POLL_SECS: u64 = 3;

fn is_nzd_account(account: &AkahuAccount) -> bool {
    account
        .balance
        .as_ref()
        .and_then(|balance| balance.currency.as_deref())
        .unwrap_or("NZD")
        == "NZD"
}

fn is_refreshable_account(account: &AkahuAccount) -> bool {
    account.status.as_deref() != Some("INACTIVE")
}

fn balance_freshness_issues(
    accounts: &[AkahuAccount],
    now: DateTime<Utc>,
    maximum_age: Duration,
) -> Vec<String> {
    accounts
        .iter()
        .filter(|account| is_nzd_account(account) && is_refreshable_account(account))
        .filter_map(|account| {
            let timestamp = match account
                .refreshed
                .as_ref()
                .and_then(|refreshed| refreshed.balance.as_deref())
            {
                Some(timestamp) => timestamp,
                None => {
                    return Some(format!("{} (no balance refresh time)", account.name));
                }
            };

            let refreshed_at = match DateTime::parse_from_rfc3339(timestamp) {
                Ok(timestamp) => timestamp.with_timezone(&Utc),
                Err(_) => {
                    return Some(format!("{} (invalid balance refresh time)", account.name));
                }
            };

            if now.signed_duration_since(refreshed_at) > maximum_age {
                Some(format!("{} (last refreshed {})", account.name, timestamp))
            } else {
                None
            }
        })
        .collect()
}

async fn fetch_fresh_akahu_accounts(
    app_id: &str,
    user_token: &str,
    timeout_secs: u64,
) -> Result<(Vec<AkahuAccount>, bool), String> {
    let started = std::time::Instant::now();
    let maximum_age = Duration::minutes(MAX_ACCEPTABLE_BALANCE_AGE_MINUTES);

    loop {
        let accounts = fetch_akahu_accounts(app_id, user_token).await?;
        let issues = balance_freshness_issues(&accounts, Utc::now(), maximum_age);
        if issues.is_empty() {
            return Ok((accounts, false));
        }

        if started.elapsed().as_secs() >= timeout_secs {
            return Ok((accounts, true));
        }

        tokio::time::sleep(std::time::Duration::from_secs(ACCOUNT_FRESHNESS_POLL_SECS)).await;
    }
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

fn previous_stored_balance(values: &[AccountValue]) -> Option<f64> {
    values.iter().rev().nth(1).map(|value| value.balance)
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
async fn fetch_akahu_balances(state: State<'_, AppState>, force: bool) -> Result<String, String> {
    let (app_id, user_token) = get_akahu_headers()?;

    // Automatic fetches remain once per day. A user-requested forced refresh
    // bypasses this guard, then replaces today's values via the daily upserts
    // below so account history still contains at most one row per day.
    let today = Local::now().format("%Y-%m-%d").to_string();
    if !force {
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

    // Trigger and wait when Akahu creates a refresh job. Bound the whole wait,
    // including an individual status request that stalls. A timeout is not a
    // failed fetch: fall back to the account data Akahu currently has cached.
    // Explicit refresh errors still fail so they are not silently hidden.
    let refresh_timed_out =
        if let Some(refresh_id) = trigger_akahu_refresh(&app_id, &user_token).await? {
            !wait_for_refresh_with_timeout(
                wait_for_refresh(&app_id, &user_token, &refresh_id, REFRESH_JOB_TIMEOUT_SECS),
                REFRESH_JOB_TIMEOUT_SECS,
            )
            .await?
        } else {
            false
        };

    // Akahu serves account reads from its cache, and a completed refresh job can
    // become visible there asynchronously. Poll until every refreshable NZD
    // account has a recent source balance timestamp. A Personal App's manual
    // refresh rest period is one hour; the extra five minutes avoids rejecting
    // a just-within-window result due to clock or request latency. If freshness
    // cannot be verified before the timeout, save the most recent account
    // response instead of failing the fetch. When the refresh job itself timed
    // out, skip this second wait and fetch whatever data is available once.
    let (accounts, freshness_timed_out) = if refresh_timed_out {
        (fetch_akahu_accounts(&app_id, &user_token).await?, false)
    } else {
        fetch_fresh_akahu_accounts(&app_id, &user_token, ACCOUNT_FRESHNESS_TIMEOUT_SECS).await?
    };
    let fetched_at = Utc::now().to_rfc3339();

    // Filter to refreshable NZD accounts. Inactive accounts remain in history
    // at their last known value instead of receiving a misleading new daily
    // snapshot that Akahu can no longer refresh.
    let nzd_accounts: Vec<&AkahuAccount> = accounts
        .iter()
        .filter(|account| is_nzd_account(account) && is_refreshable_account(account))
        .collect();

    // Save to database (tight write critical section; no lock held during the network above).
    with_db_rw(&state, |db| {
        let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
        for acc in &nzd_accounts {
            let inst_name = acc
                .connection
                .as_ref()
                .and_then(|c| c.institution.as_ref())
                .and_then(|i| i.name.as_deref())
                .or_else(|| acc.connection.as_ref().and_then(|c| c.name.as_deref()))
                .unwrap_or("Unknown");

            let acc_type = acc.account_type.as_deref().unwrap_or("Unknown");
            let balance = acc.balance.as_ref().and_then(|b| b.current).unwrap_or(0.0);

            // Upsert account
            tx.execute(
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
            upsert_daily_account_value(&tx, &acc.id, balance, &today).map_err(|e| e.to_string())?;
        }

        // ---- Virtual total account ----
        recompute_total(&tx, &today).map_err(|e| e.to_string())?;
        // ---- End virtual total ----

        // Log the fetch (idempotent: same-day re-fetch on another machine
        // must not add a duplicate fetch_log row, which would make the
        // fetch-log UNION merge misbehave).
        tx.execute(
            "DELETE FROM fetch_log WHERE fetched_at = ?1",
            rusqlite::params![today],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO fetch_log (fetched_at) VALUES (?1)",
            rusqlite::params![today],
        )
        .map_err(|e| e.to_string())?;
        // Keep the precise completion time separately: fetch_log deliberately
        // remains date-only because it enforces the once-per-day behavior.
        tx.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('last_fetch_at', ?1)",
            rusqlite::params![fetched_at],
        )
        .map_err(|e| e.to_string())?;

        tx.commit().map_err(|e| e.to_string())
    })?;

    if refresh_timed_out {
        Ok(format!(
            "Akahu refresh timed out; fetched {} NZD accounts from available data",
            nzd_accounts.len()
        ))
    } else if freshness_timed_out {
        Ok(format!(
            "Akahu balance freshness check timed out; fetched {} NZD accounts from available data",
            nzd_accounts.len()
        ))
    } else {
        Ok(format!("Fetched {} NZD accounts", nzd_accounts.len()))
    }
}

fn query_accounts_summary(
    db: &Connection,
    ignored_only: bool,
) -> Result<Vec<AccountSummary>, String> {
    let visibility_filter = if ignored_only {
        "a.is_ignored = 1
         AND a.id NOT IN ('__total__', '__assets_total__', '__liabilities_total__')"
    } else {
        "a.is_ignored = 0"
    };
    let query = format!(
        "SELECT a.id, a.name, a.type, a.institution, a.currency, a.is_manual, a.is_ignored,
                        a.interest_rate, a.repayment_amount, a.repayment_frequency,
                        a.repayment_frequency_unit, a.rate_expiry, a.initial_next_repayment_date,
                        av.balance, av.recorded_date,
                        (SELECT MAX(balance) FROM account_values WHERE account_id = a.id) AS max_balance,
                        (SELECT balance FROM account_values
                         WHERE account_id = a.id
                         ORDER BY recorded_date DESC, id DESC
                         LIMIT 1 OFFSET 1) AS previous_balance
                 FROM accounts a
                 LEFT JOIN (
                     SELECT account_id, balance, recorded_date
                     FROM (
                         SELECT account_id, balance, recorded_date,
                                ROW_NUMBER() OVER (
                                    PARTITION BY account_id
                                    ORDER BY recorded_date DESC, id DESC
                                ) AS rn
                         FROM account_values
                     )
                     WHERE rn = 1
                 ) av ON a.id = av.account_id
                 WHERE {visibility_filter}
                 ORDER BY CASE WHEN a.id = '__total__' THEN 0
                               WHEN a.id = '__assets_total__' THEN 1
                               WHEN a.id = '__liabilities_total__' THEN 2
                               ELSE 3 END,
                          a.institution, a.name"
    );
    let mut stmt = db.prepare(&query).map_err(|e| e.to_string())?;

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
                    interest_rate: row.get(7)?,
                    repayment_amount: row.get(8)?,
                    repayment_frequency: row.get(9)?,
                    repayment_frequency_unit: row.get(10)?,
                    rate_expiry: row.get(11)?,
                    initial_next_repayment_date: row.get(12)?,
                },
                latest_balance: row.get(13)?,
                latest_date: row.get(14)?,
                max_balance: row.get(15)?,
                previous_balance: row.get(16)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(summaries)
}

#[tauri::command]
fn get_accounts_summary(state: State<'_, AppState>) -> Result<Vec<AccountSummary>, String> {
    with_db_ro(&state, |db| query_accounts_summary(db, false))
}

#[tauri::command]
fn get_other_accounts_summary(state: State<'_, AppState>) -> Result<Vec<AccountSummary>, String> {
    with_db_ro(&state, |db| query_accounts_summary(db, true))
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
                "SELECT id, name, type, institution, currency, is_manual, is_ignored,
                        interest_rate, repayment_amount, repayment_frequency,
                        repayment_frequency_unit, rate_expiry, initial_next_repayment_date
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
                        interest_rate: row.get(7)?,
                        repayment_amount: row.get(8)?,
                        repayment_frequency: row.get(9)?,
                        repayment_frequency_unit: row.get(10)?,
                        rate_expiry: row.get(11)?,
                        initial_next_repayment_date: row.get(12)?,
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
                 ORDER BY recorded_date ASC, id ASC",
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
        let previous_balance = previous_stored_balance(&values);
        let min_balance = values
            .iter()
            .map(|v| v.balance)
            .fold(f64::INFINITY, f64::min);
        let max_balance = values
            .iter()
            .map(|v| v.balance)
            .fold(f64::NEG_INFINITY, f64::max);

        // Date the min/max value last occurred. `values` is ordered ASC by
        // date, so iterating and keeping the last match gives the most recent
        // date on which that extreme balance was recorded (ties included).
        let min_balance_date = if min_balance.is_finite() {
            values
                .iter()
                .rev()
                .find(|v| v.balance == min_balance)
                .map(|v| v.recorded_date.clone())
        } else {
            None
        };
        let max_balance_date = if max_balance.is_finite() {
            values
                .iter()
                .rev()
                .find(|v| v.balance == max_balance)
                .map(|v| v.recorded_date.clone())
        } else {
            None
        };

        let (next_repayment_date, estimated_repayment_date, repayment_estimate_status) =
            if account.id == "__liabilities_total__" {
                calculate_liabilities_total_repayment_dates(db, Local::now().date_naive())
                    .map_err(|e| e.to_string())?
            } else {
                calculate_repayment_dates(&account, current_balance, Local::now().date_naive())
            };

        Ok(AccountHistory {
            account,
            values,
            current_balance,
            previous_balance,
            min_balance: if min_balance.is_finite() {
                Some(min_balance)
            } else {
                None
            },
            min_balance_date,
            max_balance: if max_balance.is_finite() {
                Some(max_balance)
            } else {
                None
            },
            max_balance_date,
            next_repayment_date,
            estimated_repayment_date,
            repayment_estimate_status,
        })
    })
}

#[tauri::command]
fn get_all_accounts_config(state: State<'_, AppState>) -> Result<Vec<Account>, String> {
    with_db_ro(&state, |db| {
        let mut stmt = db
            .prepare(
                "SELECT id, name, type, institution, currency, is_manual, is_ignored,
                        interest_rate, repayment_amount, repayment_frequency,
                        repayment_frequency_unit, rate_expiry, initial_next_repayment_date
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
                    interest_rate: row.get(7)?,
                    repayment_amount: row.get(8)?,
                    repayment_frequency: row.get(9)?,
                    repayment_frequency_unit: row.get(10)?,
                    rate_expiry: row.get(11)?,
                    initial_next_repayment_date: row.get(12)?,
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
        let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
        // Toggle the is_ignored flag
        tx.execute(
            "UPDATE accounts SET is_ignored = CASE WHEN is_ignored = 0 THEN 1 ELSE 0 END
             WHERE id = ?1",
            rusqlite::params![account_id],
        )
        .map_err(|e| e.to_string())?;

        // Return the new value
        let new_value: bool = tx
            .query_row(
                "SELECT is_ignored != 0 FROM accounts WHERE id = ?1",
                rusqlite::params![account_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;

        // Recalculate the virtual total
        recompute_total(&tx, &today).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;

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
            interest_rate: None,
            repayment_amount: None,
            repayment_frequency: None,
            repayment_frequency_unit: None,
            rate_expiry: None,
            initial_next_repayment_date: None,
        })
    })?;

    Ok(account)
}

#[tauri::command]
fn update_liability_repayment(
    state: State<'_, AppState>,
    request: UpdateLiabilityRepaymentRequest,
) -> Result<(), String> {
    if request.account_id.starts_with("__") {
        return Err("Repayment details cannot be set on a total account".to_string());
    }
    if let Some(rate) = request.interest_rate {
        if !rate.is_finite() || rate < 0.0 {
            return Err("Interest rate must be zero or greater".to_string());
        }
    }
    if let Some(amount) = request.repayment_amount {
        if !amount.is_finite() || amount <= 0.0 {
            return Err("Repayment amount must be greater than zero".to_string());
        }
    }
    if matches!(request.repayment_frequency, Some(0)) {
        return Err("Repayment frequency must be greater than zero".to_string());
    }
    match (
        request.repayment_frequency,
        request.repayment_frequency_unit.as_deref(),
    ) {
        (Some(_), Some("days" | "weeks" | "months")) | (None, None) => {}
        (Some(_), _) => return Err("Choose days, weeks or months for the frequency".to_string()),
        (None, Some(_)) => return Err("Enter a repayment frequency".to_string()),
    }
    for (label, value) in [
        ("Rate expiry", request.rate_expiry.as_deref()),
        (
            "Initial next repayment date",
            request.initial_next_repayment_date.as_deref(),
        ),
    ] {
        if let Some(value) = value {
            let parsed = NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map_err(|_| format!("{} must be a valid date", label))?;
            if parsed.format("%Y-%m-%d").to_string() != value {
                return Err(format!("{} must use YYYY-MM-DD format", label));
            }
        }
    }

    with_db_rw(&state, |db| {
        let current_balance: Option<f64> = db
            .query_row(
                "SELECT (
                    SELECT balance FROM account_values
                    WHERE account_id = a.id
                    ORDER BY recorded_date DESC, id DESC LIMIT 1
                 )
                 FROM accounts a WHERE a.id = ?1",
                rusqlite::params![request.account_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Account not found: {}", e))?;
        if !matches!(current_balance, Some(balance) if balance < 0.0) {
            return Err("Repayment details can only be set for a liability account".to_string());
        }

        db.execute(
            "UPDATE accounts SET
                interest_rate = ?2,
                repayment_amount = ?3,
                repayment_frequency = ?4,
                repayment_frequency_unit = ?5,
                rate_expiry = ?6,
                initial_next_repayment_date = ?7
             WHERE id = ?1",
            rusqlite::params![
                request.account_id,
                request.interest_rate,
                request.repayment_amount,
                request.repayment_frequency,
                request.repayment_frequency_unit,
                request.rate_expiry,
                request.initial_next_repayment_date,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

#[tauri::command]
fn add_manual_account_value(
    state: State<'_, AppState>,
    request: AddManualValueRequest,
) -> Result<(), String> {
    let today = Local::now().format("%Y-%m-%d").to_string();
    with_db_rw(&state, |db| {
        let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
        // Verify it's a manual account
        let is_manual: bool = tx
            .query_row(
                "SELECT is_manual != 0 FROM accounts WHERE id = ?1",
                rusqlite::params![request.account_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Account not found: {}", e))?;

        if !is_manual {
            return Err("Can only add values to manual accounts".to_string());
        }

        // A manual account follows the same one-value-per-account-per-day rule
        // as fetched and virtual accounts. A second save today replaces the
        // earlier value instead of creating a duplicate chart point.
        upsert_daily_account_value(&tx, &request.account_id, request.balance, &today)
            .map_err(|e| e.to_string())?;

        // Recalculate the virtual total
        recompute_total(&tx, &today).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;

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
        let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
        // Verify it's a manual account.
        let is_manual: bool = tx
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
        let row: Option<(i64, f64, String)> = tx
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

        tx.execute(
            "DELETE FROM account_values WHERE id = ?1",
            rusqlite::params![row_id],
        )
        .map_err(|e| e.to_string())?;

        // Recalculate the virtual total so cards/graphs reflect the deletion.
        recompute_total(&tx, &today).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;

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
                 ORDER BY recorded_date DESC, id DESC
                 LIMIT 1",
                rusqlite::params![account_id],
                |row| row.get(0),
            )
            .ok();

        // Find the earliest measurement for this account. The date gates the
        // fixed lookbacks, while the balance is the baseline for "all time".
        let earliest_measurement: Option<(f64, String)> = db
            .query_row(
                "SELECT balance, recorded_date
                 FROM account_values
                 WHERE account_id = ?1
                 ORDER BY recorded_date ASC, id ASC
                 LIMIT 1",
                rusqlite::params![account_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        let today = Local::now().format("%Y-%m-%d").to_string();

        let periods: Vec<(&str, i64)> = vec![("30d", 30), ("180d", 180), ("360d", 360)];

        let mut results = Vec::new();

        for (label, days) in periods {
            let lookback_date = chrono::NaiveDate::parse_from_str(&today, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.checked_sub_signed(chrono::Duration::days(days)))
                .map(|d| d.format("%Y-%m-%d").to_string());

            let (prev_balance, prev_date): (Option<f64>, Option<String>) =
                if let Some(ref lb) = lookback_date {
                    // Check if the earliest measurement is after the lookback date
                    let before_measurements = earliest_measurement
                        .as_ref()
                        .map(|(_, date)| date.as_str() > lb.as_str())
                        .unwrap_or(true);

                    if before_measurements {
                        (None, None)
                    } else {
                        // Get the value closest to (≤) the lookback date
                        db.query_row(
                            "SELECT balance, recorded_date
                             FROM account_values
                             WHERE account_id = ?1 AND recorded_date <= ?2
                             ORDER BY recorded_date DESC, id DESC
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

        let (all_time_balance, all_time_date) = earliest_measurement
            .map(|(balance, date)| (Some(balance), Some(date)))
            .unwrap_or((None, None));
        let all_time_change = match (current_balance, all_time_balance) {
            (Some(cur), Some(prev)) if prev != 0.0 => {
                let amount = cur - prev;
                (Some(amount), Some((amount / prev.abs()) * 100.0))
            }
            (Some(cur), Some(prev)) => (Some(cur - prev), None),
            _ => (None, None),
        };

        results.push(AccountChange {
            period: "all".to_string(),
            previous_balance: all_time_balance,
            previous_date: all_time_date,
            change_amount: all_time_change.0,
            change_percent: all_time_change.1,
        });

        Ok(results)
    })
}

// ---------------------------------------------------------------------------
// Keychain credential commands
// ---------------------------------------------------------------------------

#[tauri::command]
fn has_akahu_credentials() -> Result<bool, String> {
    let kr =
        keyring_core::Entry::new(KEYRING_SERVICE, "akahu_app_id").map_err(|e| e.to_string())?;
    let has_app_id = kr.get_password().is_ok();
    let kr =
        keyring_core::Entry::new(KEYRING_SERVICE, "akahu_user_token").map_err(|e| e.to_string())?;
    let has_token = kr.get_password().is_ok();
    Ok(has_app_id && has_token)
}

#[tauri::command]
fn save_akahu_credentials(app_id: String, user_token: String) -> Result<(), String> {
    let kr =
        keyring_core::Entry::new(KEYRING_SERVICE, "akahu_app_id").map_err(|e| e.to_string())?;
    kr.set_password(&app_id)
        .map_err(|e| format!("Failed to save App ID: {}", e))?;
    let kr =
        keyring_core::Entry::new(KEYRING_SERVICE, "akahu_user_token").map_err(|e| e.to_string())?;
    kr.set_password(&user_token)
        .map_err(|e| format!("Failed to save User Token: {}", e))?;
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
    frequency: String,              // "off" | "daily" | "weekly" | "monthly"
    last_backup_at: Option<String>, // ISO-8601 UTC, or None if never
    backup_dir: String,             // resolved absolute path actually in use
    is_default_dir: bool,           // true when no custom dir set (backup_dir is the default)
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct DataHealth {
    db_path: String,
    is_icloud: bool,
    integrity_status: String,
    last_fetch_at: Option<String>,
    last_backup_at: Option<String>,
    account_count: i64,
    value_count: i64,
    pending_conflict_copies: usize,
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
        _ => None,                    // "off"
    }
}

/// Snapshot the live DB to `target` via `VACUUM INTO` (clean, compacted copy).
/// Opens the source read-only; VACUUM INTO does not mutate the source.
fn snapshot_db(live_path: &std::path::Path, target: &std::path::Path) -> Result<(), String> {
    let flags =
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
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
    let out =
        fs::File::create(target).map_err(|e| format!("Failed to create backup file: {}", e))?;
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

fn count_conflict_copies(live_path: &std::path::Path) -> usize {
    let Some(dir) = live_path.parent() else {
        return 0;
    };
    let Some(live_name) = live_path.file_name().and_then(|name| name.to_str()) else {
        return 0;
    };
    let stem = live_name.trim_end_matches(".db");
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| {
                    entry.file_name().to_str().is_some_and(|name| {
                        name != live_name && name.starts_with(stem) && name.ends_with(".db")
                    })
                })
                .count()
        })
        .unwrap_or(0)
}

fn validate_restored_db(path: &std::path::Path) -> Result<(), String> {
    let flags =
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags)
        .map_err(|e| format!("Restored file is not a readable SQLite database: {}", e))?;
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|e| format!("Could not validate restored database: {}", e))?;
    if integrity != "ok" {
        return Err(format!(
            "Restored database failed integrity check: {}",
            integrity
        ));
    }
    for table in ["accounts", "account_values", "fetch_log", "settings"] {
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                rusqlite::params![table],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !exists {
            return Err(format!(
                "Restored database is missing required table '{}'",
                table
            ));
        }
    }
    Ok(())
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
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create backups dir: {}", e))?;

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
                (
                    resolve_backups_dir(&state).to_string_lossy().to_string(),
                    true,
                )
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
fn get_data_health(state: State<'_, AppState>) -> Result<DataHealth, String> {
    let (integrity_status, last_fetch_at, last_backup_at, account_count, value_count) =
        with_db_ro(&state, |db| {
            let integrity_status: String = db
                .query_row("PRAGMA integrity_check", [], |row| row.get(0))
                .map_err(|e| e.to_string())?;
            let last_fetch_at = get_last_fetch_at(db).map_err(|e| e.to_string())?;
            let last_backup_at = db
                .query_row(
                    "SELECT value FROM settings WHERE key = 'last_backup_at'",
                    [],
                    |row| row.get(0),
                )
                .ok();
            let account_count = db
                .query_row(
                    "SELECT COUNT(*) FROM accounts
                     WHERE id NOT IN ('__total__', '__assets_total__', '__liabilities_total__')",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?;
            let value_count = db
                .query_row("SELECT COUNT(*) FROM account_values", [], |row| row.get(0))
                .map_err(|e| e.to_string())?;
            Ok((
                integrity_status,
                last_fetch_at,
                last_backup_at,
                account_count,
                value_count,
            ))
        })?;

    let path_string = state.db_path.to_string_lossy().to_string();
    Ok(DataHealth {
        db_path: path_string.clone(),
        is_icloud: path_string.contains("/Mobile Documents/com~apple~CloudDocs/"),
        integrity_status,
        last_fetch_at,
        last_backup_at,
        account_count,
        value_count,
        pending_conflict_copies: count_conflict_copies(&state.db_path),
    })
}

/// Return the precise fetch timestamp when available. Databases created before
/// `last_fetch_at` was introduced fall back to the latest date-only fetch log.
fn get_last_fetch_at(conn: &Connection) -> Result<Option<String>, rusqlite::Error> {
    let latest_date: Option<String> =
        conn.query_row("SELECT MAX(fetched_at) FROM fetch_log", [], |row| {
            row.get(0)
        })?;
    let precise: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'last_fetch_at'",
            [],
            |row| row.get(0),
        )
        .ok();

    if let Some(value) = precise {
        let precise_local_date = chrono::DateTime::parse_from_rfc3339(&value)
            .ok()
            .map(|date| date.with_timezone(&Local).format("%Y-%m-%d").to_string());
        if latest_date.is_none() || precise_local_date == latest_date {
            return Ok(Some(value));
        }
    }
    Ok(latest_date)
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

#[tauri::command]
fn restore_backup(state: State<'_, AppState>, backup_path: String) -> Result<String, String> {
    let source = PathBuf::from(&backup_path);
    if !source.is_file() || !backup_path.ends_with(".db.gz") {
        return Err("Select a valid .db.gz backup file".to_string());
    }

    // Take a recoverable snapshot before touching the live database.
    let safety_backup = run_backup(&state)?;
    let _guard = state.local_lock.lock().map_err(|e| e.to_string())?;
    let parent = state
        .db_path
        .parent()
        .ok_or_else(|| "Database path has no parent".to_string())?;
    let stamp = Utc::now().format("%Y%m%d-%H%M%S-%f").to_string();
    let restored_temp = parent.join(format!("my_finances-restore-{}.tmp", stamp));
    let previous_live = parent.join(format!("my_finances-before-restore-{}.db", stamp));

    let unpack_result = (|| -> Result<(), String> {
        let input = fs::File::open(&source).map_err(|e| format!("Failed to open backup: {}", e))?;
        let mut decoder = GzDecoder::new(input);
        let mut output = fs::File::create(&restored_temp)
            .map_err(|e| format!("Failed to prepare restored database: {}", e))?;
        std::io::copy(&mut decoder, &mut output)
            .map_err(|e| format!("Backup is not a valid gzip archive: {}", e))?;
        output
            .sync_all()
            .map_err(|e| format!("Failed to save restored database: {}", e))?;
        validate_restored_db(&restored_temp)
    })();
    if let Err(error) = unpack_result {
        let _ = fs::remove_file(&restored_temp);
        return Err(error);
    }

    fs::rename(&state.db_path, &previous_live)
        .map_err(|e| format!("Failed to preserve current database: {}", e))?;
    if let Err(error) = fs::rename(&restored_temp, &state.db_path) {
        let _ = fs::rename(&previous_live, &state.db_path);
        let _ = fs::remove_file(&restored_temp);
        return Err(format!("Failed to install restored database: {}", error));
    }

    let finalize_result = (|| -> Result<(), String> {
        let conn = Connection::open(&state.db_path)
            .map_err(|e| format!("Failed to open restored database: {}", e))?;
        conn.execute_batch("PRAGMA journal_mode=DELETE;")
            .map_err(|e| format!("Failed to configure restored database: {}", e))?;
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('last_backup_at', ?1)",
            rusqlite::params![Utc::now().to_rfc3339()],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(error) = finalize_result {
        let _ = fs::remove_file(&state.db_path);
        let _ = fs::rename(&previous_live, &state.db_path);
        return Err(format!("Restore was rolled back: {}", error));
    }
    let _ = fs::remove_file(&previous_live);
    Ok(safety_backup)
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
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create backups dir: {}", e))?;
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
             SELECT balance, account_id
             FROM (
                 SELECT balance, account_id,
                        ROW_NUMBER() OVER (
                            PARTITION BY account_id
                            ORDER BY recorded_date DESC, id DESC
                        ) AS rn
                 FROM account_values
             )
             WHERE rn = 1
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
             SELECT balance, account_id
             FROM (
                 SELECT balance, account_id,
                        ROW_NUMBER() OVER (
                            PARTITION BY account_id
                            ORDER BY recorded_date DESC, id DESC
                        ) AS rn
                 FROM account_values
             )
             WHERE rn = 1
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
             SELECT balance, account_id
             FROM (
                 SELECT balance, account_id,
                        ROW_NUMBER() OVER (
                            PARTITION BY account_id
                            ORDER BY recorded_date DESC, id DESC
                        ) AS rn
                 FROM account_values
             )
             WHERE rn = 1
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
    upsert_daily_account_value(conn, account_id, balance, today)
}

/// Store exactly one value for an account on a given day. This is shared by
/// fetched, manual, and virtual accounts so every write path has identical
/// idempotency semantics.
fn upsert_daily_account_value(
    conn: &Connection,
    account_id: &str,
    balance: f64,
    recorded_date: &str,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "DELETE FROM account_values WHERE account_id = ?1 AND recorded_date = ?2",
        rusqlite::params![account_id, recorded_date],
    )?;
    conn.execute(
        "INSERT INTO account_values (account_id, balance, recorded_date)
         VALUES (?1, ?2, ?3)",
        rusqlite::params![account_id, balance, recorded_date],
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

#[cfg(test)]
mod tests {
    use super::*;

    fn akahu_account(
        name: &str,
        status: Option<&str>,
        currency: Option<&str>,
        balance_refreshed_at: Option<&str>,
    ) -> AkahuAccount {
        AkahuAccount {
            id: format!("account-{}", name.to_lowercase().replace(' ', "-")),
            name: name.to_string(),
            account_type: Some("CHECKING".to_string()),
            status: status.map(str::to_string),
            connection: None,
            balance: Some(AkahuBalance {
                current: Some(100.0),
                currency: currency.map(str::to_string),
            }),
            refreshed: Some(AkahuRefreshed {
                balance: balance_refreshed_at.map(str::to_string),
            }),
        }
    }

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn);
        conn.execute(
            "INSERT INTO accounts (id, name, type, institution, currency, is_manual, is_ignored)
             VALUES ('account-1', 'Test', '', '', 'NZD', 1, 0)",
            [],
        )
        .unwrap();
        conn
    }

    fn liability_account(
        interest_rate: Option<f64>,
        repayment_amount: Option<f64>,
        frequency: Option<u32>,
        unit: Option<&str>,
        next_date: Option<&str>,
    ) -> Account {
        Account {
            id: "liability-1".into(),
            name: "Test Liability".into(),
            account_type: "LOAN".into(),
            institution: "Test Bank".into(),
            currency: "NZD".into(),
            is_manual: false,
            is_ignored: false,
            interest_rate,
            repayment_amount,
            repayment_frequency: frequency,
            repayment_frequency_unit: unit.map(str::to_string),
            rate_expiry: Some("2026-12-31".into()),
            initial_next_repayment_date: next_date.map(str::to_string),
        }
    }

    #[test]
    fn other_accounts_summary_returns_only_ignored_real_accounts() {
        let conn = test_db();
        conn.execute_batch(
            "INSERT INTO accounts
                (id, name, type, institution, currency, is_manual, is_ignored)
             VALUES ('ignored-1', 'Archived Savings', 'SAVINGS', 'Test Bank', 'NZD', 0, 1);
             INSERT INTO account_values (account_id, balance, recorded_date)
             VALUES
                ('ignored-1', 100, '2026-08-20'),
                ('ignored-1', 125, '2026-08-21');
             UPDATE accounts SET is_ignored = 1 WHERE id = '__total__';",
        )
        .unwrap();

        let summaries = query_accounts_summary(&conn, true).unwrap();

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].account.id, "ignored-1");
        assert_eq!(summaries[0].latest_balance, Some(125.0));
        assert_eq!(summaries[0].previous_balance, Some(100.0));
        assert_eq!(summaries[0].latest_date.as_deref(), Some("2026-08-21"));
    }

    #[test]
    fn liability_schema_migration_adds_nullable_fields_to_existing_accounts_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE accounts (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, type TEXT NOT NULL DEFAULT '',
                institution TEXT NOT NULL DEFAULT '', currency TEXT NOT NULL DEFAULT 'NZD',
                is_manual INTEGER NOT NULL DEFAULT 0, is_ignored INTEGER NOT NULL DEFAULT 0
             );",
        )
        .unwrap();

        init_db(&conn);
        let columns = {
            let mut stmt = conn.prepare("PRAGMA table_info(accounts)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        for expected in [
            "interest_rate",
            "repayment_amount",
            "repayment_frequency",
            "repayment_frequency_unit",
            "rate_expiry",
            "initial_next_repayment_date",
        ] {
            assert!(columns.iter().any(|column| column == expected));
        }
    }

    #[test]
    fn next_repayment_advances_from_anchor_without_month_end_drift() {
        let anchor = NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        assert_eq!(
            current_next_repayment_date(anchor, 1, "months", today),
            NaiveDate::from_ymd_opt(2026, 3, 31)
        );
    }

    #[test]
    fn zero_interest_payoff_date_counts_from_next_scheduled_payment() {
        let account = liability_account(
            Some(0.0),
            Some(250.0),
            Some(1),
            Some("months"),
            Some("2026-08-20"),
        );
        let result = calculate_repayment_dates(
            &account,
            Some(-1_000.0),
            NaiveDate::from_ymd_opt(2026, 8, 13).unwrap(),
        );
        assert_eq!(result.0.as_deref(), Some("2026-08-20"));
        assert_eq!(result.1.as_deref(), Some("2026-11-20"));
        assert_eq!(result.2, None);
    }

    #[test]
    fn payoff_estimate_uses_current_rate_after_rate_expiry() {
        let account = liability_account(
            Some(12.0),
            Some(100.0),
            Some(1),
            Some("months"),
            Some("2027-01-20"),
        );
        let result = calculate_repayment_dates(
            &account,
            Some(-1_000.0),
            NaiveDate::from_ymd_opt(2027, 1, 13).unwrap(),
        );
        assert_eq!(result.0.as_deref(), Some("2027-01-20"));
        assert!(result.1.is_some());
        assert_eq!(result.2, None);
    }

    #[test]
    fn payoff_estimate_rejects_negative_amortization() {
        let account = liability_account(
            Some(12.0),
            Some(5.0),
            Some(1),
            Some("months"),
            Some("2026-08-20"),
        );
        let result = calculate_repayment_dates(
            &account,
            Some(-1_000.0),
            NaiveDate::from_ymd_opt(2026, 8, 13).unwrap(),
        );
        assert_eq!(result.0.as_deref(), Some("2026-08-20"));
        assert_eq!(result.1, None);
        assert_eq!(
            result.2.as_deref(),
            Some("The repayment does not cover the interest charged each period.")
        );
    }

    #[test]
    fn liabilities_total_dates_are_derived_from_included_accounts() {
        let conn = test_db();
        conn.execute_batch(
            "INSERT INTO accounts
                (id, name, interest_rate, repayment_amount, repayment_frequency,
                 repayment_frequency_unit, initial_next_repayment_date)
             VALUES
                ('loan-a', 'Monthly Loan', 0, 500, 1, 'months', '2026-08-20'),
                ('loan-b', 'Weekly Loan', 0, 200, 1, 'weeks', '2026-08-15'),
                ('ignored-loan', 'Ignored Loan', NULL, NULL, NULL, NULL, NULL);
             UPDATE accounts SET is_ignored = 1 WHERE id = 'ignored-loan';
             INSERT INTO account_values (account_id, balance, recorded_date)
             VALUES
                ('loan-a', -1000, '2026-08-13'),
                ('loan-b', -600, '2026-08-13'),
                ('ignored-loan', -900, '2026-08-13');",
        )
        .unwrap();

        let result = calculate_liabilities_total_repayment_dates(
            &conn,
            NaiveDate::from_ymd_opt(2026, 8, 13).unwrap(),
        )
        .unwrap();
        assert_eq!(result.0.as_deref(), Some("2026-08-15"));
        assert_eq!(result.1.as_deref(), Some("2026-09-20"));
        assert_eq!(result.2, None);

        conn.execute_batch(
            "INSERT INTO accounts (id, name) VALUES ('loan-c', 'Incomplete Loan');
             INSERT INTO account_values (account_id, balance, recorded_date)
             VALUES ('loan-c', -100, '2026-08-13');",
        )
        .unwrap();
        let incomplete = calculate_liabilities_total_repayment_dates(
            &conn,
            NaiveDate::from_ymd_opt(2026, 8, 13).unwrap(),
        )
        .unwrap();
        assert_eq!(incomplete.0.as_deref(), Some("2026-08-15"));
        assert_eq!(incomplete.1, None);
        assert_eq!(
            incomplete.2.as_deref(),
            Some("Complete valid repayment details for: Incomplete Loan.")
        );
    }

    #[test]
    fn conflict_merge_fills_missing_repayment_details() {
        let live_path =
            std::env::temp_dir().join(format!("my-finances-live-{}.db", uuid::Uuid::new_v4()));
        let conflict_path =
            std::env::temp_dir().join(format!("my-finances-conflict-{}.db", uuid::Uuid::new_v4()));
        {
            let live = Connection::open(&live_path).unwrap();
            init_db(&live);
            live.execute(
                "INSERT INTO accounts (id, name) VALUES ('loan-1', 'Loan')",
                [],
            )
            .unwrap();
        }
        {
            let conflict = Connection::open(&conflict_path).unwrap();
            init_db(&conflict);
            conflict
                .execute(
                    "INSERT INTO accounts
                        (id, name, interest_rate, repayment_amount, repayment_frequency,
                         repayment_frequency_unit, initial_next_repayment_date)
                     VALUES ('loan-1', 'Loan', 6.5, 500, 2, 'weeks', '2026-08-20')",
                    [],
                )
                .unwrap();
        }

        let live = Connection::open(&live_path).unwrap();
        merge_one_conflict(&live, &conflict_path).unwrap();
        let details: (f64, f64, u32, String, String) = live
            .query_row(
                "SELECT interest_rate, repayment_amount, repayment_frequency,
                        repayment_frequency_unit, initial_next_repayment_date
                 FROM accounts WHERE id = 'loan-1'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            details,
            (6.5, 500.0, 2, "weeks".into(), "2026-08-20".into())
        );

        drop(live);
        let _ = fs::remove_file(live_path);
        let _ = fs::remove_file(conflict_path);
    }

    #[test]
    fn conflict_merge_accepts_pre_repayment_schema() {
        let live_path =
            std::env::temp_dir().join(format!("my-finances-live-old-{}.db", uuid::Uuid::new_v4()));
        let conflict_path = std::env::temp_dir().join(format!(
            "my-finances-conflict-old-{}.db",
            uuid::Uuid::new_v4()
        ));
        {
            let live = Connection::open(&live_path).unwrap();
            init_db(&live);
        }
        {
            let conflict = Connection::open(&conflict_path).unwrap();
            conflict
                .execute_batch(
                    "CREATE TABLE accounts (
                        id TEXT PRIMARY KEY, name TEXT NOT NULL, type TEXT NOT NULL DEFAULT '',
                        institution TEXT NOT NULL DEFAULT '', currency TEXT NOT NULL DEFAULT 'NZD',
                        is_manual INTEGER NOT NULL DEFAULT 0, is_ignored INTEGER NOT NULL DEFAULT 0
                     );
                     CREATE TABLE account_values (
                        id INTEGER PRIMARY KEY AUTOINCREMENT, account_id TEXT NOT NULL,
                        balance REAL NOT NULL, recorded_date TEXT NOT NULL
                     );
                     CREATE TABLE fetch_log (id INTEGER PRIMARY KEY AUTOINCREMENT, fetched_at TEXT NOT NULL);
                     CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                     INSERT INTO accounts (id, name) VALUES ('old-account', 'Old Account');",
                )
                .unwrap();
        }

        let live = Connection::open(&live_path).unwrap();
        merge_one_conflict(&live, &conflict_path).unwrap();
        let name: String = live
            .query_row(
                "SELECT name FROM accounts WHERE id = 'old-account'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "Old Account");

        drop(live);
        let _ = fs::remove_file(live_path);
        let _ = fs::remove_file(conflict_path);
    }

    #[test]
    fn daily_value_upsert_replaces_same_day_value() {
        let conn = test_db();
        upsert_daily_account_value(&conn, "account-1", 10.0, "2026-07-12").unwrap();
        upsert_daily_account_value(&conn, "account-1", 20.0, "2026-07-12").unwrap();

        let (count, balance): (i64, f64) = conn
            .query_row(
                "SELECT COUNT(*), MAX(balance) FROM account_values
                 WHERE account_id = 'account-1' AND recorded_date = '2026-07-12'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(balance, 20.0);
    }

    #[test]
    fn previous_balance_uses_the_second_newest_stored_value() {
        let values = vec![
            AccountValue {
                id: 1,
                account_id: "account-1".into(),
                balance: 10.0,
                recorded_date: "2026-08-13".into(),
            },
            AccountValue {
                id: 2,
                account_id: "account-1".into(),
                balance: 20.0,
                recorded_date: "2026-08-14".into(),
            },
            AccountValue {
                id: 3,
                account_id: "account-1".into(),
                balance: 30.0,
                recorded_date: "2026-08-15".into(),
            },
        ];

        assert_eq!(previous_stored_balance(&values), Some(20.0));
        assert_eq!(previous_stored_balance(&values[..1]), None);
    }

    #[test]
    fn balance_freshness_accepts_recent_source_timestamp() {
        let now = DateTime::parse_from_rfc3339("2026-08-08T08:30:00+12:00")
            .unwrap()
            .with_timezone(&Utc);
        let accounts = vec![akahu_account(
            "Current Account",
            Some("ACTIVE"),
            Some("NZD"),
            Some("2026-08-08T07:30:00+12:00"),
        )];

        assert!(balance_freshness_issues(&accounts, now, Duration::minutes(65)).is_empty());
    }

    #[test]
    fn balance_freshness_rejects_stale_or_missing_source_timestamp() {
        let now = DateTime::parse_from_rfc3339("2026-08-08T08:30:00+12:00")
            .unwrap()
            .with_timezone(&Utc);
        let accounts = vec![
            akahu_account(
                "Stale Account",
                Some("ACTIVE"),
                Some("NZD"),
                Some("2026-08-07T20:00:00+12:00"),
            ),
            akahu_account("Missing Timestamp", Some("ACTIVE"), Some("NZD"), None),
        ];

        let issues = balance_freshness_issues(&accounts, now, Duration::minutes(65));
        assert_eq!(issues.len(), 2);
        assert!(issues[0].starts_with("Stale Account"));
        assert!(issues[1].starts_with("Missing Timestamp"));
    }

    #[test]
    fn balance_freshness_ignores_inactive_and_non_nzd_accounts() {
        let now = DateTime::parse_from_rfc3339("2026-08-08T08:30:00+12:00")
            .unwrap()
            .with_timezone(&Utc);
        let accounts = vec![
            akahu_account(
                "Inactive Account",
                Some("INACTIVE"),
                Some("NZD"),
                Some("2025-01-01T00:00:00+12:00"),
            ),
            akahu_account(
                "Foreign Account",
                Some("ACTIVE"),
                Some("USD"),
                Some("2025-01-01T00:00:00+12:00"),
            ),
        ];

        assert!(balance_freshness_issues(&accounts, now, Duration::minutes(65)).is_empty());
    }

    #[tokio::test]
    async fn refresh_wait_timeout_is_a_non_fatal_incomplete_result() {
        let completed =
            wait_for_refresh_with_timeout(std::future::pending::<Result<bool, String>>(), 0)
                .await
                .unwrap();

        assert!(!completed);
    }

    #[test]
    fn totals_use_latest_date_not_highest_insert_id() {
        let conn = test_db();
        // Simulate conflict merging: an older row is inserted after the current
        // row and therefore receives the larger auto-increment id.
        upsert_daily_account_value(&conn, "account-1", 200.0, "2026-07-12").unwrap();
        upsert_daily_account_value(&conn, "account-1", 100.0, "2026-01-01").unwrap();

        recompute_total(&conn, "2026-07-12").unwrap();
        let total: f64 = conn
            .query_row(
                "SELECT balance FROM account_values
                 WHERE account_id = '__total__' AND recorded_date = '2026-07-12'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(total, 200.0);
    }

    #[test]
    fn last_fetch_time_prefers_precise_setting_and_falls_back_to_fetch_date() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO fetch_log (fetched_at) VALUES ('2026-07-12')",
            [],
        )
        .unwrap();
        assert_eq!(
            get_last_fetch_at(&conn).unwrap().as_deref(),
            Some("2026-07-12")
        );

        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('last_fetch_at', '2026-07-12T08:42:15+00:00')",
            [],
        )
        .unwrap();
        assert_eq!(
            get_last_fetch_at(&conn).unwrap().as_deref(),
            Some("2026-07-12T08:42:15+00:00")
        );

        conn.execute(
            "INSERT INTO fetch_log (fetched_at) VALUES ('2026-07-13')",
            [],
        )
        .unwrap();
        assert_eq!(
            get_last_fetch_at(&conn).unwrap().as_deref(),
            Some("2026-07-13")
        );
    }

    #[test]
    fn restore_validation_accepts_app_database_and_rejects_wrong_schema() {
        let valid_path =
            std::env::temp_dir().join(format!("my-finances-valid-{}.db", uuid::Uuid::new_v4()));
        {
            let conn = Connection::open(&valid_path).unwrap();
            init_db(&conn);
        }
        assert!(validate_restored_db(&valid_path).is_ok());

        let invalid_path =
            std::env::temp_dir().join(format!("my-finances-invalid-{}.db", uuid::Uuid::new_v4()));
        {
            let conn = Connection::open(&invalid_path).unwrap();
            conn.execute("CREATE TABLE unrelated (id INTEGER)", [])
                .unwrap();
        }
        assert!(validate_restored_db(&invalid_path).is_err());

        let _ = fs::remove_file(valid_path);
        let _ = fs::remove_file(invalid_path);
    }
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
        .plugin(tauri_plugin_dialog::init())
        .setup({
            let db_path = db_path.clone();
            move |app| {
                // Rebuild the watcher with a handler that emits a Tauri event.
                let app_handle = app.handle().clone();
                let mut watcher = notify::recommended_watcher(
                    move |_res: Result<notify::Event, notify::Error>| {
                        // Debounce in the frontend; just emit on every event.
                        let _ = app_handle.emit("db-changed", ());
                    },
                )
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
            get_other_accounts_summary,
            get_account_history,
            get_account_changes,
            get_all_accounts_config,
            toggle_ignore_account,
            add_manual_account,
            update_liability_repayment,
            add_manual_account_value,
            delete_last_manual_value,
            has_akahu_credentials,
            save_akahu_credentials,
            get_app_version,
            get_autofetch_setting,
            set_autofetch_setting,
            get_backup_settings,
            get_data_health,
            set_backup_frequency,
            do_backup_now,
            restore_backup,
            maybe_backup_on_launch,
            open_backups_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
