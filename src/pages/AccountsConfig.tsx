import { useState, useEffect, useCallback } from "react";
import { useNavigate } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import type { Account } from "../types";

export default function AccountsConfig() {
  const navigate = useNavigate();
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [loading, setLoading] = useState(true);

  // Add manual account form
  const [showAddForm, setShowAddForm] = useState(false);
  const [newName, setNewName] = useState("");
  const [newType, setNewType] = useState("");
  const [newInstitution, setNewInstitution] = useState("");
  const [adding, setAdding] = useState(false);
  const [addStatus, setAddStatus] = useState<{
    type: "success" | "error";
    message: string;
  } | null>(null);

  const loadAccounts = useCallback(async () => {
    setLoading(true);
    try {
      const data = await invoke<Account[]>("get_all_accounts_config");
      setAccounts(data);
    } catch (err) {
      console.error("Failed to load accounts:", err);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadAccounts();
  }, [loadAccounts]);

  const handleToggle = async (accountId: string) => {
    try {
      await invoke<boolean>("toggle_ignore_account", { accountId });
      await loadAccounts();
    } catch (err) {
      console.error("Failed to toggle account:", err);
    }
  };

  const handleAddManual = async () => {
    if (!newName.trim()) {
      setAddStatus({ type: "error", message: "Name is required" });
      return;
    }

    setAdding(true);
    setAddStatus(null);
    try {
      await invoke("add_manual_account", {
        request: {
          name: newName.trim(),
          account_type: newType.trim() || "Manual",
          institution: newInstitution.trim() || "Manual Entry",
        },
      });
      setNewName("");
      setNewType("");
      setNewInstitution("");
      setShowAddForm(false);
      setAddStatus({ type: "success", message: "Account added" });
      await loadAccounts();
    } catch (err) {
      setAddStatus({ type: "error", message: `Error: ${err}` });
    } finally {
      setAdding(false);
    }
  };

  const linkedAccounts = accounts.filter((a) => !a.is_manual);
  const manualAccounts = accounts.filter((a) => a.is_manual);

  return (
    <div>
      <button className="btn btn-back" onClick={() => navigate("/")}>
        ← Back to Dashboard
      </button>

      <div className="config-header">
        <h1>Accounts Configuration</h1>
        <button
          className="btn btn-primary"
          onClick={() => setShowAddForm(!showAddForm)}
        >
          {showAddForm ? "Cancel" : "+ Add Manual Account"}
        </button>
      </div>

      {addStatus && (
        <div className={`status status-${addStatus.type}`}>
          {addStatus.message}
        </div>
      )}

      {/* Add manual account form */}
      {showAddForm && (
        <div className="card add-manual-form">
          <h3>New Manual Account</h3>
          <div className="form-group">
            <label>Account Name *</label>
            <input
              type="text"
              placeholder="e.g. KiwiSaver, Term Deposit"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleAddManual()}
            />
          </div>
          <div className="form-group">
            <label>Type</label>
            <input
              type="text"
              placeholder="e.g. Savings, Investment"
              value={newType}
              onChange={(e) => setNewType(e.target.value)}
            />
          </div>
          <div className="form-group">
            <label>Institution</label>
            <input
              type="text"
              placeholder="e.g. My Bank"
              value={newInstitution}
              onChange={(e) => setNewInstitution(e.target.value)}
            />
          </div>
          <button
            className="btn btn-primary"
            onClick={handleAddManual}
            disabled={adding || !newName.trim()}
          >
            {adding ? "Adding..." : "Add Account"}
          </button>
        </div>
      )}

      {loading ? (
        <div className="empty-state"><p>Loading...</p></div>
      ) : (
        <>
          {/* Linked (Akahu) accounts */}
          {linkedAccounts.length > 0 && (
            <>
              <h2 className="section-title">Linked Accounts</h2>
              <div className="config-list">
                {linkedAccounts.map((acc) => (
                  <div key={acc.id} className="card config-item">
                    <div className="config-item-info">
                      <div className="account-name">{acc.name}</div>
                      <div className="account-institution">
                        {acc.institution}
                        {acc.type ? ` · ${acc.type}` : ""}
                      </div>
                    </div>
                    <label className="toggle">
                      <input
                        type="checkbox"
                        checked={!acc.is_ignored}
                        onChange={() => handleToggle(acc.id)}
                      />
                      <span className="slider" />
                    </label>
                  </div>
                ))}
              </div>
            </>
          )}

          {/* Manual accounts */}
          {manualAccounts.length > 0 && (
            <>
              <h2 className="section-title">Manual Accounts</h2>
              <div className="config-list">
                {manualAccounts.map((acc) => (
                  <div key={acc.id} className="card config-item">
                    <div className="config-item-info">
                      <div className="account-name">{acc.name}</div>
                      <div className="account-institution">
                        {acc.institution}
                        {acc.type ? ` · ${acc.type}` : ""}
                      </div>
                    </div>
                    <label className="toggle">
                      <input
                        type="checkbox"
                        checked={!acc.is_ignored}
                        onChange={() => handleToggle(acc.id)}
                      />
                      <span className="slider" />
                    </label>
                  </div>
                ))}
              </div>
            </>
          )}

          {accounts.length === 0 && (
            <div className="empty-state">
              <p>No accounts yet. Fetch from Akahu on the dashboard or add a manual account.</p>
            </div>
          )}
        </>
      )}
    </div>
  );
}