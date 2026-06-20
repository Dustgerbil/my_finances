import { useState, useEffect } from "react";
import { useNavigate } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import type { BackupSettings } from "../types";

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
      </div>

      <div className="card" style={{ marginTop: "16px", textAlign: "center" }}>
        <p style={{ fontSize: "0.8rem", color: "var(--text-secondary)" }}>
          Version: {version || "…"}
        </p>
      </div>
    </div>
  );
}
