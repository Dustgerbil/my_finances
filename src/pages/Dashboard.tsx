import { useState, useEffect, useCallback, useRef } from "react";
import { useNavigate } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AccountSummary, AccountChange } from "../types";

export default function Dashboard() {
  const navigate = useNavigate();
  const [accounts, setAccounts] = useState<AccountSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [fetching, setFetching] = useState(false);
  const [changes, setChanges] = useState<Record<string, AccountChange[]>>({});
  const [status, setStatus] = useState<{
    type: "loading" | "success" | "error";
    message: string;
  } | null>(null);
  const [hasCredentials, setHasCredentials] = useState<boolean | null>(null);
  const [autofetch, setAutofetch] = useState(false);
  const didAutofetch = useRef(false);

  const checkCredentials = useCallback(async () => {
    try {
      const ok = await invoke<boolean>("has_akahu_credentials");
      setHasCredentials(ok);
    } catch {
      setHasCredentials(false);
    }
  }, []);

  const loadAccounts = useCallback(async () => {
    setLoading(true);
    try {
      const data = await invoke<AccountSummary[]>("get_accounts_summary");
      setAccounts(data);
      const changePromises = data.map(async (item) => {
        try {
          const ch = await invoke<AccountChange[]>("get_account_changes", {
            accountId: item.account.id,
          });
          return { id: item.account.id, changes: ch };
        } catch {
          return { id: item.account.id, changes: [] };
        }
      });
      const results = await Promise.all(changePromises);
      const changeMap: Record<string, AccountChange[]> = {};
      for (const r of results) {
        changeMap[r.id] = r.changes;
      }
      setChanges(changeMap);
    } catch (err) {
      setStatus({ type: "error", message: `Failed to load: ${err}` });
    } finally {
      setLoading(false);
    }
  }, []);

  // Auto-fetch on first load if enabled
  useEffect(() => {
    const init = async () => {
      const enabled = await invoke<boolean>("get_autofetch_setting").catch(() => false);
      setAutofetch(enabled);

      // Kick off a backup if a frequency is set and one is due (fire-and-forget;
      // mirrors the autofetch pattern). Runs in parallel with the fetch below.
      invoke<boolean>("maybe_backup_on_launch").catch(() => {});

      if (enabled && !didAutofetch.current) {
        didAutofetch.current = true;
        const credsOk = await invoke<boolean>("has_akahu_credentials").catch(() => false);
        if (credsOk) {
          try {
            const result = await invoke<string>("fetch_akahu_balances");
            // Don't show "Already fetched today" during auto-fetch
            if (result !== "Already fetched today") {
              setStatus({ type: "success", message: result });
            }
          } catch (err) {
            setStatus({ type: "error", message: `Auto-fetch failed: ${err}` });
          }
        }
      }
    };
    init();
    checkCredentials();
  }, [checkCredentials]);

  useEffect(() => {
    loadAccounts();
  }, [loadAccounts]);

  // Listen for remote DB changes (another machine wrote and iCloud synced it
  // down). Debounce: coalesce a burst of filesystem events from one sync into
  // a single reload. Also fires on our own writes, which is harmless.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let timer: number | undefined;
    listen("db-changed", () => {
      if (timer) clearTimeout(timer);
      timer = window.setTimeout(() => {
        loadAccounts();
      }, 500);
    }).then((u) => (unlisten = u));
    return () => {
      unlisten?.();
      if (timer) clearTimeout(timer);
    };
  }, [loadAccounts]);

  const handleFetch = async () => {
    setFetching(true);
    setStatus({ type: "loading", message: "Fetching from Akahu..." });
    try {
      const result = await invoke<string>("fetch_akahu_balances");
      setStatus({ type: "success", message: result });
      await loadAccounts();
    } catch (err) {
      setStatus({ type: "error", message: `Error: ${err}` });
    } finally {
      setFetching(false);
    }
  };

  const formatCurrency = (val: number | null | undefined) => {
    if (val == null) return "—";
    const abs = Math.abs(val);
    const prefix = val < 0 ? "-" : "";
    return `${prefix}$${abs.toLocaleString(undefined, {
      minimumFractionDigits: 2,
      maximumFractionDigits: 2,
    })}`;
  };

  const formatDate = (d: string | null | undefined) => {
    if (!d) return "";
    return new Date(d).toLocaleDateString();
  };

  // Split accounts: total, positive, negative, with sub-totals
  const totalAccount = accounts.find((a) => a.account.id === "__total__");
  const assetsTotal = accounts.find((a) => a.account.id === "__assets_total__");
  const liabilitiesTotal = accounts.find((a) => a.account.id === "__liabilities_total__");
  const positiveAccounts = accounts.filter(
    (a) => !a.account.id.startsWith("__") && (a.latest_balance ?? 0) >= 0
  );
  const negativeAccounts = accounts.filter(
    (a) => !a.account.id.startsWith("__") && (a.latest_balance ?? 0) < 0
  );

  const renderCard = (item: AccountSummary, isNegative = false) => {
    const id = item.account.id;
    const isTotal = id === "__total__";
    const isSubTotal = id === "__assets_total__" || id === "__liabilities_total__";
    const isAtMax = item.latest_balance != null && item.max_balance != null && item.latest_balance === item.max_balance;
    const cls = [
      isTotal ? " total-card" : "",
      isSubTotal ? " subtotal-card" : "",
      id === "__assets_total__" ? " assets-subtotal" : "",
      id === "__liabilities_total__" ? " liabilities-subtotal" : "",
      isNegative ? " negative" : "",
    ].join("");
    return (
      <div
        key={item.account.id}
        className={`card account-card${cls}`}
        onClick={() => navigate(`/account/${encodeURIComponent(item.account.id)}`)}
      >
        <div className="account-name">
          {item.account.name}
          {isAtMax && <span className="alltime-high" title="All-time high"> ⭐</span>}
          {item.account.is_manual && (
            <span className="manual-badge">Manual</span>
          )}
        </div>
        <div className="account-institution">
          {item.account.institution}
          {item.account.type ? ` · ${item.account.type}` : ""}
        </div>
        <div className="account-balance">
          {formatCurrency(item.latest_balance)}
        </div>
        {item.latest_date && (
          <div className="account-date">
            As at {formatDate(item.latest_date)}
          </div>
        )}
        {changes[item.account.id] && changes[item.account.id].length > 0 && (
          <div className="account-changes">
            {changes[item.account.id].map((ch) => {
              if (ch.change_amount == null) return null;
              const symbol =
                ch.period === "30d" ? "30d" : ch.period === "180d" ? "180d" : "360d";
              const sign = ch.change_amount > 0 ? "+" : "";
              return (
                <span
                  key={ch.period}
                  className={`change-chip ${ch.change_amount > 0 ? "up" : ch.change_amount < 0 ? "down" : ""}`}
                  title={`${symbol}: ${sign}${formatCurrency(ch.change_amount)}`}
                >
                  {symbol} {sign}{formatCurrency(ch.change_amount)}
                </span>
              );
            })}
          </div>
        )}
      </div>
    );
  };

  return (
    <div>
      <div className="header">
        <h1>My Finances</h1>
        <div className="header-actions">
          {!autofetch && (
            <button
              className="btn btn-primary"
              onClick={handleFetch}
              disabled={fetching}
            >
              {fetching ? "Fetching..." : "Fetch from Akahu"}
            </button>
          )}
          <button
            className="btn btn-secondary"
            onClick={() => navigate("/config")}
          >
            Accounts Config
          </button>
          <button
            className="btn btn-secondary"
            onClick={() => navigate("/settings")}
            title="Settings"
          >
            ⚙
          </button>
        </div>
      </div>

      {status && (
        <div className={`status status-${status.type}`}>{status.message}</div>
      )}

      {hasCredentials === false && (
        <div className="status status-error" style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
          <span>⚠ Akahu credentials not found. Please add them in Settings.</span>
          <button className="btn btn-primary" onClick={() => navigate("/settings")} style={{ marginLeft: "12px", whiteSpace: "nowrap" }}>
            Add Credentials
          </button>
        </div>
      )}

      {loading ? (
        <div className="empty-state">
          <p>Loading accounts...</p>
        </div>
      ) : accounts.length === 0 ? (
        <div className="empty-state">
          <p>No accounts yet.</p>
          <p>Click "Fetch from Akahu" to get started, or add a manual account in config.</p>
        </div>
      ) : (
        <>
          {/* Total section */}
          {totalAccount && (
            <div className="card-grid">{renderCard(totalAccount)}</div>
          )}

          {/* Positive balances */}
          {positiveAccounts.length > 0 && (
            <>
              <h2 className="section-title">Assets &amp; Savings</h2>
              <div className="card-grid">
                {assetsTotal && renderCard(assetsTotal)}
                {positiveAccounts.map((a) => renderCard(a))}
              </div>
            </>
          )}

          {/* Negative balances */}
          {negativeAccounts.length > 0 && (
            <>
              <h2 className="section-title">Liabilities</h2>
              <div className="card-grid">
                {liabilitiesTotal && renderCard(liabilitiesTotal, true)}
                {negativeAccounts.map((a) => renderCard(a, true))}
              </div>
            </>
          )}
        </>
      )}
    </div>
  );
}