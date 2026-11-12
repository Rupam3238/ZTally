import { useEffect, useState } from "react";
import { api, type SwitchHistoryEntry } from "../lib/ipc";
import { useT } from "../lib/i18n";

export function SwitchHistoryModal({ onClose }: { onClose: () => void }) {
  const t = useT();
  const [entries, setEntries] = useState<SwitchHistoryEntry[] | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let alive = true;
    api.switchHistory().then((history) => {
      if (alive) setEntries(history);
    }).catch(() => {
      if (alive) setFailed(true);
    });
    return () => {
      alive = false;
    };
  }, []);

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div
        className="modal switch-history-modal"
        role="dialog"
        aria-modal="true"
        aria-label={t("switchHistory.title")}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 12 }}>
          <h2 style={{ margin: 0 }}>{t("switchHistory.title")}</h2>
          <button className="btn ghost" onClick={onClose}>{t("switchHistory.close")}</button>
        </div>
        <div className="switch-history-list">
          {failed ? (
            <div style={{ padding: "14px 0", color: "var(--muted)" }}>{t("switchHistory.error")}</div>
          ) : entries === null ? (
            <div style={{ padding: "14px 0", color: "var(--muted)" }}>{t("switchHistory.loading")}</div>
          ) : entries.length ? (
            entries.map((entry, index) => (
              <div className="switch-history-row" key={`${entry.atMs}-${index}`}>
                <span className="switch-history-apps">{entry.fromDisplay} → {entry.toDisplay}</span>
                <time className="switch-history-time" dateTime={new Date(entry.atMs).toISOString()}>
                  {new Date(entry.atMs).toLocaleString([], { dateStyle: "medium", timeStyle: "short" })}
                </time>
              </div>
            ))
          ) : (
            <div style={{ padding: "14px 0", color: "var(--muted)" }}>{t("switchHistory.empty")}</div>
          )}
        </div>
      </div>
    </div>
  );
}
