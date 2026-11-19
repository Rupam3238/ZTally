

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

function resolveAppName(appKey: string, labels: Map<string, string>): string {
  const label = labels.get(appKey)?.trim();
  if (label) return label;
  const key = appKey.trim();
  return key || "Unknown app";
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
    })().catch(() => {});
    return () => {
      alive = false;
    };
  }, [status?.focus]);

  if (!report) return null;
  const { rec, report: sessionReport } = report;
  const reconstruction = sessionReport.reconstruction;
  const reliability = sessionReport.reliability;
  const labels = new Map<string, string>();
  for (const observation of reconstruction.observations) {
    if (!observation.afk && !labels.has(observation.appKey) && observation.display.trim()) {
      labels.set(observation.appKey, observation.display.trim());
    }
  }

  const appEntries = Object.entries(reconstruction.appTotals)
    .map(([key, ms]) => ({ key, label: resolveAppName(key, labels), ms }))
    .sort((a, b) => b.ms - a.ms)
    .slice(0, 3);

  const length = rec.end - rec.start;
  const minutes = Math.round(rec.plannedMs / 60_000);
  const plannedMinutes = Math.round(rec.plannedMs / 60_000);
  const remainingMs = Math.max(0, rec.plannedMs - length);
  const completedEarly = remainingMs > 0 && !rec.completed;

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
          <strong>{t(completedEarly ? "focusReport.finishedEarlyBy" : "focusReport.plannedDuration", {
            time: fmtDur(completedEarly ? remainingMs : rec.plannedMs),
            planned: fmtDur(rec.plannedMs),
            actual: fmtDur(length),
          })}</strong>
          {plannedMinutes > 0 && (
            <span>{t("focusReport.actualOfPlanned", { actual: fmtDur(length), planned: fmtDur(rec.plannedMs) })}</span>
          )}
        </div>
        <div className="fr-metrics">
          <div className="fr-metric">
            <span>{t("focusReport.appTime")}</span>
            <strong>{fmtDur(reconstruction.dwellTimeMs)}</strong>
          </div>
          <div className="fr-metric">
            <span>{t("focusReport.appSwitches")}</span>
            <strong>{reconstruction.transitions.length}</strong>
          </div>
          <div className="fr-metric">
            <span>{t("focusReport.typicalAppVisit")}</span>
            <strong>{reconstruction.medianDwellMs > 0 ? fmtDur(reconstruction.medianDwellMs) : t("focusReport.notEnoughData")}</strong>
          </div>
        </div>

        <div className="fr-apps">
          <h3>{t("focusReport.topApplications")}</h3>
          {appEntries.length ? (
            appEntries.map((a) => (
              <div key={a.key} className="fr-row">
                <span className="grow ellipsis">{a.label}</span>
                <span className="mono">{fmtDur(a.ms)}</span>
              </div>
            ))
          ) : (
            <p className="dim small">{t("focusReport.nothing")}</p>
          )}
        </div>

        <div className="fr-actions">
          <button className="btn ghost" onClick={() => setShowDetails(true)}>
            {t("focusReport.details")}
          </button>
          <button className="btn ghost" onClick={() => setReport(null)}>
            {t("focusReport.done")}
          </button>
          <button
            className="btn primary"
            onClick={async () => {
              setReport(null);
              await api.focusStart(minutes);
              toast(t("focus.started", { min: minutes }));
            }}
          >
            <Icon name="bolt" size={15} /> {t("focusReport.again", { min: minutes })}
          </button>
        </div>
      </div>

      {showDetails && (
        <div className="modal-backdrop" onMouseDown={() => setShowDetails(false)}>
          <div className="modal" onMouseDown={(event) => event.stopPropagation()} style={{ maxWidth: 560, width: "min(560px, calc(100vw - 32px))" }}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 12 }}>
              <h3 style={{ margin: 0 }}>Reliability details</h3>
              <button className="btn ghost" onClick={() => setShowDetails(false)}>Close</button>
            </div>
            <div style={{ display: "grid", gap: 8, marginTop: 12 }}>
              <div style={{ border: "1px solid var(--line)", borderRadius: 10, padding: 10 }}>
                <strong>{reliability.status}</strong>
                <div style={{ marginTop: 6, color: "var(--muted)", fontSize: 12 }}>
                  Coverage {Math.round(reliability.coverage * 100)}% · Observation quality {Math.round(reliability.observationQuality * 100)}% · Tracking integrity {Math.round(reliability.trackingIntegrity * 100)}%
                </div>
              </div>
              {reliability.findings.length ? reliability.findings.map((finding) => (
                <div key={finding.code} style={{ border: "1px solid var(--line)", borderRadius: 10, padding: 10 }}>
                  <div style={{ display: "flex", justifyContent: "space-between", gap: 8, marginBottom: 6 }}>
                    <strong>{finding.code}</strong>
                    <span style={{ fontSize: 11, textTransform: "uppercase", opacity: 0.8 }}>{finding.severity}</span>
                  </div>
                  <div>{finding.summary}</div>
                </div>
              )) : <div style={{ opacity: 0.7 }}>No reliability findings for this session.</div>}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
