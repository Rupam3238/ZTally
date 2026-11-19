import { useEffect, useState } from "react";
import { api, type FocusRecord, type FocusSessionReport } from "../lib/ipc";
import { useStore } from "../lib/store";
import { fmtDur } from "../lib/time";

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
    <div className="focus-history-layout">
      <aside className="focus-history-list">
        {sessions.map((session) => {
          const active = selected?.id === session.id;
          return (
            <button
              key={session.id ?? `${session.start}-${session.end}`}
              onClick={() => setSelectedId(session.id ?? null)}
              className={`focus-session-button${active ? " active" : ""}`}
            >
              <div className="focus-session-button-top">
                <strong>{new Date(session.start).toLocaleDateString(undefined, { month: "short", day: "numeric" })}</strong>
                <span>{session.recovered ? "Recovered" : sessionStatus(session.status)}</span>
              </div>
              <div className="focus-session-button-time">
                {new Date(session.start).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
                {" – "}
                {new Date(session.end).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
              </div>
              <div className="focus-session-button-duration">
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
  const session = report.session;
  const metrics = report.metrics;
  const baseline = report.baseline;
  const hasBaseline = baseline.sampleCount > 0
    && baseline.medianSwitchesPerObservedHour != null
    && baseline.medianAppRunMs != null;

  return (
    <div className="focus-session-detail">
      <div className="focus-session-heading">
        <div>
          <div className="focus-session-kicker">Focus session</div>
          <h2>{new Date(session.start).toLocaleString([], { dateStyle: "medium", timeStyle: "short" })}</h2>
        </div>
        <span className="focus-session-status">{session.recovered ? "Recovered" : sessionStatus(session.status)}</span>
      </div>

      <div className="focus-session-stats">
        <StatCard label="Duration" value={fmtDur(session.end - session.start)} />
        <StatCard label="Planned" value={fmtDur(session.plannedMs)} />
        <StatCard label="Tracked app time" value={fmtDur(metrics.trackedAppMs)} />
        <StatCard label="Observed" value={`${Math.round(metrics.coverage * 100)}%`} />
      </div>

      <div className="focus-session-panels">
        <Panel title="App-window activity">
          <MetricRow label="Window switches" value={String(metrics.switchCount)} />
          <MetricRow label="Switches per tracked hour" value={metrics.switchesPerObservedHour == null ? "—" : metrics.switchesPerObservedHour.toFixed(1)} />
          <MetricRow label="Median time in one app" value={metrics.medianAppRunMs == null ? "—" : fmtDur(metrics.medianAppRunMs)} />
        </Panel>

        <Panel title="Recent sessions">
          {hasBaseline ? (
            <div className="focus-session-baseline">
              <span>Median across {baseline.sampleCount} earlier {baseline.sampleCount === 1 ? "session" : "sessions"}</span>
              <strong>{baseline.medianSwitchesPerObservedHour!.toFixed(1)} switches/hour</strong>
              <strong>{fmtDur(baseline.medianAppRunMs!)} median app run</strong>
            </div>
          ) : (
            <div className="focus-session-empty">No comparison available yet.</div>
          )}
        </Panel>
      </div>
    </div>
  );
}

function sessionStatus(status: string): string {
  switch (status) {
    case "completed": return "Completed";
    case "ended_early": return "Ended early";
    case "interrupted": return "Interrupted";
    case "active": return "In progress";
    default: return status;
  }
}

function StatCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="focus-session-stat">
      <div>{label}</div>
      <strong>{value}</strong>
    </div>
  );
}

function Panel({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="focus-session-panel">
      <h3>{title}</h3>
      {children}
    </section>
  );
}

function MetricRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="focus-session-metric">
      <div>
        <span>{label}</span>
        <strong>{value}</strong>
      </div>
    </div>
  );
}
