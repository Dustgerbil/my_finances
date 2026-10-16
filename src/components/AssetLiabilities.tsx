import { useEffect, useState } from "react";
import { Link } from "react-router";
import { invoke } from "@tauri-apps/api/core";
import type { AssetFinancing, FinancingLiability } from "../types";

const money = (value: number | null) => value == null ? "—" :
  new Intl.NumberFormat("en-NZ", { style: "currency", currency: "NZD" }).format(value);

function liabilityDetails(item: FinancingLiability) {
  const date = item.recorded_date
    ? new Date(`${item.recorded_date}T00:00:00`).toLocaleDateString("en-NZ")
    : "No recorded value";
  return [date, item.is_retired ? "Retired" : "", item.is_ignored ? "Ignored" : ""]
    .filter(Boolean).join(" · ");
}

export default function AssetLiabilities({ assetId, financing, summary, readOnly, onChanged, onEditingChange }: {
  assetId: string;
  financing: AssetFinancing;
  summary?: AssetFinancing | null;
  readOnly: boolean;
  onChanged: () => Promise<void>;
  onEditingChange: (editing: boolean) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => { onEditingChange(editing || saving); }, [editing, saving, onEditingChange]);
  const linked = financing.linked_liabilities;
  const figures = summary ?? financing;
  const displayedLinks = figures.linked_liabilities;
  const options = [...linked, ...financing.available_liabilities];

  async function save() {
    setSaving(true);
    setError(null);
    try {
      await invoke("set_asset_liabilities", { assetId, liabilityIds: selected });
      setEditing(false);
      await onChanged();
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  if (readOnly && displayedLinks.length === 0) return null;

  return (
    <section className="card asset-liabilities" aria-labelledby="asset-liabilities-title">
      <div className="repayment-details-header">
        <h3 id="asset-liabilities-title">Linked liabilities</h3>
        {!readOnly && !editing && (
          <button className="btn btn-secondary" onClick={() => {
            setSelected(linked.map((item) => item.id));
            setError(null);
            setEditing(true);
          }}>{linked.length ? "Edit" : "Add liabilities"}</button>
        )}
      </div>

      {displayedLinks.length > 0 && (
        <>
          <div className="detail-stats equity-stats">
            <div className="detail-stat">
              <div className="stat-label">Linked debt</div>
              <div className="stat-value">{money(figures.total_debt)}</div>
            </div>
            <div className="detail-stat">
              <div className="stat-label">Equity</div>
              <div className={`stat-value ${(figures.equity_amount ?? 0) < 0 ? "stat-min" : "stat-current"}`}>
                {money(figures.equity_amount)}
              </div>
            </div>
            <div className="detail-stat">
              <div className="stat-label">Equity percentage</div>
              <div className="stat-value">{figures.equity_percent == null ? "—" : `${figures.equity_percent.toFixed(1)}%`}</div>
            </div>
          </div>
          <p className="calculation-note">
            {summary ? "Combined equity uses all linked assets and their debts. Edit each asset to change the liabilities attached to it." : "Equity is this asset’s latest recorded value minus its linked debts."} Credit balances count as zero debt.
            {figures.equity_amount == null && " A recorded value is missing; equity is unavailable."}
            {figures.equity_amount != null && figures.equity_percent == null && " Equity percentage requires an asset value greater than zero."}
          </p>
        </>
      )}

      {editing ? (
        <>
          <p className="calculation-note">Select the liabilities that funded this asset only. Each liability can be linked to one asset; unlink it there before moving it here. Changes apply when you save.</p>
          <fieldset className="liability-options" disabled={saving}>
            <legend>Liabilities that funded this asset</legend>
            {options.map((item) => (
              <label className="liability-link-row" key={item.id}>
                <input type="checkbox" checked={selected.includes(item.id)} onChange={(event) => {
                  setSelected((previous) => event.target.checked
                    ? [...previous, item.id] : previous.filter((id) => id !== item.id));
                }} />
                <span className="liability-link-name"><strong>{item.name}</strong><small>{liabilityDetails(item)}</small></span>
                <span className="liability-link-balance">{money(item.balance)}</span>
              </label>
            ))}
          </fieldset>
          {options.length === 0 && <p className="calculation-note">No unlinked liabilities are available. Add a liability account or record its negative balance first.</p>}
          <div className="asset-link-actions">
            <button className="btn btn-primary" disabled={saving} onClick={save}>{saving ? "Saving…" : "Save"}</button>
            <button className="btn btn-secondary" disabled={saving} onClick={() => { setEditing(false); setError(null); }}>Cancel</button>
          </div>
        </>
      ) : displayedLinks.length ? (
        <ul className="linked-liabilities-list">
          {displayedLinks.map((item) => (
            <li className="liability-link-row" key={item.id}>
              <span className="liability-link-name">
                <Link to={`/account/${encodeURIComponent(item.id)}`}>{item.name}</Link>
                <small>{liabilityDetails(item)}</small>
              </span>
              <span className="liability-link-balance">{money(item.balance)}</span>
            </li>
          ))}
        </ul>
      ) : <p className="calculation-note">Link mortgages or other debts used to fund this asset to see its equity.</p>}
      {error && <p className="status-msg error" role="alert">{error}</p>}
    </section>
  );
}
