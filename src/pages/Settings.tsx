import { useState, useEffect, useCallback } from "react";
import { useNavigate } from "react-router";
import { invoke } from "@tauri-apps/api/core";
import { confirm, open } from "@tauri-apps/plugin-dialog";
import { IPhoneSyncSettings } from "../sync";
import type { BackupSettings, DataHealth } from "../types";

export default function Settings() {
  const navigate = useNavigate();
  const [appId, setAppId] = useState("");
  const [userToken, setUserToken] = useState("");
  const [saving, setSaving] = useState(false);
  const [version, setVersion] = useState("");
  const [autofetch, setAutofetch] = useState(false);
  const [autofetchLoading, setAutofetchLoading] = useState(true);
  const [backup, setBackup] = useState<BackupSettings | null>(null);
  const [backupLoading, setBackupLoading] = useState(false);
  const [backingUp, setBackingUp] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const [restorePath, setRestorePath] = useState<string | null>(null);
  const [health, setHealth] = useState<DataHealth | null>(null);
  const [healthLoading, setHealthLoading] = useState(true);
  const [healthError, setHealthError] = useState<string | null>(null);
  const [status, setStatus] = useState<{
    type: "success" | "error";
    message: string;
  } | null>(null);

  useEffect(() => {
    invoke<string>("get_app_version").then(setVersion).catch(() => setVersion("unknown"));
  }, []);

  useEffect(() => {
    invoke<boolean>("get_autofetch_setting")
      .then(setAutofetch)
      .catch(() => setAutofetch(false))
      .finally(() => setAutofetchLoading(false));
  }, []);

  const loadBackupSettings = () => {
    setBackupLoading(true);
    invoke<BackupSettings>("get_backup_settings")
      .then(setBackup)
      .catch(() => setBackup(null))
      .finally(() => setBackupLoading(false));
  };

  useEffect(() => {
    loadBackupSettings();
  }, []);

  const loadHealth = useCallback(async () => {
    setHealthLoading(true);
    setHealthError(null);
    try {
      setHealth(await invoke<DataHealth>("get_data_health"));
    } catch (err) {
      setHealthError(`Could not check data health: ${err}`);
    } finally {
      setHealthLoading(false);
    }
  }, []);

  useEffect(() => {
    loadHealth();
  }, [loadHealth]);

  const handleFrequencyChange = async (frequency: string) => {
    const prev = backup;
    setBackup((b) => (b ? { ...b, frequency } : b));
    try {
      await invoke("set_backup_frequency", { frequency });
    } catch (err) {
      setBackup(prev);
      setStatus({ type: "error", message: `Failed to save setting: ${err}` });
    }
  };

  const handleBackupNow = async () => {
    setBackingUp(true);
    setStatus(null);
    try {
      const name = await invoke<string>("do_backup_now");
      setStatus({ type: "success", message: `Backup created: ${name}` });
      loadBackupSettings();
    } catch (err) {
      setStatus({ type: "error", message: `Backup failed: ${err}` });
    } finally {
      setBackingUp(false);
    }
  };

  const handleOpenBackups = async () => {
    try {
      await invoke("open_backups_folder");
    } catch (err) {
      setStatus({ type: "error", message: `Failed to open folder: ${err}` });
    }
  };

  const handleChooseRestore = async () => {
    const selected = await open({
      multiple: false,
      title: "Choose a My Finances backup",
      filters: [{ name: "My Finances backup", extensions: ["gz"] }],
    });
    if (typeof selected === "string") setRestorePath(selected);
  };

  const handleRestore = async () => {
    if (!restorePath) return;
    const confirmed = await confirm(
      "A safety backup of the current database will be created first.",
      { title: "Restore this backup?", kind: "warning" },
    );
    if (!confirmed) return;
    setRestoring(true);
    setStatus(null);
    try {
      const safetyBackup = await invoke<string>("restore_backup", { backupPath: restorePath });
      setStatus({
        type: "success",
        message: `Backup restored. Pre-restore safety backup: ${safetyBackup}`,
      });
      setRestorePath(null);
      loadBackupSettings();
    } catch (err) {
      setStatus({ type: "error", message: `Restore failed: ${err}` });
    } finally {
      setRestoring(false);
    }
  };

  const formatBackupTime = (iso: string | null) => {
    if (!iso) return "Never";
    const d = new Date(iso);
    if (isNaN(d.getTime())) return iso;
    return d.toLocaleString(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  };

  const formatHealthDate = (value: string | null) => {
    if (!value) return "Never";
    const isDateOnly = /^\d{4}-\d{2}-\d{2}$/.test(value);
    const date = new Date(isDateOnly ? `${value}T00:00:00` : value);
    if (Number.isNaN(date.getTime())) return value;
    return isDateOnly ? date.toLocaleDateString() : date.toLocaleString();
  };

  const handleSave = async () => {
    if (!appId.trim() || !userToken.trim()) {
      setStatus({ type: "error", message: "Both fields are required" });
      return;
    }

    setSaving(true);
    setStatus(null);
    try {
      await invoke("save_akahu_credentials", {
        appId: appId.trim(),
        userToken: userToken.trim(),
      });
      setStatus({ type: "success", message: "Credentials saved to Keychain" });
      setAppId("");
      setUserToken("");
    } catch (err) {
      setStatus({ type: "error", message: `Error: ${err}` });
    } finally {
      setSaving(false);
    }
  };

  const handleAutofetchToggle = async (enabled: boolean) => {
    setAutofetch(enabled);
    try {
      await invoke("set_autofetch_setting", { enabled });
    } catch (err) {
      setAutofetch(!enabled);
      setStatus({ type: "error", message: `Failed to save setting: ${err}` });
    }
  };

  return (
    <div>
      <button className="btn btn-back" onClick={() => navigate("/")}>
        ← Back to Dashboard
      </button>

      <div className="config-header">
        <h1>Settings</h1>
      </div>

      {status && (
        <div className={`status status-${status.type}`}>{status.message}</div>
      )}

      <div className="card config-item">
        <div className="config-item-info">
          <div className="account-name">Accounts Configuration</div>
          <div className="account-institution">
            Choose visible accounts and add manual accounts
          </div>
        </div>
        <button className="btn" onClick={() => navigate("/config")}>
          Manage accounts
        </button>
      </div>

      <div className="card add-manual-form">
        <h3>Akahu Credentials</h3>
        <p style={{ fontSize: "0.85rem", color: "var(--text-secondary)", marginBottom: "8px" }}>
          These are stored securely in your macOS Keychain.
          They'll be available no matter where you launch the app from.
        </p>

        <div className="form-group">
          <label htmlFor="appId">App ID</label>
          <input
            id="appId"
            type="text"
            placeholder="app_token_..."
            value={appId}
            onChange={(e) => setAppId(e.target.value)}
          />
        </div>

        <div className="form-group">
          <label htmlFor="userToken">User Token</label>
          <input
            id="userToken"
            type="password"
            placeholder="user_token_..."
            value={userToken}
            onChange={(e) => setUserToken(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && handleSave()}
          />
        </div>

        <button
          className="btn btn-primary"
          onClick={handleSave}
          disabled={saving || !appId.trim() || !userToken.trim()}
        >
          {saving ? "Saving..." : "Save to Keychain"}
        </button>
      </div>

      <div className="card config-item">
        <div className="config-item-info">
          <div className="account-name">Auto-fetch on open</div>
          <div className="account-institution">
            Automatically fetch latest balances when you open the app
          </div>
        </div>
        <label className="toggle">
          <input
            type="checkbox"
            checked={autofetch}
            disabled={autofetchLoading}
            onChange={(e) => handleAutofetchToggle(e.target.checked)}
          />
          <span className="slider" />
        </label>
      </div>

      <div className="card" style={{ marginTop: "16px" }}>
        <h3>Backups</h3>
        <p style={{ fontSize: "0.85rem", color: "var(--text-secondary)", marginBottom: "12px" }}>
          Snapshots of your database are stored locally as compressed <code>.db.gz</code> files.
          They are kept separate from iCloud so they survive even if the live database is lost.
        </p>

        <div className="form-group">
          <label htmlFor="backupFreq">Backup frequency</label>
          <select
            id="backupFreq"
            value={backup?.frequency ?? "off"}
            disabled={backupLoading}
            onChange={(e) => handleFrequencyChange(e.target.value)}
          >
            <option value="off">Off</option>
            <option value="daily">Daily (keeps last 30)</option>
            <option value="weekly">Weekly (keeps last 12)</option>
            <option value="monthly">Monthly (keeps last 24)</option>
          </select>
        </div>

        <div style={{ fontSize: "0.85rem", color: "var(--text-secondary)", marginBottom: "12px" }}>
          Last backup: {backup ? formatBackupTime(backup.last_backup_at) : "…"}
          <br />
          Location: {backup?.backup_dir ?? "…"}{backup?.is_default_dir ? " (default)" : ""}
        </div>

        <div style={{ display: "flex", gap: "8px", flexWrap: "wrap" }}>
          <button
            className="btn btn-primary"
            onClick={handleBackupNow}
            disabled={backingUp}
          >
            {backingUp ? "Backing up…" : "Back up now"}
          </button>
          <button className="btn" onClick={handleOpenBackups}>
            Open backups folder
          </button>
        </div>

        <div className="restore-section">
          <h4>Restore a backup</h4>
          <p>
            The selected backup is checked before it replaces the current database.
            A fresh safety backup is created automatically.
          </p>
          <div className="restore-actions">
            <button className="btn" onClick={handleChooseRestore} disabled={restoring}>
              Choose backup…
            </button>
            {restorePath && (
              <button className="btn btn-danger" onClick={handleRestore} disabled={restoring}>
                {restoring ? "Restoring…" : "Restore selected backup"}
              </button>
            )}
          </div>
          {restorePath && <div className="restore-path">{restorePath}</div>}
        </div>
      </div>

      <IPhoneSyncSettings />

      <section className="card data-health" aria-labelledby="data-health-title">
        <div className="data-health-header">
          <div>
            <h2 id="data-health-title">Data Health</h2>
            <p>Database, backup, and iCloud synchronization status</p>
          </div>
          <button className="btn" onClick={loadHealth} disabled={healthLoading}>
            {healthLoading ? "Checking…" : "Refresh"}
          </button>
        </div>

        {healthError ? (
          <div className="status status-error">{healthError}</div>
        ) : health ? (
          <div className="data-health-grid">
            <div className="health-item">
              <span>Database integrity</span>
              <strong className={health.integrity_status === "ok" ? "health-good" : "health-bad"}>
                {health.integrity_status === "ok" ? "Healthy" : health.integrity_status}
              </strong>
            </div>
            <div className="health-item">
              <span>Storage</span>
              <strong>{health.is_icloud ? "iCloud Drive" : "Local only"}</strong>
            </div>
            <div className="health-item">
              <span>Last successful fetch</span>
              <strong>{formatHealthDate(health.last_fetch_at)}</strong>
            </div>
            <div className="health-item">
              <span>Last backup</span>
              <strong>{formatHealthDate(health.last_backup_at)}</strong>
            </div>
            <div className="health-item">
              <span>Accounts / values</span>
              <strong>{health.account_count} / {health.value_count}</strong>
            </div>
            <div className="health-item">
              <span>Pending conflict copies</span>
              <strong className={health.pending_conflict_copies === 0 ? "health-good" : "health-warning"}>
                {health.pending_conflict_copies}
              </strong>
            </div>
            <div className="health-path">
              <span>Database location</span>
              <code>{health.db_path}</code>
            </div>
          </div>
        ) : (
          <p>Checking database…</p>
        )}
      </section>

      <div className="card" style={{ marginTop: "16px", textAlign: "center" }}>
        <p style={{ fontSize: "0.8rem", color: "var(--text-secondary)" }}>
          Version: {version || "…"}
        </p>
      </div>
    </div>
  );
}
