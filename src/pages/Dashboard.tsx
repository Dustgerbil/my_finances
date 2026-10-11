import { useReadOnly } from "../sync";
import { useState, useEffect, useCallback, useRef } from "react";
import { useNavigate } from "react-router";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Cell, Pie, PieChart } from "recharts";
import type { AccountSummary, AccountChange } from "../types";
import { formatAccountType } from "../format";
import BalanceDirection from "../components/BalanceDirection";
import { daysUntilRateExpiry } from "../rateExpiry";

const ACCOUNT_MIX_COLORS = ["#4f46e5", "#0ea5e9", "#14b8a6", "#f59e0b", "#94a3b8"];
const MAX_NAMED_ACCOUNT_SLICES = 4;

interface AccountMixSlice {
  name: string;
  value: number;
  percentage: number;
  color: string;
}

function buildAccountMix(items: AccountSummary[]): AccountMixSlice[] {
  const balances = items
    .flatMap((item) => {
      const balance = item.latest_balance;
      return balance != null && Number.isFinite(balance) && balance !== 0
        ? [{ name: item.account.name, value: Math.abs(balance) }]
        : [];
    })
    .sort((a, b) => b.value - a.value);

  const total = balances.reduce((sum, item) => sum + item.value, 0);
  if (total === 0) return [];

  const visible = balances.slice(0, MAX_NAMED_ACCOUNT_SLICES);
  const otherValue = balances
    .slice(MAX_NAMED_ACCOUNT_SLICES)
    .reduce((sum, item) => sum + item.value, 0);
  if (otherValue > 0) visible.push({ name: "Other", value: otherValue });

  return visible.map((item, index) => ({
    ...item,
    percentage: (item.value / total) * 100,
    color: ACCOUNT_MIX_COLORS[index],
  }));
}

export default function Dashboard() {
  const readOnly = useReadOnly();
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
  const didAutofetch = useRef(false);
  const [today, setToday] = useState(() => new Date());

  useEffect(() => {
    const updateToday = () => setToday(new Date());
    const timer = window.setInterval(updateToday, 60_000);
    window.addEventListener("focus", updateToday);
    document.addEventListener("visibilitychange", updateToday);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", updateToday);
      document.removeEventListener("visibilitychange", updateToday);
    };
  }, []);

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
    if (readOnly) return;
    const init = async () => {
      const enabled = await invoke<boolean>("get_autofetch_setting").catch(() => false);

      // Kick off a backup if a frequency is set and one is due (fire-and-forget;
      // mirrors the autofetch pattern). Runs in parallel with the fetch below.
      invoke<boolean>("maybe_backup_on_launch").catch(() => {});

      if (enabled && !didAutofetch.current) {
        didAutofetch.current = true;
        const credsOk = await invoke<boolean>("has_akahu_credentials").catch(() => false);
        if (credsOk) {
          try {
            const result = await invoke<string>("fetch_akahu_balances", {
              force: false,
            });
            // Don't show "Already fetched today" during auto-fetch
            if (result !== "Already fetched today") {
              setStatus({ type: "success", message: result });
              await loadAccounts();
            }
          } catch (err) {
            setStatus({ type: "error", message: `Auto-fetch failed: ${err}` });
          }
        }
      }
    };
    init();
    checkCredentials();
  }, [checkCredentials, loadAccounts, readOnly]);

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
    setStatus({ type: "loading", message: "Refreshing from Akahu..." });
    try {
      const result = await invoke<string>("fetch_akahu_balances", {
        force: true,
      });
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

  const formatCompactCurrency = (val: number) => {
    const abs = Math.abs(val);
    const prefix = val < 0 ? "−" : val > 0 ? "+" : "";
    return `${prefix}$${new Intl.NumberFormat(undefined, {
      notation: "compact",
      maximumFractionDigits: 1,
    }).format(abs)}`;
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
  const expiringRates = negativeAccounts.flatMap((item) => {
    if (item.is_retired || item.account.is_ignored) return [];
    const days = daysUntilRateExpiry(item.account.rate_expiry, today);
    return days != null && days >= 0 && days < 30 ? [{ item, days }] : [];
  }).sort((a, b) => a.days - b.days);

  const renderGrandTotalWaterfall = () => {
    const assets = assetsTotal?.latest_balance;
    const liabilities = liabilitiesTotal?.latest_balance;
    const netTotal = totalAccount?.latest_balance;

    if (assets == null && liabilities == null && netTotal == null) {
      return (
        <div className="virtual-composition">
          <div className="virtual-composition-empty">No balances yet</div>
        </div>
      );
    }

    const assetValue = Number.isFinite(assets) ? Math.max(assets ?? 0, 0) : 0;
    const liabilityValue = Number.isFinite(liabilities) ? Math.min(liabilities ?? 0, 0) : 0;
    const netValue = Number.isFinite(netTotal)
      ? netTotal ?? 0
      : assetValue + liabilityValue;
    const maximumValue = Math.max(0, assetValue, netValue);
    const minimumValue = Math.min(0, netValue);
    const valueRange = maximumValue - minimumValue || 1;
    const plotTop = 24;
    const plotBottom = 104;
    const yForValue = (value: number) =>
      plotTop + ((maximumValue - value) / valueRange) * (plotBottom - plotTop);
    const zeroY = yForValue(0);
    const assetY = yForValue(assetValue);
    const netY = yForValue(netValue);
    const barHeight = (from: number, to: number) => Math.max(Math.abs(to - from), 2);
    const chartLabel = `Assets ${formatCurrency(assetValue)}, liabilities ${formatCurrency(
      liabilityValue
    )}, net total ${formatCurrency(netValue)}`;

    return (
      <div className="virtual-composition grand-total-composition">
        <svg
          className="grand-total-waterfall"
          viewBox="0 0 300 140"
          role="img"
          aria-label={chartLabel}
        >
          <line className="waterfall-axis" x1="8" y1={zeroY} x2="292" y2={zeroY} />

          <rect
            className="waterfall-bar waterfall-assets"
            x="18"
            y={Math.min(assetY, zeroY)}
            width="64"
            height={barHeight(assetY, zeroY)}
            rx="5"
          />
          <text className="waterfall-value" x="50" y="14" textAnchor="middle">
            {formatCompactCurrency(assetValue)}
          </text>
          <text className="waterfall-label" x="50" y="126" textAnchor="middle">
            Assets
          </text>

          <line className="waterfall-connector" x1="82" y1={assetY} x2="118" y2={assetY} />
          <rect
            className="waterfall-bar waterfall-liabilities"
            x="118"
            y={Math.min(assetY, netY)}
            width="64"
            height={barHeight(assetY, netY)}
            rx="5"
          />
          <text className="waterfall-value" x="150" y="14" textAnchor="middle">
            {formatCompactCurrency(liabilityValue)}
          </text>
          <text className="waterfall-label" x="150" y="126" textAnchor="middle">
            Liabilities
          </text>

          <line className="waterfall-connector" x1="182" y1={netY} x2="218" y2={netY} />
          <rect
            className="waterfall-bar waterfall-net"
            x="218"
            y={Math.min(netY, zeroY)}
            width="64"
            height={barHeight(netY, zeroY)}
            rx="5"
          />
          <text className="waterfall-value" x="250" y="14" textAnchor="middle">
            {formatCompactCurrency(netValue)}
          </text>
          <text className="waterfall-label" x="250" y="126" textAnchor="middle">
            Net total
          </text>
        </svg>
      </div>
    );
  };

  const renderAccountMix = (items: AccountSummary[]) => {
    const slices = buildAccountMix(items);
    const chartLabel = slices.length > 0
      ? `Largest account proportions by balance size: ${slices
          .map((slice) => `${slice.name} ${Math.round(slice.percentage)} percent`)
          .join(", ")}`
      : "No account balances available for this total";

    return (
      <div className="virtual-composition">
        <div className="virtual-composition-title">Largest accounts</div>
        {slices.length > 0 ? (
          <div className="virtual-composition-body" role="img" aria-label={chartLabel}>
            <PieChart width={116} height={116} className="virtual-pie" aria-hidden="true">
              <Pie
                data={slices}
                dataKey="value"
                nameKey="name"
                cx="50%"
                cy="50%"
                innerRadius={28}
                outerRadius={54}
                paddingAngle={1}
                stroke="#ffffff"
                strokeWidth={2}
                isAnimationActive={false}
              >
                {slices.map((slice) => (
                  <Cell key={slice.name} fill={slice.color} />
                ))}
              </Pie>
            </PieChart>
            <ul className="virtual-composition-legend">
              {slices.map((slice) => (
                <li key={slice.name} title={slice.name}>
                  <span className="virtual-legend-swatch" style={{ backgroundColor: slice.color }} />
                  <span className="virtual-legend-name">{slice.name}</span>
                  <span className="virtual-legend-percentage">{Math.round(slice.percentage)}%</span>
                </li>
              ))}
            </ul>
          </div>
        ) : (
          <div className="virtual-composition-empty">No balances yet</div>
        )}
      </div>
    );
  };

  const renderCard = (item: AccountSummary, isNegative = false) => {
    const id = item.account.id;
    const isVirtual = id.startsWith("__");
    const isTotal = id === "__total__";
    const isSubTotal = id === "__assets_total__" || id === "__liabilities_total__";
    const isAtMax = item.latest_balance != null && item.max_balance != null && item.latest_balance === item.max_balance;
    const mixAccounts = id === "__assets_total__"
      ? positiveAccounts
      : id === "__liabilities_total__"
      ? negativeAccounts
      : [];
    const cls = [
      isVirtual ? " virtual-card" : "",
      isTotal ? " total-card" : "",
      isSubTotal ? " subtotal-card" : "",
      id === "__assets_total__" ? " assets-subtotal" : "",
      id === "__liabilities_total__" ? " liabilities-subtotal" : "",
      isNegative ? " negative" : "",
    ].join("");
    return (
      <div key={item.account.id} className={`account-card-container${isVirtual ? " virtual-card" : ""}`}>
      <button
        type="button"
        className={`card account-card${cls}`}
        onClick={() => navigate(`/account/${encodeURIComponent(item.account.id)}`)}
        aria-label={`View ${item.account.name} account details`}
      >
        <div className={isVirtual ? "virtual-card-content" : undefined}>
          <div className="account-card-summary">
            <div className="account-name">
              {item.account.name}
              {item.account.is_manual && (
                <span className="manual-badge">Manual</span>
              )}
            </div>
            <div className="account-institution">
              {item.account.institution}
              {item.account.type ? ` · ${formatAccountType(item.account.type)}` : ""}
            </div>
            <div className="account-balance balance-with-direction">
              <span>{formatCurrency(item.latest_balance)}</span>
              <BalanceDirection
                current={item.latest_balance}
                previous={item.previous_balance}
              />
              {isAtMax && (
                <span className="alltime-high" role="img" aria-label="All-time high" title="All-time high">
                  ⭐
                </span>
              )}
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
                    ch.period === "30d"
                      ? "30d"
                      : ch.period === "90d"
                      ? "90d"
                      : ch.period === "180d"
                      ? "180d"
                      : ch.period === "270d"
                      ? "270d"
                      : ch.period === "360d"
                      ? "360d"
                      : "All";
                  const titleLabel = ch.period === "all" ? "All time" : symbol;
                  const sign = ch.change_amount > 0 ? "+" : "";
                  return (
                    <span
                      key={ch.period}
                      className={`change-chip ${ch.change_amount > 0 ? "up" : ch.change_amount < 0 ? "down" : ""}`}
                      title={`${titleLabel}: ${sign}${formatCurrency(ch.change_amount)}`}
                    >
                      {symbol} {sign}{formatCurrency(ch.change_amount)}
                    </span>
                  );
                })}
              </div>
            )}
          </div>
          {isTotal
            ? renderGrandTotalWaterfall()
            : isVirtual && renderAccountMix(mixAccounts)}
        </div>
      </button>
      </div>
    );
  };

  return (
    <div>
      <div className="header">
        <h1>My Finances</h1>
        <div className="header-actions">
          <button className="btn btn-secondary" onClick={() => navigate("/retired-accounts")}>
            Retired Accounts
          </button>
          <button
            className="btn btn-secondary"
            onClick={() => navigate("/other-accounts")}
          >
            Other Accounts
          </button>
          {!readOnly && <><button
            className="btn btn-primary"
            onClick={handleFetch}
            disabled={fetching}
          >
            {fetching ? "Refreshing..." : "Refresh now"}
          </button>
          <button
            className="btn btn-secondary"
            onClick={() => navigate("/settings")}
            title="Settings"
          >
            ⚙
          </button></>}
        </div>
      </div>

      {status && (
        <div className={`status status-${status.type}`}>{status.message}</div>
      )}

      {expiringRates.length > 0 && (
        <section className="status rate-expiry-notice" aria-label="Upcoming rate expiries" role="status">
          <strong>Rate expiry approaching</strong>
          <ul>
            {expiringRates.map(({ item, days }) => (
              <li key={item.account.id}>
                <a href={`/account/${encodeURIComponent(item.account.id)}`} onClick={(event) => {
                  event.preventDefault();
                  navigate(`/account/${encodeURIComponent(item.account.id)}`);
                }}>{item.account.name}</a>
                {" — rate expires "}{days === 0 ? "today" : `in ${days} ${days === 1 ? "day" : "days"}`}
                {" ("}{new Date(`${item.account.rate_expiry}T00:00:00`).toLocaleDateString()}{")."}
              </li>
            ))}
          </ul>
        </section>
      )}

      {!readOnly && hasCredentials === false && (
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
          <p>{readOnly ? "Accounts will appear after your Mac publishes them." : 'Click "Refresh now" to get started, or add a manual account in config.'}</p>
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
                {positiveAccounts.filter((a) => !a.is_retired).map((a) => renderCard(a))}
              </div>
            </>
          )}

          {/* Negative balances */}
          {negativeAccounts.length > 0 && (
            <>
              <h2 className="section-title">Liabilities</h2>
              <div className="card-grid">
                {liabilitiesTotal && renderCard(liabilitiesTotal, true)}
                {negativeAccounts.filter((a) => !a.is_retired).map((a) => renderCard(a, true))}
              </div>
            </>
          )}
        </>
      )}
    </div>
  );
}
