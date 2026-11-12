import { useEffect, useState } from "react";
import { api, type FocusRecord, type FocusSessionReport } from "../lib/ipc";
import { useStore } from "../lib/store";
import { fmtDur } from "../lib/time";

function pct(part: number, whole: number): number {
  return whole > 0 ? Math.round((part / whole) * 100) : 0;
}

function reliabilityTone(status: string): "good" | "warn" | "bad" | "unknown" {
  if (status.includes("high")) return "good";
  if (status.includes("mixed")) return "warn";
  if (status.includes("low")) return "bad";
  return "unknown";
}

function resolveAppLabel(appKey: string, labels: Map<string, string>): string {
  const label = labels.get(appKey)?.trim();
  if (label) return label;
  const key = appKey.trim();
  return key || "Unknown app";
}

export function FocusHistory() {
  const { version } = useStore();
  const [sessions, setSessions] = useState<FocusRecord[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [report, setReport] = useState<FocusSessionReport | null>(null);
  const [loadingList, setLoadingList] = useState(true);
  const [reportErrorId, setReportErrorId] = useState<number | null>(null);
  const [listError, setListError] = useState(false);

  useEffect(() => {
    let alive = true;
    setLoadingList(true);
    setListError(false);
    api.focusSessions().then((list) => {
      if (!alive) return;
      setSessions(list);
      setSelectedId((current) => current != null && list.some((session) => session.id === current)
        ? current
        : list.find((session) => session.id != null)?.id ?? null);
    }).catch(() => {
      if (alive) {
        setSessions([]);
        setListError(true);
      }
    }).finally(() => {
      if (alive) setLoadingList(false);
    });
    return () => {
      alive = false;
    };
  }, [version]);

  useEffect(() => {
    if (selectedId == null) {
      setReport(null);
      setReportErrorId(null);
      return;
    }
    let alive = true;
    setReport(null);
    setReportErrorId(null);
    api.focusSessionReport(selectedId).then((result) => {
      if (alive) setReport(result);
    }).catch(() => {
      if (alive) {
        setReport(null);
        setReportErrorId(selectedId);
      }
    });
    return () => {
      alive = false;
    };
  }, [selectedId, version]);

  const selected = sessions.find((session) => session.id === selectedId) ?? null;
  const reportForSelection = report && report.session.id === selectedId ? report : null;

  if (loadingList) {
    return (
      <div style={{ padding: 24, color: "var(--muted)" }}>Loading focus sessions…</div>
    );
  }

  if (!sessions.length) return <div style={{ padding: 24, color: "var(--muted)" }}>{listError ? "Focus session history could not be loaded." : "No focus sessions recorded yet."}</div>;

  return (
    <div style={{ display: "grid", gridTemplateColumns: "minmax(240px, 320px) minmax(0, 1fr)", gap: 18, padding: 18, minHeight: 0 }}>
      <aside style={{ display: "grid", gap: 12 }}>
        {sessions.map((session) => {
          const active = selected?.id === session.id;
          return (
            <button
              key={session.id ?? `${session.start}-${session.end}`}
              onClick={() => setSelectedId(session.id ?? null)}
              style={{
                textAlign: "left",
                border: active ? "1px solid var(--brand)" : "1px solid var(--line)",
                background: active ? "var(--surface-2)" : "var(--surface)",
                color: "var(--ink)",
                borderRadius: 12,
                padding: "12px 14px",
                display: "grid",
                gap: 6,
                cursor: "pointer",
              }}
            >
              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 8 }}>
                <strong>{new Date(session.start).toLocaleDateString(undefined, { month: "short", day: "numeric" })}</strong>
                <span style={{ fontSize: 11, opacity: 0.8 }}>{session.status}{session.recovered ? " · Recovered" : ""}</span>
              </div>
              <div style={{ fontSize: 12, opacity: 0.8 }}>
                {new Date(session.start).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
                {" – "}
                {new Date(session.end).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
              </div>
              <div style={{ fontSize: 12, opacity: 0.8 }}>
                {fmtDur(session.end - session.start)}
              </div>
            </button>
          );
        })}
      </aside>

      {selected && reportForSelection && <FocusSessionDetail key={selectedId} report={reportForSelection} />}
      {selected && !reportForSelection && reportErrorId === selectedId && <div style={{ padding: 12, color: "var(--muted)" }}>This session report could not be loaded.</div>}
      {selected && !reportForSelection && reportErrorId !== selectedId && <div style={{ padding: 12, color: "var(--muted)" }}>Reconstructing session evidence…</div>}
    </div>
  );
}

function FocusSessionDetail({ report }: { report: FocusSessionReport }) {
  const [showFindings, setShowFindings] = useState(false);
  const session = report.session;
  const reconstruction = report.reconstruction;
  const reliability = report.reliability;
  const appLabels = new Map<string, string>();
  const appMeta = new Map<string, { title: string; domain: string | null }>();
  for (const observation of reconstruction.observations) {
    if (observation.afk) continue;
    if (!appLabels.has(observation.appKey) && observation.display.trim()) {
      appLabels.set(observation.appKey, observation.display.trim());
    }
    if (!appMeta.has(observation.appKey)) {
      appMeta.set(observation.appKey, { title: observation.title.trim(), domain: observation.domain?.trim() ?? null });
    }
  }
  const appTotals = Object.entries(reconstruction.appTotals).sort(([, a], [, b]) => b - a);
  const totalLength = Math.max(0, session.end - session.start);
  const evidencePct = Math.round(reliability.coverage * 100);
  const attributableMs = reconstruction.dwellTimeMs;

  return (
    <div style={{ display: "grid", gap: 16, minHeight: 0 }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 12, flexWrap: "wrap" }}>
        <div>
          <div style={{ fontSize: 12, letterSpacing: "0.12em", textTransform: "uppercase", opacity: 0.7 }}>Focus session</div>
          <h2 style={{ margin: "4px 0 0" }}>{new Date(session.start).toLocaleString([], { dateStyle: "medium", timeStyle: "short" })}</h2>
        </div>
        <button
          style={{
            border: "1px solid var(--line)",
            background: "var(--surface)",
            color: "var(--ink)",
            borderRadius: 999,
            padding: "8px 12px",
            cursor: "pointer",
          }}
          onClick={() => setShowFindings(true)}
        >
          <span style={{ marginRight: 6 }}>
            <span style={{
              display: "inline-block",
              width: 8,
              height: 8,
              borderRadius: "50%",
              background: reliabilityTone(reliability.status) === "good" ? "var(--good)" : reliabilityTone(reliability.status) === "warn" ? "var(--warning)" : reliabilityTone(reliability.status) === "bad" ? "var(--critical)" : "var(--muted)",
              verticalAlign: "middle",
              marginRight: 6,
            }} />
            {reliability.status}
          </span>
        </button>
      </div>

      <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(150px, 1fr))", gap: 12 }}>
        <StatCard label="Actual duration" value={fmtDur(session.end - session.start)} />
        <StatCard label="Planned" value={fmtDur(session.plannedMs)} />
        <StatCard label="Observed" value={fmtDur(reconstruction.observedMs)} />
        <StatCard label="System UI observed" value={fmtDur(reconstruction.systemUiMs)} />
      </div>

      <div style={{ display: "grid", gridTemplateColumns: "minmax(0, 1.2fr) minmax(0, 0.8fr)", gap: 16, minHeight: 0 }}>
        <Panel title="Session summary">
          <MetricRow label="Observed" value={fmtDur(reconstruction.observedMs)} pct={pct(reconstruction.observedMs, totalLength)} />
          <MetricRow label="AFK observed" value={reconstruction.afkMs == null ? "Not separately evidenced" : fmtDur(reconstruction.afkMs)} />
          <MetricRow label="System UI observed" value={fmtDur(reconstruction.systemUiMs)} pct={pct(reconstruction.systemUiMs, totalLength)} />
          <MetricRow label="Untracked" value={fmtDur(reconstruction.untrackedMs)} pct={pct(reconstruction.untrackedMs, totalLength)} />
          <MetricRow label="Uncertain transition time" value={fmtDur(reconstruction.uncertainMs)} />
          <MetricRow label="Recovery gap metadata" value={fmtDur(reconstruction.recoveryGapMs)} />
          <div style={{ marginTop: 8, fontSize: 11, color: "var(--muted)" }}>
            {reliability.status} confidence · {evidencePct}% tracked-window coverage.
          </div>
        </Panel>

        <Panel title="Application totals">
          {appTotals.length ? (
            <div style={{ display: "grid", gap: 8 }}>
              {appTotals.slice(0, 8).map(([key, ms]) => (
                <MetricRow
                  key={key}
                  label={resolveAppLabel(key, appLabels)}
                  value={fmtDur(ms)}
                  pct={pct(ms, attributableMs)}
                />
              ))}
              <div style={{ fontSize: 11, color: "var(--muted)" }}>Share of attributed app time; system UI and untracked gaps are excluded.</div>
            </div>
          ) : (
            <div style={{ opacity: 0.7 }}>No app-level evidence was recorded in this session.</div>
          )}
        </Panel>
      </div>

      <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(260px, 1fr))", gap: 16, minHeight: 0 }}>
        <Panel title="Switching and dwell">
          <MetricRow label="Transitions" value={String(reconstruction.transitions.length)} />
          <MetricRow label="Short visits" value={String(reconstruction.shortVisits)} />
          <MetricRow label="Median dwell" value={fmtDur(reconstruction.medianDwellMs)} />
          <MetricRow label="Average dwell" value={fmtDur(reconstruction.averageDwellMs)} />
          {reconstruction.transitions.length > 0 && (
            <div style={{ maxHeight: 180, overflowY: "auto", overflowX: "hidden", marginTop: 10, paddingRight: 4 }}>
              <div style={{ display: "grid", gap: 6 }}>
                {reconstruction.transitions.map((transition, index) => (
                  <div key={`${transition.atMs}-${index}`} style={{ display: "flex", justifyContent: "space-between", gap: 10, borderTop: "1px solid var(--line)", paddingTop: 7 }}>
                    <span style={{ minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                      {resolveAppLabel(transition.fromApp, appLabels)} → {resolveAppLabel(transition.toApp, appLabels)}
                    </span>
                    <span style={{ flex: "none", color: transition.uncertain ? "var(--warning)" : "var(--muted)" }}>
                      {transition.uncertain ? `Uncertain · ${fmtDur(transition.gapMs)}` : new Date(transition.atMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          )}
        </Panel>

        <Panel title="Application visits">
          <div style={{ maxHeight: 280, overflowY: "auto", overflowX: "hidden", paddingRight: 4 }}>
            {reconstruction.appVisits.length ? (
              <div style={{ display: "grid", gap: 8 }}>
                {reconstruction.appVisits.map((visit, index) => {
                  const meta = appMeta.get(visit.appKey);
                  const name = resolveAppLabel(visit.appKey, appLabels);
                  return (
                    <div key={`${visit.appKey}-${visit.startMs}-${index}`} style={{ borderTop: "1px solid var(--line)", paddingTop: 8 }}>
                      <div style={{ display: "flex", justifyContent: "space-between", gap: 8 }}>
                        <strong>{name}</strong>
                        <span style={{ color: "var(--muted)" }}>{fmtDur(visit.durationMs)}</span>
                      </div>
                      <div style={{ fontSize: 11, color: "var(--muted)", marginTop: 3 }}>
                        {new Date(visit.startMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
                        {" – "}
                        {new Date(visit.endMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
                      </div>
                      {(meta?.domain || meta?.title) && (
                        <div style={{ fontSize: 11, color: "var(--muted)", marginTop: 3 }}>
                          {meta?.domain ?? meta?.title}
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            ) : (
              <div style={{ opacity: 0.7 }}>No application visits were reconstructed for this session.</div>
            )}
          </div>
        </Panel>
      </div>

      {showFindings && (
        <div className="modal-backdrop" onMouseDown={() => setShowFindings(false)}>
          <div className="modal" onMouseDown={(event) => event.stopPropagation()} style={{ maxWidth: 620, width: "min(620px, calc(100vw - 32px))" }}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 12 }}>
              <h3 style={{ margin: 0 }}>Reliability and evidence</h3>
              <button className="btn ghost" onClick={() => setShowFindings(false)}>Close</button>
            </div>
            <div style={{ display: "grid", gap: 10, marginTop: 12 }}>
              <div style={{ display: "grid", gridTemplateColumns: "repeat(2, minmax(0, 1fr))", gap: 8 }}>
                <Score label="Coverage" value={reliability.coverage} />
                <Score label="Observation quality" value={reliability.observationQuality} />
                <Score label="Tracking integrity" value={reliability.trackingIntegrity} />
                <Score label="Data integrity" value={reliability.dataIntegrity} />
              </div>

              <div style={{ border: "1px solid var(--line)", borderRadius: 10, padding: 12, display: "grid", gap: 6 }}>
                <MetricRow label="Observed" value={fmtDur(reconstruction.observedMs)} pct={pct(reconstruction.observedMs, totalLength)} />
                <MetricRow label="AFK observed" value={reconstruction.afkMs == null ? "Not separately evidenced" : fmtDur(reconstruction.afkMs)} />
                <MetricRow label="System UI observed" value={fmtDur(reconstruction.systemUiMs)} pct={pct(reconstruction.systemUiMs, totalLength)} />
                <MetricRow label="Untracked" value={fmtDur(reconstruction.untrackedMs)} pct={pct(reconstruction.untrackedMs, totalLength)} />
                <MetricRow label="Uncertain transition time" value={fmtDur(reconstruction.uncertainMs)} />
                <MetricRow label="Recovery gap metadata" value={fmtDur(reconstruction.recoveryGapMs)} />
              </div>

              {reliability.findings.length ? reliability.findings.map((finding) => (
                <div key={finding.code} style={{ border: "1px solid var(--line)", borderRadius: 10, padding: 12 }}>
                  <div style={{ display: "flex", justifyContent: "space-between", gap: 8, marginBottom: 6 }}>
                    <strong>{finding.code}</strong>
                    <span style={{ fontSize: 11, textTransform: "uppercase", opacity: 0.8 }}>{finding.severity}</span>
                  </div>
                  <div>{finding.summary}</div>
                </div>
              )) : <div style={{ opacity: 0.7 }}>No findings to report.</div>}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function StatCard({ label, value }: { label: string; value: string }) {
  return (
    <div style={{ border: "1px solid var(--line)", borderRadius: 12, padding: 12, background: "var(--surface)" }}>
      <div style={{ fontSize: 12, opacity: 0.75 }}>{label}</div>
      <div style={{ marginTop: 8, fontSize: 20, fontWeight: 700 }}>{value}</div>
    </div>
  );
}

function Panel({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div style={{ border: "1px solid var(--line)", borderRadius: 12, background: "var(--surface)", padding: 14, minWidth: 0 }}>
      <h3 style={{ margin: "0 0 12px" }}>{title}</h3>
      {children}
    </div>
  );
}

function MetricRow({ label, value, pct: percentage }: { label: string; value: string; pct?: number }) {
  return (
    <div style={{ display: "grid", gap: 4, padding: "8px 0" }}>
      <div style={{ display: "flex", justifyContent: "space-between", gap: 8 }}>
        <span>{label}</span>
        <strong>{value}</strong>
      </div>
      {percentage != null && <div style={{ height: 6, borderRadius: 999, background: "var(--surface-3)", overflow: "hidden" }}>
        <div style={{ height: "100%", width: `${Math.min(100, Math.max(0, percentage))}%`, background: "var(--brand)" }} />
      </div>}
    </div>
  );
}

function Score({ label, value }: { label: string; value: number }) {
  return (
    <div style={{ border: "1px solid var(--line)", borderRadius: 8, padding: 9 }}>
      <div style={{ color: "var(--muted)", fontSize: 11 }}>{label}</div>
      <strong>{Math.round(value * 100)}%</strong>
    </div>
  );
}
