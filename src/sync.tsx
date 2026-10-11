import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";

interface SyncStatus {
  read_only: boolean;
  has_cache: boolean;
  message: string;
  error: string | null;
  last_sync_at: string | null;
  source_updated_at: string | null;
  published_at: string | null;
}

const SyncStatusContext = createContext<SyncStatus | null>(null);
const ReadOnlyContext = createContext(false);
export const useReadOnly = () => useContext(ReadOnlyContext);
const dateLabel = (value: string | null) => value ? new Date(value).toLocaleString() : "Not yet";

export function SyncProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [startupError, setStartupError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const running = useRef(false);
  const refresh = useCallback(async (connect = false) => {
    if (running.current) return;
    running.current = true;
    setBusy(true);
    try {
      setStatus(await invoke<SyncStatus>("sync_mobile_snapshot", { connect }));
    } catch (error) {
      setStatus((previous) => previous ? { ...previous, error: String(error) } : previous);
    } finally {
      running.current = false;
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    invoke<SyncStatus>("get_sync_status").then((value) => {
      if (!disposed) { setStatus(value); setStartupError(null); }
    }).catch(() => { if (!disposed) setStartupError("Could not initialise My Finances. Please close and reopen the app."); });
    return () => { disposed = true; };
  }, []);

  const readOnly = status?.read_only;
  useEffect(() => {
    if (readOnly === undefined) return;
    const check = () => {
      if (document.visibilityState === "hidden") return;
      if (readOnly) void refresh();
      else void invoke<SyncStatus>("get_sync_status").then(setStatus).catch(() => {});
    };
    check();
    const timer = window.setInterval(check, readOnly ? 30_000 : 5_000);
    window.addEventListener("focus", check);
    document.addEventListener("visibilitychange", check);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", check);
      document.removeEventListener("visibilitychange", check);
    };
  }, [readOnly, refresh]);

  if (!status) return <div className="empty-state" role="status">{startupError ?? "Opening My Finances…"}</div>;
  return (
    <SyncStatusContext.Provider value={status}>
      <ReadOnlyContext.Provider value={status.read_only}>
        {status.read_only ? (
          <section className="sync-panel" aria-label="iCloud sync">
            <strong>{busy ? "Checking iCloud Drive…" : status.message}</strong>
            {status.error && <p role="status">{status.error}</p>}
            {status.has_cache && <div className="sync-times">
              <p>Last bank refresh: {dateLabel(status.source_updated_at)}</p>
              <p>Mac snapshot: {dateLabel(status.published_at)}</p>
              <p>Last synced: {dateLabel(status.last_sync_at)}</p>
            </div>}
            <div className="sync-actions">
              <button className="btn btn-primary" disabled={busy} onClick={() => void refresh(true)}>
                {status.has_cache ? "Reconnect folder" : "Connect iCloud folder"}
              </button>
              <button className="btn btn-secondary" disabled={busy} onClick={() => void refresh()}>Check for updates</button>
            </div>
            {!status.has_cache && <p>Select <strong>iCloud Drive → my_finances → mobile</strong> once. Updates then arrive automatically while the app is open, and when you return to it.</p>}
          </section>
        ) : null}
        {(!status.read_only || status.has_cache) && children}
      </ReadOnlyContext.Provider>
    </SyncStatusContext.Provider>
  );
}

export function IPhoneSyncSettings() {
  const status = useContext(SyncStatusContext);
  if (!status || status.read_only) return null;
  return (
    <details className="sync-desktop">
      <summary>iPhone sync{status.error ? " — needs attention" : ""}</summary>
      <p>{status.error ?? (status.message || "Preparing the iPhone snapshot…")}</p>
      {status.published_at && <p>Last published: {dateLabel(status.published_at)}</p>}
      <p>On your iPhone, connect iCloud Drive → my_finances → mobile.</p>
    </details>
  );
}
