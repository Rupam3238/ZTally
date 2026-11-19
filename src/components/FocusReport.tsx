

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
  const [showDetails, setShowDetails] = useState(false);

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
  const enoughBaseline = baseline.sampleCount >= 3
    && baseline.medianSwitchesPerObservedHour != null
    && baseline.medianAppRunMs != null;
  const reliableEnough = metrics.coverage >= 0.6
    && metrics.switchesPerObservedHour != null
    && metrics.medianAppRunMs != null;
  const rateAboveBaseline = enoughBaseline && reliableEnough
    && (baseline.medianSwitchesPerObservedHour! === 0
      ? metrics.switchesPerObservedHour! > 0
      : metrics.switchesPerObservedHour! >= baseline.medianSwitchesPerObservedHour! * 1.25);
  const runBelowBaseline = enoughBaseline && reliableEnough
    && metrics.medianAppRunMs! <= baseline.medianAppRunMs! * 0.75;

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
            <strong>{metrics.medianAppRunMs == null ? "Not enough data" : fmtDur(metrics.medianAppRunMs)}</strong>
          </div>
        </div>

        <div className="fr-apps">
          <h3>Your pattern</h3>
          <p className="dim small">
            {metrics.coverage < 0.6
              ? "Tracking evidence is incomplete, so treat these figures as descriptive only."
              : !enoughBaseline
                ? "More earlier sessions are needed to compare this with your personal pattern."
                : rateAboveBaseline && runBelowBaseline
                  ? "Switching was higher and same-app runs were shorter than your recent median. Try one planned check-in point next time, then compare."
                  : "Use this as a personal comparison, not a score. Change one condition at a time and see whether the pattern repeats."}
          </p>
          <p className="dim small">
            App-window activity is only an indirect proxy; it cannot measure attention, task progress, or whether a switch was useful.
          </p>
        </div>

        <div className="fr-actions">
          <button className="btn ghost" onClick={() => setShowDetails(true)}>
            How to read this
          </button>
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

      {showDetails && (
        <div className="modal-backdrop" onMouseDown={() => setShowDetails(false)}>
          <div className="modal" onMouseDown={(event) => event.stopPropagation()} style={{ maxWidth: 560, width: "min(560px, calc(100vw - 32px))" }}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 12 }}>
              <h3 style={{ margin: 0 }}>How to read this</h3>
              <button className="btn ghost" onClick={() => setShowDetails(false)}>Close</button>
            </div>
            <div style={{ display: "grid", gap: 8, marginTop: 12 }}>
              <p>Evidence coverage: {Math.round(metrics.coverage * 100)}%. The personal baseline uses the median of up to five earlier sessions with at least 60% coverage.</p>
              {enoughBaseline && (
                <p>Recent median: {baseline.medianSwitchesPerObservedHour!.toFixed(1)} app-window switches per tracked hour and {fmtDur(baseline.medianAppRunMs!)} per same-app run, based on {baseline.sampleCount} sessions.</p>
              )}
              <p>These are not scientific measurements of attention. Task-switching studies measure controlled task changes; window changes cannot tell whether you switched tasks, experienced attention residue, or made progress.</p>
              <p>Interpret the metrics as personal observations, and compare sessions with similar goals and conditions.</p>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
