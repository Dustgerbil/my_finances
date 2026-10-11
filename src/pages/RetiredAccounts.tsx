import { useReadOnly } from "../sync";
import { useCallback, useEffect, useState } from "react";
import { useNavigate } from "react-router";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AccountSummary } from "../types";

export default function RetiredAccounts() {
  const readOnly = useReadOnly();
  const navigate = useNavigate();
  const [accounts, setAccounts] = useState<AccountSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const loadAccounts = useCallback(async () => {
    try {
      const data = await invoke<AccountSummary[]>("get_accounts_summary");
      setAccounts(data.filter((item) => item.is_retired));
      setError(null);
    } catch (err) {
      setError(`Failed to load retired accounts: ${err}`);
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => {
    void loadAccounts();
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let timer: number | undefined;
    listen("db-changed", () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => void loadAccounts(), 500);
    }).then((stop) => { if (disposed) stop(); else unlisten = stop; });
    return () => { disposed = true; unlisten?.(); window.clearTimeout(timer); };
  }, [loadAccounts]);
  return (
    <div>
      <button className="btn btn-back" onClick={() => navigate("/")}>← Back to Home</button>
      <div className="page-header">
        <h1>Retired Accounts</h1>
        <p>Closed accounts kept for their history. Their balances still count toward virtual-account totals and progress.</p>
        {!readOnly && <p>Open an account and turn off Retired to return it to the home screen.</p>}
      </div>
      {error && <div className="status status-error" role="alert">{error}</div>}
      {loading ? <div className="empty-state">Loading accounts…</div> : accounts.length === 0 ? (
        <div className="empty-state"><p>No retired accounts.</p>{!readOnly && <p>Open an account and use its Retired toggle to move it here.</p>}</div>
      ) : (
        <div className="card-grid">
          {accounts.map((item) => (
            <div key={item.account.id} className="account-card-container">
              <button className="card account-card" onClick={() => navigate(`/account/${encodeURIComponent(item.account.id)}`)}
                aria-label={`View ${item.account.name} account details`}>
                <div className="account-name">{item.account.name}{item.account.is_manual && <span className="manual-badge">Manual</span>}</div>
                <div className="account-institution">{item.account.institution}</div>
                <div className="account-balance">{item.latest_balance == null ? "—" : new Intl.NumberFormat("en-NZ", { style: "currency", currency: item.account.currency }).format(item.latest_balance)}</div>
                {item.latest_date && <div className="account-date">As at {item.latest_date}</div>}
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
