import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";

const IDLE_DELAY = 1500;

/** Keep the current screen mounted; apply background changes only between interactions. */
export function useLiveData<T>(load: () => Promise<T>, paused = false) {
  const [data, setData] = useState<T | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const pauseRef = useRef(paused);
  pauseRef.current = paused;
  const generation = useRef(0);
  const lastInteraction = useRef(0);

  const isBusy = useCallback(() =>
    pauseRef.current || document.visibilityState === "hidden" ||
    Date.now() - lastInteraction.current < IDLE_DELAY ||
    !!document.activeElement?.matches("input, textarea, select, [contenteditable='true']"), []);

  const update = useCallback(async (background: boolean) => {
    const request = ++generation.current;
    try {
      const result = await load();
      if (request !== generation.current) return true;
      // Interaction may have started while the read was in flight. Read again
      // when idle rather than applying an old result after a subsequent save.
      if (background && isBusy()) return false;
      setData((previous) => JSON.stringify(previous) === JSON.stringify(result) ? previous : result);
      setError(null);
    } catch (err) {
      if (request !== generation.current) return true;
      if (background && isBusy()) return false;
      setError(String(err));
    } finally {
      if (request === generation.current) setLoading(false);
    }
    return true;
  }, [load, isBusy]);

  // Explicit saves refresh immediately and supersede any older background read.
  const refresh = useCallback(async () => { await update(false); }, [update]);

  useEffect(() => {
    setData(null);
    setError(null);
    setLoading(true);
    void refresh();
    return () => { generation.current++; };
  }, [refresh]);

  useEffect(() => {
    let disposed = false;
    let pending = false;
    let running = false;
    let timer: number | undefined;
    let unlisten: (() => void) | undefined;

    const schedule = () => {
      window.clearTimeout(timer);
      if (!disposed) timer = window.setTimeout(flush, 500);
    };
    const flush = async () => {
      if (disposed || !pending || running) return;
      if (isBusy()) { schedule(); return; }
      pending = false;
      running = true;
      const applied = await update(true);
      running = false;
      pending ||= !applied;
      if (pending) schedule();
    };
    const interacted = () => { lastInteraction.current = Date.now(); };
    // Capture scrolls within nested containers as well as the document.
    const events = ["scroll", "wheel", "pointerdown", "pointermove", "keydown", "input", "focusin", "focusout", "touchmove"];
    events.forEach((name) => document.addEventListener(name, interacted, { capture: true, passive: true }));
    listen("db-changed", () => {
      if (disposed) return;
      pending = true;
      schedule();
    }).then((stop) => {
      if (disposed) stop(); else unlisten = stop;
    }).catch((err) => {
      if (!disposed) setError(`Automatic updates unavailable: ${err}`);
    });

    return () => {
      disposed = true;
      window.clearTimeout(timer);
      unlisten?.();
      events.forEach((name) => document.removeEventListener(name, interacted, true));
    };
  }, [update, isBusy]);

  return { data, loading, error, refresh };
}
