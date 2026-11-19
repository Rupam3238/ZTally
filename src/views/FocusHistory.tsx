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
  const session = report.session;
  const metrics = report.metrics;
  const baseline = report.baseline;
  const enoughBaseline = baseline.sampleCount >= 3
    && baseline.medianSwitchesPerObservedHour != null
    && baseline.medianAppRunMs != null;
  const reliableEnough = metrics.coverage >= 0.6 && metrics.switchesPerObservedHour != null && metrics.medianAppRunMs != null;
  const rateAboveBaseline = enoughBaseline && reliableEnough
    && (baseline.medianSwitchesPerObservedHour! === 0
      ? metrics.switchesPerObservedHour! > 0
      : metrics.switchesPerObservedHour! >= baseline.medianSwitchesPerObservedHour! * 1.25);
  const runBelowBaseline = enoughBaseline && reliableEnough
    && metrics.medianAppRunMs! <= baseline.medianAppRunMs! * 0.75;
  const rateBelowBaseline = enoughBaseline && reliableEnough
    && (baseline.medianSwitchesPerObservedHour! === 0
      ? metrics.switchesPerObservedHour! === 0
      : metrics.switchesPerObservedHour! <= baseline.medianSwitchesPerObservedHour! * 0.75);
  const runAboveBaseline = enoughBaseline && reliableEnough
    && metrics.medianAppRunMs! >= baseline.medianAppRunMs! * 1.25;

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 12, flexWrap: "wrap" }}>
        <div>
          <div style={{ fontSize: 12, letterSpacing: "0.12em", textTransform: "uppercase", opacity: 0.7 }}>Focus pattern</div>
          <h2 style={{ margin: "4px 0 0" }}>{new Date(session.start).toLocaleString([], { dateStyle: "medium", timeStyle: "short" })}</h2>
        </div>
        <span style={{ color: "var(--muted)", fontSize: 12 }}>{session.status}{session.recovered ? " · Recovered" : ""}</span>
      </div>

      <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(150px, 1fr))", gap: 12 }}>
        <StatCard label="Actual duration" value={fmtDur(session.end - session.start)} />
        <StatCard label="Planned" value={fmtDur(session.plannedMs)} />
        <StatCard label="Evidence coverage" value={`${Math.round(metrics.coverage * 100)}%`} />
        <StatCard label="Window switches / tracked hour" value={metrics.switchesPerObservedHour == null ? "—" : metrics.switchesPerObservedHour.toFixed(1)} />
      </div>

      <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(260px, 1fr))", gap: 16 }}>
        <Panel title="Session pattern">
          <MetricRow label="Tracked app-window time" value={fmtDur(metrics.trackedAppMs)} />
          <MetricRow label="Time covered by observations" value={fmtDur(metrics.observedMs)} />
          <MetricRow label="Confident app-window switches" value={String(metrics.switchCount)} />
          <MetricRow label="Median same-app run" value={metrics.medianAppRunMs == null ? "Not enough data" : fmtDur(metrics.medianAppRunMs)} />
        </Panel>

        <Panel title="Your recent baseline">
          {enoughBaseline ? (
            <>
              <MetricRow label="Previous sessions compared" value={String(baseline.sampleCount)} />
              <MetricRow label="Typical switches per tracked hour" value={baseline.medianSwitchesPerObservedHour!.toFixed(1)} />
              <MetricRow label="Typical same-app run" value={fmtDur(baseline.medianAppRunMs!)} />
              <div style={{ fontSize: 11, color: "var(--muted)" }}>Baseline is the median of up to five recent sessions with at least 60% evidence coverage.</div>
            </>
          ) : (
            <div style={{ color: "var(--muted)" }}>A personal baseline needs at least three earlier sessions with usable tracking. No generic attention target is assumed.</div>
          )}
        </Panel>
      </div>

      <Panel title="What this suggests">
        {!reliableEnough ? (
          <p style={{ margin: 0, color: "var(--muted)" }}>Tracking evidence is too incomplete for a useful comparison. Treat this session as descriptive, not a measure of focus.</p>
        ) : !enoughBaseline ? (
          <p style={{ margin: 0, color: "var(--muted)" }}>Keep recording focus sessions to establish your own pattern. App-window activity cannot tell whether a switch was useful or whether your attention wandered.</p>
        ) : rateAboveBaseline && runBelowBaseline ? (
          <p style={{ margin: 0 }}>This session had more app switching and shorter same-app runs than your recent median. For one session, try a single planned check-in point and compare the pattern; this is an experiment, not a prescription.</p>
        ) : rateBelowBaseline && runAboveBaseline ? (
          <p style={{ margin: 0 }}>This session had fewer app switches and longer same-app runs than your recent median. See whether the pattern repeats under similar conditions before drawing conclusions.</p>
        ) : (
          <p style={{ margin: 0 }}>This session is close to your recent pattern. If you want to improve it, change one controllable condition next time and compare the result rather than chasing a universal target.</p>
        )}
        <p style={{ margin: "10px 0 0", color: "var(--muted)", fontSize: 12 }}>
          These are app-activity proxies, not direct measures of attention, task progress, or productivity. Task-switching research studies cognitive tasks; window changes alone cannot identify task switches or attention residue.
        </p>
        <div style={{ marginTop: 8, color: "var(--muted)", fontSize: 11 }}>
          Based on Monsell (2003), Leroy (2009), and Mark, Gudith &amp; Klocke (2008); see the project README for references.
        </div>
      </Panel>
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
    <div style={{ border: "1px solid var(--line)", borderRadius: 12, background: "var(--surface)", padding: 14 }}>
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
