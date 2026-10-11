import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import type { Account } from "../types";

export default function RetirementToggle({ item, onChanged }: {
  item: { account: Account; is_retired: boolean; current_balance: number | null };
  onChanged: () => Promise<void>;
}) {
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return (
    <div className="retirement-control">
      <label className="retirement-label">
        <span className="toggle">
          <input type="checkbox" role="switch" checked={item.is_retired} disabled={saving}
            aria-label={`Retired: ${item.account.name}`}
            onChange={async (event) => {
              const retired = event.target.checked;
              setSaving(true);
              setError(null);
              try {
                if (retired && item.current_balance != null && item.current_balance !== 0) {
                  const balance = new Intl.NumberFormat("en-NZ", {
                    style: "currency",
                    currency: item.account.currency,
                  }).format(item.current_balance);
                  const confirmed = await confirm(
                    `${item.account.name} still has a balance of ${balance}. This balance will still count toward your overall account value after you retire the account.`,
                    {
                      title: "Retire account with a remaining balance?",
                      kind: "warning",
                      okLabel: "Retire account",
                      cancelLabel: "Cancel",
                    },
                  );
                  if (!confirmed) return;
                }
                await invoke("set_account_retired", { accountId: item.account.id, retired });
                await onChanged();
              } catch (err) {
                setError(`Could not update retirement: ${err}`);
              } finally {
                setSaving(false);
              }
            }} />
          <span className="slider" />
        </span>
        {saving ? "Saving…" : "Retired"}
      </label>
      {error && <div className="status status-error" role="alert">{error}</div>}
    </div>
  );
}
