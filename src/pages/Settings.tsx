import { useState, useEffect } from "react";
import { useNavigate } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";

export default function Settings() {
  const navigate = useNavigate();
  const [appId, setAppId] = useState("");
  const [userToken, setUserToken] = useState("");
  const [saving, setSaving] = useState(false);
  const [version, setVersion] = useState("");
  const [autofetch, setAutofetch] = useState(false);
  const [autofetchLoading, setAutofetchLoading] = useState(true);
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

  const unixToReadable = (ts: string) => {
    const n = Number(ts);
    if (!n) return ts;
    const d = new Date(n * 1000);
    const pad = (v: number) => String(v).padStart(2, "0");
    return `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}${pad(d.getHours())}${pad(d.getMinutes())}${pad(d.getSeconds())}`;
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

      <div className="card" style={{ marginTop: "16px", textAlign: "center" }}>
        <p style={{ fontSize: "0.8rem", color: "var(--text-secondary)" }}>
          Version: {version ? unixToReadable(version) : "..."}
        </p>
      </div>
    </div>
  );
}
