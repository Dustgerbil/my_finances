import { useState, useEffect, useCallback } from "react";
import { useNavigate, useParams } from "react-router";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  LineChart,
  Line,
  XAxis,
  YAxis,
  CartesianGrid,
  ResponsiveContainer,
  type DotItemDotProps,
} from "recharts";
import type { AccountHistory, AccountChange, DeletedValue } from "../types";
import { formatAccountType } from "../format";
import BalanceDirection from "../components/BalanceDirection";

type ChartRange = "1m" | "6m" | "1y" | "all";

const chartRanges: { value: ChartRange; label: string; months: number | null }[] = [
  { value: "1m", label: "1M", months: 1 },
  { value: "6m", label: "6M", months: 6 },
  { value: "1y", label: "1Y", months: 12 },
  { value: "all", label: "All", months: null },
];

function subtractMonths(dateString: string, months: number) {
  const date = new Date(`${dateString}T00:00:00`);
  const originalDay = date.getDate();
  date.setDate(1);
  date.setMonth(date.getMonth() - months);
  const lastDay = new Date(date.getFullYear(), date.getMonth() + 1, 0).getDate();
  date.setDate(Math.min(originalDay, lastDay));
  return [
    date.getFullYear(),
    String(date.getMonth() + 1).padStart(2, "0"),
    String(date.getDate()).padStart(2, "0"),
  ].join("-");
}

export default function AccountDetail() {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const [data, setData] = useState<AccountHistory | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [changes, setChanges] = useState<AccountChange[]>([]);
  const [chartRange, setChartRange] = useState<ChartRange>("1y");
  const [hoveredChartIndex, setHoveredChartIndex] = useState<number | null>(null);

  // Optional repayment details for accounts whose current balance is negative.
  const [interestRate, setInterestRate] = useState("");
  const [repaymentAmount, setRepaymentAmount] = useState("");
  const [repaymentFrequency, setRepaymentFrequency] = useState("");
  const [repaymentFrequencyUnit, setRepaymentFrequencyUnit] = useState<
    "days" | "weeks" | "months"
  >("months");
  const [rateExpiry, setRateExpiry] = useState("");
  const [initialNextRepaymentDate, setInitialNextRepaymentDate] = useState("");
  const [editingRepayment, setEditingRepayment] = useState(false);
  const [savingRepayment, setSavingRepayment] = useState(false);
  const [repaymentStatus, setRepaymentStatus] = useState<{
    type: "success" | "error";
    message: string;
  } | null>(null);

  // Add value form state
  const [addValue, setAddValue] = useState("");
  const [adding, setAdding] = useState(false);
  const [addStatus, setAddStatus] = useState<{
    type: "success" | "error";
    message: string;
  } | null>(null);

  // Delete-last-value state: two-click confirm to avoid accidental deletes.
  const [deleting, setDeleting] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);

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

  const resetRepaymentDraft = useCallback(() => {
    if (!data) return;
    setInterestRate(data.account.interest_rate?.toString() ?? "");
    setRepaymentAmount(data.account.repayment_amount?.toString() ?? "");
    setRepaymentFrequency(data.account.repayment_frequency?.toString() ?? "");
    setRepaymentFrequencyUnit(data.account.repayment_frequency_unit ?? "months");
    setRateExpiry(data.account.rate_expiry ?? "");
    setInitialNextRepaymentDate(
      data.next_repayment_date ?? data.account.initial_next_repayment_date ?? "",
    );
  }, [data]);

  useEffect(() => {
    resetRepaymentDraft();
  }, [resetRepaymentDraft]);

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

  const handleDeleteLastValue = async () => {
    if (!id) return;
    // Second click within the confirm window actually deletes.
    if (!confirmingDelete) {
      setConfirmingDelete(true);
      // Auto-reset the confirm state after a few seconds.
      setTimeout(() => setConfirmingDelete(false), 4000);
      return;
    }
    setConfirmingDelete(false);
    setDeleting(true);
    setAddStatus(null);
    try {
      const deleted = await invoke<DeletedValue | null>(
        "delete_last_manual_value",
        { accountId: decodeURIComponent(id) },
      );
      if (deleted) {
        setAddStatus({
          type: "success",
          message: `Deleted ${formatCurrency(deleted.balance)} from ${formatDate(
            deleted.recorded_date,
          )}`,
        });
      } else {
        setAddStatus({ type: "error", message: "No values to delete" });
      }
      await loadHistory();
    } catch (err) {
      setAddStatus({ type: "error", message: `Error: ${err}` });
    } finally {
      setDeleting(false);
    }
  };

  const handleSaveRepayment = async () => {
    if (!id) return;
    const parseOptionalNumber = (value: string, label: string) => {
      if (!value.trim()) return null;
      const number = Number(value);
      if (!Number.isFinite(number)) throw new Error(`${label} must be a valid number`);
      return number;
    };

    setSavingRepayment(true);
    setRepaymentStatus(null);
    try {
      const frequency = parseOptionalNumber(repaymentFrequency, "Frequency");
      if (frequency != null && !Number.isInteger(frequency)) {
        throw new Error("Frequency must be a whole number");
      }
      const displayedNextRepaymentDate =
        data?.next_repayment_date ?? data?.account.initial_next_repayment_date ?? "";
      const repaymentDateAnchor =
        initialNextRepaymentDate === displayedNextRepaymentDate
          ? data?.account.initial_next_repayment_date ?? null
          : initialNextRepaymentDate || null;
      await invoke("update_liability_repayment", {
        request: {
          account_id: decodeURIComponent(id),
          interest_rate: parseOptionalNumber(interestRate, "Interest rate"),
          repayment_amount: parseOptionalNumber(repaymentAmount, "Repayment amount"),
          repayment_frequency: frequency,
          repayment_frequency_unit: frequency == null ? null : repaymentFrequencyUnit,
          rate_expiry: rateExpiry || null,
          initial_next_repayment_date: repaymentDateAnchor,
        },
      });
      setRepaymentStatus({ type: "success", message: "Repayment details saved" });
      setEditingRepayment(false);
      await loadHistory();
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setRepaymentStatus({ type: "error", message: `Error: ${message}` });
    } finally {
      setSavingRepayment(false);
    }
  };

  const handleCancelRepaymentEdit = () => {
    resetRepaymentDraft();
    setRepaymentStatus(null);
    setEditingRepayment(false);
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

  const latestRecordedDate = data.values.reduce(
    (latest, value) => value.recorded_date > latest ? value.recorded_date : latest,
    "",
  );
  const selectedRange = chartRanges.find((range) => range.value === chartRange)!;
  const cutoffDate = selectedRange.months != null && latestRecordedDate
    ? subtractMonths(latestRecordedDate, selectedRange.months)
    : null;
  const chartData = data.values
    .filter((value) => cutoffDate == null || value.recorded_date >= cutoffDate)
    .map((v) => ({
      date: v.recorded_date,
      balance: v.balance,
      label: formatDate(v.recorded_date),
    }));
  const hoveredChartPoint = hoveredChartIndex != null
    ? chartData[hoveredChartIndex] ?? null
    : null;

  return (
    <div>
      <button className="btn btn-back" onClick={() => navigate("/")}>
        ← Back to Dashboard
      </button>

      <div className="header">
        <div>
          <h1>
            {data.account.name}
            {data.account.is_manual && (
              <span className="manual-badge">Manual</span>
            )}
          </h1>
          <p style={{ color: "var(--text-secondary)", fontSize: "0.9rem" }}>
            {data.account.institution}
            {data.account.type ? ` · ${formatAccountType(data.account.type)}` : ""}
          </p>
        </div>
      </div>

      {data.values.length > 0 ? (
        <div className="chart-container">
          <div className="chart-header">
            <h3>Balance over time</h3>
            <div className="chart-range" aria-label="Chart date range">
              {chartRanges.map((range) => (
                <button
                  key={range.value}
                  type="button"
                  className={chartRange === range.value ? "active" : ""}
                  aria-pressed={chartRange === range.value}
                  onClick={() => setChartRange(range.value)}
                >
                  {range.label}
                </button>
              ))}
            </div>
          </div>
          {hoveredChartPoint && (
            <div className="chart-hover-tooltip" role="status">
              <strong>{formatCurrency(hoveredChartPoint.balance)}</strong>
              <span>{hoveredChartPoint.label}</span>
            </div>
          )}
          <ResponsiveContainer width="100%" height={300}>
            <LineChart
              data={chartData}
              onMouseLeave={() => setHoveredChartIndex(null)}
            >
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
              <Line
                type="monotone"
                dataKey="balance"
                stroke="var(--accent)"
                strokeWidth={2}
                dot={(props: DotItemDotProps) => (
                  <g
                    onMouseEnter={() => setHoveredChartIndex(props.index)}
                    onMouseLeave={() => setHoveredChartIndex(null)}
                  >
                    <circle
                      cx={props.cx}
                      cy={props.cy}
                      r={10}
                      fill="transparent"
                    />
                    <circle
                      cx={props.cx}
                      cy={props.cy}
                      r={3}
                      fill="var(--accent)"
                    />
                  </g>
                )}
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
          <div className="stat-value stat-current balance-with-direction">
            <span>{formatCurrency(data.current_balance)}</span>
            <BalanceDirection
              current={data.current_balance}
              previous={data.previous_balance}
            />
            {data.current_balance != null &&
              data.max_balance != null &&
              data.current_balance === data.max_balance && (
                <span className="alltime-high" role="img" aria-label="All-time high" title="All-time high">
                  ⭐
                </span>
              )}
          </div>
        </div>
        <div className="detail-stat">
          <div className="stat-label">Minimum</div>
          <div className="stat-value stat-min">
            {formatCurrency(data.min_balance)}
          </div>
          {data.min_balance_date && (
            <div className="stat-date">{formatDate(data.min_balance_date)}</div>
          )}
        </div>
        <div className="detail-stat">
          <div className="stat-label">Maximum</div>
          <div className="stat-value stat-max">
            {formatCurrency(data.max_balance)}
          </div>
          {data.max_balance_date && (
            <div className="stat-date">{formatDate(data.max_balance_date)}</div>
          )}
        </div>
      </div>

      {data.current_balance != null &&
        data.current_balance < 0 &&
        (data.account.id === "__liabilities_total__" ||
          !data.account.id.startsWith("__")) && (
          <div className="card liability-calculations">
            <h3>
              {data.account.id === "__liabilities_total__"
                ? "Combined repayment calculations"
                : "Repayment calculations"}
            </h3>
            <div className="detail-stats">
              <div className="detail-stat">
                <div className="stat-label">Next repayment</div>
                <div className="stat-value">
                  {data.next_repayment_date
                    ? formatDate(data.next_repayment_date)
                    : "—"}
                </div>
              </div>
              <div className="detail-stat">
                <div className="stat-label">Estimated repayment date</div>
                <div className="stat-value">
                  {data.estimated_repayment_date
                    ? formatDate(data.estimated_repayment_date)
                    : "—"}
                </div>
                {data.repayment_estimate_status && (
                  <div className="stat-date">{data.repayment_estimate_status}</div>
                )}
              </div>
            </div>
            <p className="calculation-note">
              {data.account.id === "__liabilities_total__"
                ? "Derived from the included liability accounts. The combined payoff date is the latest individual estimate; each estimate assumes its current rate continues indefinitely and ignores rate expiry."
                : "The estimate assumes the current interest rate continues indefinitely; the rate expiry is not used in this calculation."}
            </p>
          </div>
      )}

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
                  : ch.period === "360d"
                  ? "360 days"
                  : "All time";
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

      {data.current_balance != null &&
        data.current_balance < 0 &&
        !data.account.id.startsWith("__") && (
          <div className="card liability-repayment-form">
            <div className="repayment-details-header">
              <h3>Repayment details</h3>
              {!editingRepayment && (
                <button
                  type="button"
                  className="btn"
                  onClick={() => {
                    setRepaymentStatus(null);
                    setEditingRepayment(true);
                  }}
                >
                  Edit
                </button>
              )}
            </div>

            {editingRepayment ? (
              <>
                <p>All fields are optional. Complete all except rate expiry to calculate an estimate.</p>
                <div className="repayment-form-grid">
                  <div className="form-group">
                    <label htmlFor="interestRate">Annual interest rate (%)</label>
                    <input
                      id="interestRate"
                      type="number"
                      min="0"
                      step="0.01"
                      placeholder="e.g. 6.49"
                      value={interestRate}
                      onChange={(e) => setInterestRate(e.target.value)}
                    />
                  </div>
                  <div className="form-group">
                    <label htmlFor="repaymentAmount">Repayment amount</label>
                    <input
                      id="repaymentAmount"
                      type="number"
                      min="0"
                      step="0.01"
                      placeholder="0.00"
                      value={repaymentAmount}
                      onChange={(e) => setRepaymentAmount(e.target.value)}
                    />
                  </div>
                  <div className="form-group">
                    <label htmlFor="repaymentFrequency">Repayment frequency</label>
                    <div className="frequency-inputs">
                      <input
                        id="repaymentFrequency"
                        type="number"
                        min="1"
                        step="1"
                        placeholder="e.g. 2"
                        value={repaymentFrequency}
                        onChange={(e) => setRepaymentFrequency(e.target.value)}
                      />
                      <select
                        aria-label="Repayment frequency unit"
                        value={repaymentFrequencyUnit}
                        onChange={(e) =>
                          setRepaymentFrequencyUnit(
                            e.target.value as "days" | "weeks" | "months",
                          )
                        }
                      >
                        <option value="days">days</option>
                        <option value="weeks">weeks</option>
                        <option value="months">months</option>
                      </select>
                    </div>
                  </div>
                  <div className="form-group">
                    <label htmlFor="initialNextRepaymentDate">Next payment date</label>
                    <input
                      id="initialNextRepaymentDate"
                      type="date"
                      value={initialNextRepaymentDate}
                      onChange={(e) => setInitialNextRepaymentDate(e.target.value)}
                    />
                  </div>
                  <div className="form-group">
                    <label htmlFor="rateExpiry">Rate expiry</label>
                    <input
                      id="rateExpiry"
                      type="date"
                      value={rateExpiry}
                      onChange={(e) => setRateExpiry(e.target.value)}
                    />
                  </div>
                </div>
                <p className="calculation-note">
                  The initial date anchors the schedule. The displayed next repayment
                  automatically advances by the configured frequency as time passes.
                </p>
                <div className="repayment-actions">
                  <button
                    className="btn btn-primary"
                    onClick={handleSaveRepayment}
                    disabled={savingRepayment}
                  >
                    {savingRepayment ? "Saving..." : "Save repayment details"}
                  </button>
                  <button
                    type="button"
                    className="btn"
                    onClick={handleCancelRepaymentEdit}
                    disabled={savingRepayment}
                  >
                    Cancel
                  </button>
                  {repaymentStatus && (
                    <span className={`inline-status ${repaymentStatus.type}`}>
                      {repaymentStatus.message}
                    </span>
                  )}
                </div>
              </>
            ) : (
              <>
                <div className="repayment-readonly-grid">
                  <div>
                    <span>Annual interest rate</span>
                    <strong>
                      {data.account.interest_rate != null
                        ? `${data.account.interest_rate}%`
                        : "—"}
                    </strong>
                  </div>
                  <div>
                    <span>Repayment amount</span>
                    <strong>{formatCurrency(data.account.repayment_amount)}</strong>
                  </div>
                  <div>
                    <span>Repayment frequency</span>
                    <strong>
                      {data.account.repayment_frequency != null &&
                      data.account.repayment_frequency_unit
                        ? `${data.account.repayment_frequency} ${data.account.repayment_frequency_unit}`
                        : "—"}
                    </strong>
                  </div>
                  <div>
                    <span>Next payment date</span>
                    <strong>
                      {data.next_repayment_date
                        ? formatDate(data.next_repayment_date)
                        : "—"}
                    </strong>
                  </div>
                  <div>
                    <span>Rate expiry</span>
                    <strong>
                      {data.account.rate_expiry
                        ? formatDate(data.account.rate_expiry)
                        : "—"}
                    </strong>
                  </div>
                </div>
                {repaymentStatus && (
                  <span className={`inline-status ${repaymentStatus.type}`}>
                    {repaymentStatus.message}
                  </span>
                )}
              </>
            )}
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
          {data.values.length > 0 && (
            <button
              className="btn"
              onClick={handleDeleteLastValue}
              disabled={deleting || adding}
              style={{
                color: confirmingDelete ? "var(--danger)" : undefined,
                borderColor: confirmingDelete ? "var(--danger)" : undefined,
              }}
            >
              {deleting
                ? "Deleting..."
                : confirmingDelete
                ? "Click again to confirm"
                : "Delete last value"}
            </button>
          )}
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
