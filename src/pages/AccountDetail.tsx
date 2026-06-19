import { useState, useEffect, useCallback } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  LineChart,
  Line,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
} from "recharts";
import type { AccountHistory, AccountChange } from "../types";

export default function AccountDetail() {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const [data, setData] = useState<AccountHistory | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [changes, setChanges] = useState<AccountChange[]>([]);

  // Add value form state
  const [addValue, setAddValue] = useState("");
  const [adding, setAdding] = useState(false);
  const [addStatus, setAddStatus] = useState<{
    type: "success" | "error";
    message: string;
  } | null>(null);

  const loadHistory = useCallback(async () => {
    if (!id) return;
    setLoading(true);
    try {
      const decodedId = decodeURIComponent(id);
      const [result, changeData] = await Promise.all([
        invoke<AccountHistory>("get_account_history", { accountId: decodedId }),
        invoke<AccountChange[]>("get_account_changes", { accountId: decodedId }),
      ]);
      setData(result);
      setChanges(changeData);
    } catch (err) {
      setError(`Failed to load account: ${err}`);
    } finally {
      setLoading(false);
    }
  }, [id]);

  useEffect(() => {
    loadHistory();
  }, [loadHistory]);

  // Listen for remote DB changes (another machine wrote and iCloud synced it
  // down). Debounce: coalesce a burst of filesystem events into one reload.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let timer: number | undefined;
    listen("db-changed", () => {
      if (timer) clearTimeout(timer);
      timer = window.setTimeout(() => {
        loadHistory();
      }, 500);
    }).then((u) => (unlisten = u));
    return () => {
      unlisten?.();
      if (timer) clearTimeout(timer);
    };
  }, [loadHistory]);

  const handleAddValue = async () => {
    if (!id || !addValue) return;
    const balance = parseFloat(addValue);
    if (isNaN(balance)) {
      setAddStatus({ type: "error", message: "Please enter a valid number" });
      return;
    }

    setAdding(true);
    setAddStatus(null);
    try {
      await invoke("add_manual_account_value", {
        request: {
          account_id: decodeURIComponent(id),
          balance,
        },
      });
      setAddValue("");
      setAddStatus({ type: "success", message: "Value added" });
      await loadHistory();
    } catch (err) {
      setAddStatus({ type: "error", message: `Error: ${err}` });
    } finally {
      setAdding(false);
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

  const formatDate = (d: string) => {
    return new Date(d).toLocaleDateString();
  };

  if (loading) {
    return (
      <div>
        <button className="btn btn-back" onClick={() => navigate("/")}>
          ← Back
        </button>
        <div className="empty-state"><p>Loading...</p></div>
      </div>
    );
  }

  if (error || !data) {
    return (
      <div>
        <button className="btn btn-back" onClick={() => navigate("/")}>
          ← Back
        </button>
        <div className="empty-state">
          <p>{error || "Account not found"}</p>
        </div>
      </div>
    );
  }

  const chartData = data.values.map((v) => ({
    date: v.recorded_date,
    balance: v.balance,
    label: formatDate(v.recorded_date),
  }));

  return (
    <div>
      <button className="btn btn-back" onClick={() => navigate("/")}>
        ← Back to Dashboard
      </button>

      <div className="header">
        <div>
          <h1>
            {data.account.name}
            {data.current_balance != null && data.max_balance != null && data.current_balance === data.max_balance && (
              <span className="alltime-high" title="All-time high"> ⭐</span>
            )}
            {data.account.is_manual && (
              <span className="manual-badge">Manual</span>
            )}
          </h1>
          <p style={{ color: "var(--text-secondary)", fontSize: "0.9rem" }}>
            {data.account.institution}
            {data.account.type ? ` · ${data.account.type}` : ""}
          </p>
        </div>
      </div>

      {data.values.length > 0 ? (
        <div className="chart-container">
          <h3>Balance over time</h3>
          <ResponsiveContainer width="100%" height={300}>
            <LineChart data={chartData}>
              <CartesianGrid strokeDasharray="3 3" stroke="#e5e7eb" />
              <XAxis
                dataKey="label"
                tick={{ fontSize: 12 }}
                tickMargin={8}
              />
              <YAxis
                tick={{ fontSize: 12 }}
                tickFormatter={(v) => {
                  const abs = Math.abs(v);
                  const sign = v < 0 ? "-" : "";
                  return `${sign}$${(abs / 1000).toFixed(0)}k`;
                }}
                tickMargin={8}
              />
              <Tooltip
                formatter={(value) => {
                  const n = Number(value);
                  const abs = Math.abs(n);
                  const sign = n < 0 ? "-" : "";
                  return [
                    `${sign}$${abs.toLocaleString(undefined, {
                      minimumFractionDigits: 2,
                    })}`,
                    "Balance",
                  ];
                }}
              />
              <Line
                type="monotone"
                dataKey="balance"
                stroke="var(--accent)"
                strokeWidth={2}
                dot={{ r: 3, fill: "var(--accent)" }}
                activeDot={{ r: 5 }}
              />
            </LineChart>
          </ResponsiveContainer>
        </div>
      ) : (
        <div className="empty-state">
          <p>No data points yet for this account.</p>
        </div>
      )}

      <div className="detail-stats">
        <div className="detail-stat">
          <div className="stat-label">Current Value</div>
          <div className="stat-value stat-current">
            {formatCurrency(data.current_balance)}
          </div>
        </div>
        <div className="detail-stat">
          <div className="stat-label">Minimum</div>
          <div className="stat-value stat-min">
            {formatCurrency(data.min_balance)}
          </div>
        </div>
        <div className="detail-stat">
          <div className="stat-label">Maximum</div>
          <div className="stat-value stat-max">
            {formatCurrency(data.max_balance)}
          </div>
        </div>
      </div>

      {/* Historical changes */}
      {changes.length > 0 && (
        <div className="changes-section">
          <h3>Change over time</h3>
          <div className="changes-grid">
            {changes.map((ch) => {
              const label =
                ch.period === "30d"
                  ? "30 days"
                  : ch.period === "180d"
                  ? "180 days"
                  : "360 days";
              const isUp = ch.change_amount != null && ch.change_amount > 0;
              const isDown = ch.change_amount != null && ch.change_amount < 0;
              const cls = isUp ? "change-up" : isDown ? "change-down" : "";

              return (
                <div key={ch.period} className="change-item">
                  <div className="change-period">{label}</div>
                  {ch.change_amount != null ? (
                    <div className={`change-amount ${cls}`}>
                      {ch.change_amount > 0 ? "+" : ""}
                      {formatCurrency(ch.change_amount)}
                      {ch.change_percent != null && (
                        <span className="change-percent">
                          {" "}
                          ({ch.change_percent > 0 ? "+" : ""}
                          {ch.change_percent.toFixed(1)}%)
                        </span>
                      )}
                    </div>
                  ) : (
                    <div className="change-amount change-na">—</div>
                  )}
                  {ch.previous_date && (
                    <div className="change-date">
                      from {formatDate(ch.previous_date)}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* Manual account: add value form */}
      {data.account.is_manual && (
        <div className="card add-value-form">
          <div className="form-group">
            <label htmlFor="addValue">Add today's balance</label>
            <input
              id="addValue"
              type="number"
              step="0.01"
              placeholder="0.00"
              value={addValue}
              onChange={(e) => setAddValue(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleAddValue()}
            />
          </div>
          <button
            className="btn btn-primary"
            onClick={handleAddValue}
            disabled={adding || !addValue}
          >
            {adding ? "Saving..." : "Save"}
          </button>
          {addStatus && (
            <span
              style={{
                fontSize: "0.8rem",
                color:
                  addStatus.type === "success"
                    ? "var(--success)"
                    : "var(--danger)",
              }}
            >
              {addStatus.message}
            </span>
          )}
        </div>
      )}
    </div>
  );
}