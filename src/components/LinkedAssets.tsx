import { useEffect, useState } from "react";
import { Link } from "react-router";
import { invoke } from "@tauri-apps/api/core";
import type { AssetLinks } from "../types";

const money = (value: number | null) => value == null ? "No recorded value" :
  new Intl.NumberFormat("en-NZ", { style: "currency", currency: "NZD" }).format(value);

export default function LinkedAssets({ assetId, links, readOnly, onChanged, onEditingChange }: {
  assetId: string; links: AssetLinks; readOnly: boolean;
  onChanged: () => Promise<void>; onEditingChange: (editing: boolean) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => { onEditingChange(editing || saving); }, [editing, saving, onEditingChange]);
  const linked = links.linked_assets;
  const options = [...linked, ...links.available_assets];
  async function save() {
    setSaving(true); setError(null);
    try {
      await invoke("set_linked_assets", { assetId, linkedAssetIds: selected });
      setEditing(false);
      await onChanged();
    } catch (err) { setError(String(err)); }
    finally { setSaving(false); }
  }
  if (links.primary_asset) return <section className="card linked-assets">
    <h3>Linked assets</h3>
    <p>This account is included in <Link to={`/account/${encodeURIComponent(links.primary_asset.id)}`}>{links.primary_asset.name}</Link>’s combined view. Open that asset to view the combined history or edit the links.</p>
  </section>;
  if (readOnly && !linked.length) return null;
  return <section className="card linked-assets" aria-labelledby="linked-assets-title">
    <div className="repayment-details-header">
      <h3 id="linked-assets-title">Linked assets</h3>
      {!readOnly && !editing && <button className="btn btn-secondary" onClick={() => {
        setSelected(linked.map(item => item.id)); setEditing(true); setError(null);
      }}>{linked.length ? "Edit linked assets" : "Link assets"}</button>}
    </div>
    <p className="calculation-note">Link previous provider accounts to continue the balance history here. Each account can belong to one primary asset. Overall totals still count each account once.</p>
    {editing ? <>
      <fieldset className="liability-options" disabled={saving}>
        <legend>Assets to include</legend>
        {options.map(item => <label className="liability-link-row" key={item.id}>
          <input type="checkbox" checked={selected.includes(item.id)} onChange={event => setSelected(previous => event.target.checked ? [...previous, item.id] : previous.filter(id => id !== item.id))} />
          <span className="liability-link-name"><strong>{item.name}</strong><small>{[item.recorded_date, item.is_retired && "Retired", item.is_ignored && "Ignored"].filter(Boolean).join(" · ")}</small></span>
          <span className="liability-link-balance">{money(item.balance)}</span>
        </label>)}
      </fieldset>
      {!options.length && <p>No available assets with recorded balances.</p>}
      <div className="asset-link-actions">
        <button className="btn btn-primary" disabled={saving} onClick={save}>{saving ? "Saving…" : "Save linked assets"}</button>
        <button className="btn btn-secondary" disabled={saving} onClick={() => { setEditing(false); setError(null); }}>Cancel</button>
      </div>
    </> : linked.length > 0 && <ul className="linked-liabilities-list">
      {linked.map(item => <li className="liability-link-row" key={item.id}>
        <span className="liability-link-name"><Link to={`/account/${encodeURIComponent(item.id)}`}>{item.name}</Link><small>{[item.recorded_date, item.is_retired && "Retired", item.is_ignored && "Ignored"].filter(Boolean).join(" · ")}</small></span>
        <span className="liability-link-balance">{money(item.balance)}</span>
      </li>)}
    </ul>}
    {error && <p className="status-msg error" role="alert">{error}</p>}
  </section>;
}
