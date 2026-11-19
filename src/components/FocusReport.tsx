

import { useEffect, useRef, useState } from "react";
import { useT } from "../lib/i18n";
import { api, type FocusRecord, type FocusSessionReport } from "../lib/ipc";
import { useStore } from "../lib/store";
import { fmtDur } from "../lib/time";
import { Icon } from "./Icon";

interface Report {
  rec: FocusRecord;
  report: FocusSessionReport;
}

export function FocusReport() {
  const { status, toast } = useStore();
  const t = useT();
  const prev = useRef(status?.focus ?? null);
  const [report, setReport] = useState<Report | null>(null);

  useEffect(() => {
    const was = prev.current;
    const now = status?.focus ?? null;
    prev.current = now;
    if (!was || now) return;

    let alive = true;
    (async () => {
      const recs = await api.focusHistory(was.start, Date.now() + 1000);
      const rec = recs.find((r) => r.start === was.start);
      if (!rec || rec.id == null || rec.end - rec.start < 60_000) return;
      const sessionReport = await api.focusSessionReport(rec.id);
      if (alive) setReport({ rec, report: sessionReport });
    })().catch(() => {
      if (alive) toast("Focus-session analysis could not be loaded.");
    });
    return () => {
      alive = false;
    };
  }, [status?.focus, toast]);

  if (!report) return null;
  const { rec, report: sessionReport } = report;
  const metrics = sessionReport.metrics;
  const baseline = sessionReport.baseline;
  const length = rec.end - rec.start;
  const minutes = Math.round(rec.plannedMs / 60_000);
  const plannedMinutes = Math.round(rec.plannedMs / 60_000);
  const remainingMs = Math.max(0, rec.plannedMs - length);
  const completedEarly = remainingMs > 0 && !rec.completed;
  const hasBaseline = baseline.sampleCount > 0
    && baseline.medianSwitchesPerObservedHour != null
    && baseline.medianAppRunMs != null;

  return (
    <div className="modal-backdrop" onMouseDown={() => setReport(null)}>
      <div className="modal focus-report" role="dialog" aria-label={t("focusReport.title")} onMouseDown={(e) => e.stopPropagation()}>
        <div className="fr-top">
          <div>
            <div className="fr-kicker">
              <Icon name="bolt" size={14} /> {t("focusReport.title")}
            </div>
            <div className="fr-sub">
              {t(rec.completed ? "focusReport.completed" : "focusReport.endedEarly", { time: fmtDur(length) })}
            </div>
          </div>
        </div>
        <div className="fr-outcome">
          <strong>{completedEarly ? `Ended ${fmtDur(remainingMs)} early` : `Planned ${fmtDur(rec.plannedMs)}`}</strong>
          {plannedMinutes > 0 && (
            <span>{fmtDur(length)} completed of {fmtDur(rec.plannedMs)} planned</span>
          )}
        </div>
        <div className="fr-metrics">
          <div className="fr-metric">
            <span>Tracked app-window time</span>
            <strong>{fmtDur(metrics.trackedAppMs)}</strong>
          </div>
          <div className="fr-metric">
            <span>Switches per tracked hour</span>
            <strong>{metrics.switchesPerObservedHour == null ? "—" : metrics.switchesPerObservedHour.toFixed(1)}</strong>
          </div>
          <div className="fr-metric">
            <span>Median same-app run</span>
            <strong>{metrics.medianAppRunMs == null ? "—" : fmtDur(metrics.medianAppRunMs)}</strong>
          </div>
        </div>

        <div className="fr-apps">
          <h3>Recent sessions</h3>
          {hasBaseline ? (
            <div className="fr-baseline">
              <span>Median across {baseline.sampleCount} earlier {baseline.sampleCount === 1 ? "session" : "sessions"}</span>
              <strong>{baseline.medianSwitchesPerObservedHour!.toFixed(1)} switches/hour</strong>
              <strong>{fmtDur(baseline.medianAppRunMs!)} median app run</strong>
            </div>
          ) : <p className="dim small">No comparison available yet.</p>}
        </div>

        <div className="fr-actions">
          <button className="btn ghost" onClick={() => setReport(null)}>
            Done
          </button>
          <button
            className="btn primary"
            onClick={async () => {
              setReport(null);
              await api.focusStart(minutes);
              toast(t("focus.started", { min: minutes }));
            }}
          >
            <Icon name="bolt" size={15} /> Another {minutes} minutes
          </button>
        </div>
      </div>
    </div>
  );
}
