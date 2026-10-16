import { useLiveData } from "../useLiveData";
import { useCallback, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useNavigate } from "react-router";
import { formatAccountType } from "../format";
import type { AccountSummary } from "../types";

function formatCurrency(value: number | null | undefined): string {
  if (value == null) return "—";
  const absoluteValue = Math.abs(value);
  const prefix = value < 0 ? "-" : "";
  return `${prefix}$${absoluteValue.toLocaleString(undefined, {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  })}`;
}

function formatDate(value: string | null | undefined): string {
  if (!value) return "";
  return new Date(value).toLocaleDateString();
}

export default function OtherAccounts() {
  const navigate = useNavigate();
  const readAccounts = useCallback(async () => {
    const accounts = await invoke<AccountSummary[]>("get_other_accounts_summary");
    return accounts;
  }, []);
  const { data, loading, error } = useLiveData(readAccounts);
  const [individualAssets, setIndividualAssets] = useState<Record<string, boolean>>({});
  const accounts = (data ?? []).map(raw => raw.combined && !individualAssets[raw.account.id] ? { ...raw, ...raw.combined } : raw);
  return (
    <div>
      <button className="btn btn-back" onClick={() => navigate("/")}>
        ← Back to Home
      </button>

      <div className="page-header">
        <h1>Other Accounts</h1>
        <p>Accounts excluded from the home screen.</p>
      </div>

      {error && <div className="status status-error">{error}</div>}

      {loading ? (
        <div className="empty-state">
          <p>Loading accounts...</p>
        </div>
      ) : accounts.length === 0 ? (
        <div className="empty-state">
          <p>No other accounts.</p>
          <p>Accounts excluded from the home screen will appear here.</p>
        </div>
      ) : (
        <div className="card-grid">
          {accounts.map((item) => (
            <div className="card other-account-card" key={item.account.id}>
              <div className="account-name">
                {item.account.name}
                {item.account.is_manual && (
                  <span className="manual-badge">Manual</span>
                )}
              </div>
              <div className="account-institution">
                {item.account.institution}
                {item.account.type
                  ? ` · ${formatAccountType(item.account.type)}`
                  : ""}
              </div>
              {item.combined && <div className="account-date">{individualAssets[item.account.id] ? "This asset only" : `Combined · ${item.combined.account_count} accounts`}</div>}
              <div
                className={`account-balance${
                  (item.latest_balance ?? 0) < 0 ? " negative" : ""
                }`}
              >
                {formatCurrency(item.latest_balance)}
              </div>
              {item.latest_date && (
                <div className="account-date">
                  As at {formatDate(item.latest_date)}
                </div>
              )}
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
