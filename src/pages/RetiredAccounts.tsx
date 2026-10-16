import { useReadOnly } from "../sync";
import { useCallback, useState } from "react";
import { useNavigate } from "react-router";
import { invoke } from "@tauri-apps/api/core";
import { useLiveData } from "../useLiveData";
import type { AccountSummary } from "../types";

export default function RetiredAccounts() {
  const readOnly = useReadOnly();
  const navigate = useNavigate();
  const readAccounts = useCallback(async () => {
    const accounts = await invoke<AccountSummary[]>("get_accounts_summary");
    return accounts.filter((item) => item.is_retired);
  }, []);
  const { data, loading, error } = useLiveData(readAccounts);
  const [individualAssets, setIndividualAssets] = useState<Record<string, boolean>>({});
  const accounts = (data ?? []).map(raw => raw.combined && !individualAssets[raw.account.id] ? { ...raw, ...raw.combined } : raw);
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
              {item.combined && <div className="account-date">{individualAssets[item.account.id] ? "This asset only" : `Combined · ${item.combined.account_count} accounts`}</div>}
                <div className="account-balance">{item.latest_balance == null ? "—" : new Intl.NumberFormat("en-NZ", { style: "currency", currency: item.account.currency }).format(item.latest_balance)}</div>
                {item.latest_date && <div className="account-date">As at {item.latest_date}</div>}
              </button>
              {item.combined && <button className="btn btn-secondary asset-card-toggle" aria-pressed={!!individualAssets[item.account.id]}
                onClick={() => setIndividualAssets(previous => ({ ...previous, [item.account.id]: !previous[item.account.id] }))}>
                {individualAssets[item.account.id] ? "Show combined assets" : "Show this asset only"}
              </button>}

            </div>
          ))}
        </div>
      )}
    </div>
  );
}
