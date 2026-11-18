import { useEffect, useState } from "react";
import { api, type SwitchHistoryEntry } from "../lib/ipc";
import { useT } from "../lib/i18n";
import { dayBounds } from "../lib/time";

type HistoryPeriod = "today" | "custom" | "all";

function localDateValue(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function dateRangeBounds(from: string, to: string): [number, number] | null {
  if (!from || !to || from > to) return null;
  const [fromYear, fromMonth, fromDay] = from.split("-").map(Number);
  const [toYear, toMonth, toDay] = to.split("-").map(Number);
  const start = new Date(fromYear, fromMonth - 1, fromDay);
  const lastDay = new Date(toYear, toMonth - 1, toDay);
  if (
    start.getFullYear() !== fromYear || start.getMonth() !== fromMonth - 1 || start.getDate() !== fromDay ||
    lastDay.getFullYear() !== toYear || lastDay.getMonth() !== toMonth - 1 || lastDay.getDate() !== toDay
  ) return null;
  return [start.getTime(), new Date(toYear, toMonth - 1, toDay + 1).getTime()];
}

export function SwitchHistoryModal({ onClose }: { onClose: () => void }) {
  const t = useT();
  const [period, setPeriod] = useState<HistoryPeriod>("today");
  const [fromDate, setFromDate] = useState(() => localDateValue(new Date()));
  const [toDate, setToDate] = useState(() => localDateValue(new Date()));
  const [entries, setEntries] = useState<SwitchHistoryEntry[] | null>(null);
  const [failed, setFailed] = useState(false);
  const customBounds = dateRangeBounds(fromDate, toDate);

  useEffect(() => {
    let start: number | null = null;
    let end: number | null = null;
    if (period === "today") {
      [start, end] = dayBounds(new Date());
    } else if (period === "custom") {
      const bounds = dateRangeBounds(fromDate, toDate);
      if (!bounds) {
        setEntries(null);
        setFailed(false);
        return;
      }
      [start, end] = bounds;
    }

    let alive = true;
    setEntries(null);
    setFailed(false);
    api.switchHistory(start, end).then((history) => {
      if (alive) setEntries(history);
    }).catch(() => {
      if (alive) setFailed(true);
    });
    return () => {
      alive = false;
    };
  }, [period, fromDate, toDate]);

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
        <div className="segmented sm switch-history-period" role="group" aria-label={t("switchHistory.period")}>
          <button className={period === "today" ? "on" : ""} aria-pressed={period === "today"} onClick={() => setPeriod("today")}>
            {t("switchHistory.today")}
          </button>
          <button className={period === "custom" ? "on" : ""} aria-pressed={period === "custom"} onClick={() => setPeriod("custom")}>
            {t("switchHistory.customRange")}
          </button>
          <button className={period === "all" ? "on" : ""} aria-pressed={period === "all"} onClick={() => setPeriod("all")}>
            {t("switchHistory.allTime")}
          </button>
        </div>
        {period === "custom" && (
          <div className="switch-history-dates">
            <label className="switch-history-date-field">
              <span>{t("switchHistory.from")}</span>
              <input type="date" value={fromDate} aria-label={t("switchHistory.from")} onChange={(event) => setFromDate(event.currentTarget.value)} />
            </label>
            <label className="switch-history-date-field">
              <span>{t("switchHistory.to")}</span>
              <input type="date" value={toDate} aria-label={t("switchHistory.to")} onChange={(event) => setToDate(event.currentTarget.value)} />
            </label>
          </div>
        )}
        <div className="switch-history-list">
          {period === "custom" && customBounds === null ? (
            <div style={{ padding: "14px 0", color: "var(--muted)" }}>{t("switchHistory.invalidRange")}</div>
          ) : failed ? (
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
