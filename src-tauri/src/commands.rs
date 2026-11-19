

use crate::classify::Rule;
use crate::db::{AppEntry, Category, Goal};
use crate::model::Settings;
use crate::report::{self, Ctx, DayView, GoalProgress, RangeView};
use crate::tracker::{BackendInfo, Current};
use crate::AppState;
use serde::Serialize;
use std::collections::HashMap;
use tauri::{AppHandle, State};

type R<T> = Result<T, String>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusSessionReport {
    session: FocusSessionSummary,
    metrics: crate::reconstruct::FocusSessionMetrics,
    baseline: FocusSessionBaseline,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FocusSessionSummary {
    id: Option<i64>,
    start: i64,
    end: i64,
    planned_ms: i64,
    drift_ms: i64,
    completed: bool,
    status: String,
    recovered: bool,
}

impl From<crate::db::FocusRecord> for FocusSessionSummary {
    fn from(session: crate::db::FocusRecord) -> Self {
        Self {
            id: session.id,
            start: session.start,
            end: session.end,
            planned_ms: session.planned_ms,
            drift_ms: session.drift_ms,
            completed: session.completed,
            status: session.status,
            recovered: session.recovered,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FocusSessionBaseline {
    sample_count: usize,
    median_switches_per_observed_hour: Option<f64>,
    median_app_run_ms: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchHistoryEntry {
    from_app: String,
    from_display: String,
    to_app: String,
    to_display: String,
    at_ms: i64,
    uncertain: bool,
    gap_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    backend: BackendInfo,
    current: Current,
    paused_until: i64,
    extension_connected: bool,
    extension_browser: Option<String>,
    db_path: String,
    first_day: Option<i64>,
    version: &'static str,
    platform: &'static str,
    focus: Option<crate::focus::FocusSession>,

    today_ms: i64,
}

#[tauri::command]
fn status(st: State<AppState>) -> R<Status> {


    let current = st.engine.lock().current.clone();
    st.engine.lock().flush();
    let first_day = st.db.lock().first_span_start()?;
    let (s, e) = crate::today_bounds();
    let today_ms = st.db.lock().spans_in(s, e)?.iter().filter(|r| !r.afk).map(|r| r.end - r.start).sum();
    let paused_until = *st.paused_until.lock();
    let focus = st.focus.lock().clone();
    Ok(Status {
        backend: st.backend.clone(),
        current,
        paused_until,
        extension_connected: st.bridge.connected(),
        extension_browser: st.bridge.connected_browser(),
        db_path: st.db_path.clone(),
        first_day,
        version: env!("CARGO_PKG_VERSION"),
        platform: if cfg!(windows) { "windows" } else { "linux" },
        focus,
        today_ms,
    })
}

#[tauri::command]
fn day(st: State<AppState>, start: i64, end: i64) -> R<DayView> {
    st.engine.lock().flush();
    let rows = st.db.lock().spans_in(start, end)?;
    let audio = st.db.lock().audio_in(start, end)?;
    let calls = st.db.lock().calls_in(start, end)?;
    let cats = st.db.lock().categories()?;
    let c = st.classifier();
    let mut view = report::day_view(&Ctx::new(&c, &cats), &rows, &audio, &calls, start, end);
    set_switch_metrics(&mut view.summary, &rows, &[(start, end)]);
    Ok(view)
}


fn runs(days: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut sorted = days.to_vec();
    sorted.sort();
    let mut out: Vec<(i64, i64)> = vec![];
    for (s, e) in sorted {
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

fn switch_metrics(rows: &[crate::db::SpanRow], intervals: &[(i64, i64)]) -> (u32, i64) {
    let mut switches = 0u32;
    let mut eligible_ms = 0i64;
    for (start, end) in runs(intervals) {
        let reconstruction = crate::reconstruct::reconstruct_session(start, end, rows, 0);
        switches = switches.saturating_add(
            reconstruction.transitions.iter().filter(|transition| !transition.uncertain).count() as u32,
        );
        eligible_ms += reconstruction.eligible_observed_ms;
    }
    (switches, eligible_ms)
}

fn set_switch_metrics(summary: &mut report::Summary, rows: &[crate::db::SpanRow], intervals: &[(i64, i64)]) {
    (summary.switches, summary.switch_eligible_ms) = switch_metrics(rows, intervals);
}

type Rows = (Vec<crate::db::SpanRow>, Vec<crate::db::SpanRow>, Vec<crate::db::SpanRow>);

fn rows_for(st: &AppState, days: &[(i64, i64)]) -> R<Rows> {
    st.engine.lock().flush();
    let (mut rows, mut audio, mut calls) = (vec![], vec![], vec![]);
    for (s, e) in runs(days) {
        rows.extend(st.db.lock().spans_in(s, e)?);
        audio.extend(st.db.lock().audio_in(s, e)?);
        calls.extend(st.db.lock().calls_in(s, e)?);
    }
    Ok((rows, audio, calls))
}

#[tauri::command]
fn range(st: State<AppState>, days: Vec<(i64, i64)>) -> R<RangeView> {
    if days.is_empty() {
        return Err("empty range".into());
    }
    let mut days = days;
    days.sort();
    let (rows, audio, calls) = rows_for(&st, &days)?;
    let cats = st.db.lock().categories()?;
    let c = st.classifier();
    let mut view = report::range_view(&Ctx::new(&c, &cats), &rows, &audio, &calls, &days);
    set_switch_metrics(&mut view.summary, &rows, &days);
    Ok(view)
}

#[tauri::command]
fn switch_history(st: State<AppState>, start: Option<i64>, end: Option<i64>) -> R<Vec<SwitchHistoryEntry>> {
    st.engine.lock().flush();
    let spans = match (start, end) {
        (Some(start), Some(end)) if end > start => st.db.lock().spans_in(start, end)?,
        (None, None) => st.db.lock().all_spans()?,
        _ => return Err("A valid start and end are required for a switch-history range.".into()),
    };
    let Some(start_ms) = spans.iter().map(|span| span.start).min() else {
        return Ok(Vec::new());
    };
    let Some(end_ms) = spans.iter().map(|span| span.end).max() else {
        return Ok(Vec::new());
    };
    if end_ms <= start_ms {
        return Ok(Vec::new());
    }

    let reconstruction = crate::reconstruct::reconstruct_session(start_ms, end_ms, &spans, 0);
    let mut labels = HashMap::new();
    for observation in &reconstruction.observations {
        if !observation.afk && !observation.display.trim().is_empty() {
            labels.entry(observation.app_key.clone()).or_insert_with(|| observation.display.trim().to_string());
        }
    }
    Ok(reconstruction
        .transitions
        .into_iter()
        .filter(|transition| !transition.uncertain)
        .map(|transition| SwitchHistoryEntry {
            from_display: labels.get(&transition.from_app).cloned().unwrap_or_else(|| transition.from_app.clone()),
            to_display: labels.get(&transition.to_app).cloned().unwrap_or_else(|| transition.to_app.clone()),
            from_app: transition.from_app,
            to_app: transition.to_app,
            at_ms: transition.at_ms,
            uncertain: transition.uncertain,
            gap_ms: transition.gap_ms,
        })
        .collect())
}


#[tauri::command]
fn categories(st: State<AppState>) -> R<Vec<Category>> {
    st.db.lock().categories()
}

#[tauri::command]
fn save_category(st: State<AppState>, category: Category) -> R<i64> {
    if category.name.trim().is_empty() {
        return Err("Name can't be empty".into());
    }
    if !matches!(category.kind.as_str(), "productive" | "neutral" | "distracting") {
        return Err("Invalid kind".into());
    }
    let id = st.db.lock().upsert_category(&Category { slot: category.slot.clamp(1, 8), ..category })?;
    st.reload_rules();
    Ok(id)
}

#[tauri::command]
fn delete_category(st: State<AppState>, id: i64) -> R<()> {
    st.db.lock().delete_category(id)?;
    st.reload_rules();
    Ok(())
}

#[tauri::command]
fn rules(st: State<AppState>) -> R<Vec<Rule>> {
    st.db.lock().rules()
}

#[tauri::command]
fn add_rule(st: State<AppState>, match_type: String, pattern: String, category_id: i64) -> R<i64> {
    if !matches!(match_type.as_str(), "app" | "domain" | "title") {
        return Err("Invalid rule type".into());
    }
    if pattern.trim().is_empty() {
        return Err("Pattern can't be empty".into());
    }

    let id = st.db.lock().add_rule(&match_type, &pattern, category_id)?;
    st.reload_rules();
    Ok(id)
}

#[tauri::command]
fn delete_rule(st: State<AppState>, id: i64) -> R<()> {
    st.db.lock().delete_rule(id)?;
    st.reload_rules();
    Ok(())
}

#[tauri::command]
fn reset_rules(st: State<AppState>) -> R<()> {
    st.db.lock().reset_rules_to_defaults(false)?;
    st.reload_rules();
    Ok(())
}


#[tauri::command]
fn goals(st: State<AppState>) -> R<Vec<Goal>> {
    st.db.lock().goals()
}

#[tauri::command]
fn save_goal(st: State<AppState>, goal: Goal) -> R<i64> {
    if goal.minutes <= 0 {
        return Err("Minutes must be positive".into());
    }
    st.db.lock().upsert_goal(&goal)
}

#[tauri::command]
fn delete_goal(st: State<AppState>, id: i64) -> R<()> {
    st.db.lock().delete_goal(id)
}

#[tauri::command]
fn goal_progress(st: State<AppState>, day: (i64, i64), week: (i64, i64)) -> R<Vec<GoalProgress>> {
    st.engine.lock().flush();
    let goals = st.db.lock().goals()?;
    let cats = st.db.lock().categories()?;
    let c = st.classifier();
    let ctx = Ctx::new(&c, &cats);
    let day_rows = st.db.lock().spans_in(day.0, day.1)?;
    let week_rows = st.db.lock().spans_in(week.0, week.1)?;
    Ok(goals.iter().map(|g| report::goal_progress(&ctx, g, if g.period == "week" { &week_rows } else { &day_rows })).collect())
}


#[tauri::command]
fn settings(st: State<AppState>) -> Settings {
    st.settings.lock().clone()
}

#[tauri::command]
fn save_settings(app: AppHandle, st: State<AppState>, settings: Settings) -> R<()> {
    let settings = Settings { idle_threshold_s: settings.idle_threshold_s.clamp(30, 3600), ..settings };
    st.db.lock().save_settings(&settings)?;
    let relabel = {
        let old = st.settings.lock();
        old.language != settings.language || old.focus_minutes != settings.focus_minutes
    };
    *st.settings.lock() = settings;
    if relabel {
        crate::apply_language(&app);
    }
    Ok(())
}

#[tauri::command]
fn pause(app: AppHandle, until: i64) {
    crate::pause_until(&app, until);
}

#[tauri::command]
fn focus_start(app: AppHandle, minutes: i64) -> R<()> {
    crate::start_focus(&app, minutes)
}

#[tauri::command]
fn focus_stop(app: AppHandle) -> R<()> {
    crate::end_focus(&app, false)
}

#[tauri::command]
fn focus_history(st: State<AppState>, start: i64, end: i64) -> R<Vec<FocusSessionSummary>> {
    st.db
        .lock()
        .focus_in(start, end)
        .map(|sessions| sessions.into_iter().map(Into::into).collect())
}

#[tauri::command]
fn focus_sessions(st: State<AppState>) -> R<Vec<FocusSessionSummary>> {
    st.db
        .lock()
        .focus_sessions()
        .map(|sessions| sessions.into_iter().map(Into::into).collect())
}

#[tauri::command]
fn focus_session_report(st: State<AppState>, id: i64) -> R<FocusSessionReport> {
    st.engine.lock().flush();
    let db = st.db.lock();
    let session = db.focus_by_id(id)?.ok_or_else(|| format!("focus session {id} not found"))?;
    let spans = db.spans_in(session.start, session.end)?;
    let reconstruction = crate::reconstruct::reconstruct_session(session.start, session.end, &spans, session.recovery_gap_ms);
    let reliability = crate::reconstruct::assess_reliability(&reconstruction);
    let metrics = crate::reconstruct::focus_session_metrics(&reconstruction, &reliability);
    let prior_sessions = db.focus_sessions()?
        .into_iter()
        .filter(|candidate| candidate.start < session.start && candidate.end <= session.start && candidate.status != "active");
    let mut baseline_metrics = Vec::new();
    for candidate in prior_sessions {
        if baseline_metrics.len() == 5 {
            break;
        }
        let rows = db.spans_in(candidate.start, candidate.end)?;
        let reconstructed = crate::reconstruct::reconstruct_session(
            candidate.start,
            candidate.end,
            &rows,
            candidate.recovery_gap_ms,
        );
        let assessed = crate::reconstruct::assess_reliability(&reconstructed);
        let prior = crate::reconstruct::focus_session_metrics(&reconstructed, &assessed);
        if prior.tracked_app_ms > 0 && prior.coverage >= 0.6 {
            baseline_metrics.push(prior);
        }
    }
    let baseline = FocusSessionBaseline {
        sample_count: baseline_metrics.len(),
        median_switches_per_observed_hour: median_f64(
            baseline_metrics.iter().filter_map(|m| m.switches_per_observed_hour).collect(),
        ),
        median_app_run_ms: median_i64(
            baseline_metrics.iter().filter_map(|m| m.median_app_run_ms).collect(),
        ),
    };
    Ok(FocusSessionReport { session: session.into(), metrics, baseline })
}

fn median_i64(mut values: Vec<i64>) -> Option<i64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 0 {
        values[middle - 1] / 2 + values[middle] / 2
            + (values[middle - 1] % 2 + values[middle] % 2) / 2
    } else {
        values[middle]
    })
}

fn median_f64(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 0 {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    })
}

#[tauri::command]
fn apps(st: State<AppState>) -> R<Vec<AppEntry>> {
    st.engine.lock().flush();
    st.db.lock().apps()
}

#[tauri::command]
fn domains(st: State<AppState>) -> R<Vec<(String, i64)>> {
    st.db.lock().domains()
}

#[tauri::command]
fn rename_app(st: State<AppState>, key: String, display: String) -> R<()> {
    st.db.lock().rename_app(&key, display.trim())
}

#[tauri::command]
fn forget(st: State<AppState>, start: i64, end: i64, app_key: Option<String>) -> R<usize> {
    st.engine.lock().shutdown();
    st.db.lock().delete_spans(start, end, app_key.as_deref())
}


#[tauri::command]
fn forget_days(st: State<AppState>, days: Vec<(i64, i64)>) -> R<usize> {
    st.engine.lock().shutdown();
    let mut n = 0;
    for (s, e) in runs(&days) {
        n += st.db.lock().delete_spans(s, e, None)?;
    }
    Ok(n)
}


#[tauri::command]
fn reset_all(app: AppHandle, st: State<AppState>) -> R<()> {
    *st.focus.lock() = None;
    st.engine.lock().forget_cache();
    st.db.lock().wipe()?;
    *st.settings.lock() = st.db.lock().settings();
    *st.paused_until.lock() = 0;
    st.reload_rules();
    crate::apply_language(&app);
    Ok(())
}

#[tauri::command]
fn export(st: State<AppState>, days: Vec<(i64, i64)>, format: String, path: String) -> R<usize> {
    let (rows, audio, calls) = rows_for(&st, &days)?;
    let cats = st.db.lock().categories()?;
    let c = st.classifier();
    let out = report::export(&Ctx::new(&c, &cats), &rows, &audio, &calls, &cats, &format);
    std::fs::write(&path, out).map_err(|e| e.to_string())?;
    Ok(rows.len() + audio.len() + calls.len())
}

#[tauri::command]
fn save_bytes(path: String, data: Vec<u8>) -> R<()> {
    std::fs::write(path, data).map_err(|e| e.to_string())
}

#[tauri::command]
fn install_gnome_extension() -> R<String> {
    #[cfg(target_os = "linux")]
    {
        crate::tracker::install_gnome_extension()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("Only available on Linux".into())
    }
}

const EXT_FILES: &[(&str, &str)] = &[
    ("manifest.json", include_str!("../../extension/manifest.json")),
    ("background.js", include_str!("../../extension/background.js")),
    ("README.txt", include_str!("../../extension/README.txt")),
];
const EXT_ICON: &[u8] = include_bytes!("../../extension/icon-128.png");


#[tauri::command]
fn open_browser_extension() -> R<String> {
    let dir = prepare_extension()?;
    let _ = tauri_plugin_opener::open_path(&dir, None::<&str>);
    Ok(dir)
}


#[tauri::command]
fn prepare_extension() -> R<String> {
    let dir = crate::data_dir().join("browser-extension");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for (name, body) in EXT_FILES {
        std::fs::write(dir.join(name), body).map_err(|e| e.to_string())?;
    }
    std::fs::write(dir.join("icon-128.png"), EXT_ICON).map_err(|e| e.to_string())?;
    Ok(dir.to_string_lossy().into_owned())
}

#[tauri::command]
fn detect_browsers() -> Vec<crate::browsers::BrowserInfo> {
    crate::browsers::detect()
}

#[tauri::command]
fn open_extension_page(id: String) -> R<bool> {
    crate::browsers::open_extension_page(&id)
}


#[tauri::command]
fn open_link(target: String) -> R<()> {
    let url = match target.as_str() {
        "website" => "https://zsync.eu/ztally/",
        "source" => "https://github.com/TheHolyOneZ/ZTally",
        "author" => "https://github.com/TheHolyOneZ",
        "projects" => "https://zsync.eu/",
        "mods" => "https://zlogic.eu/",
        _ => return Err("unknown link".into()),
    };
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_data_folder() -> R<()> {
    tauri_plugin_opener::open_path(crate::data_dir(), None::<&str>).map_err(|e| e.to_string())
}

pub fn handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        status,
        day,
        range,
        switch_history,
        categories,
        save_category,
        delete_category,
        rules,
        add_rule,
        delete_rule,
        reset_rules,
        goals,
        save_goal,
        delete_goal,
        goal_progress,
        settings,
        save_settings,
        pause,
        focus_start,
        focus_stop,
        focus_history,
        focus_sessions,
        focus_session_report,
        apps,
        domains,
        rename_app,
        forget,
        forget_days,
        reset_all,
        export,
        save_bytes,
        install_gnome_extension,
        open_browser_extension,
        open_data_folder,
        open_link,
        prepare_extension,
        detect_browsers,
        open_extension_page,
    ]
}

#[cfg(test)]
mod tests {
    use crate::db::SpanRow;

    fn span(start: i64, end: i64, app_key: &str, afk: bool) -> SpanRow {
        SpanRow {
            start,
            end,
            app_key: app_key.to_string(),
            display: app_key.to_string(),
            title: String::new(),
            domain: None,
            afk,
        }
    }

    #[test]
    fn runs_merge_touching_days_only() {
        assert_eq!(super::runs(&[(20, 30), (0, 10), (10, 20), (50, 60)]), vec![(0, 30), (50, 60)]);
        assert_eq!(super::runs(&[]), vec![]);
    }

    #[test]
    fn switch_metrics_exclude_shell_and_unknown_gap_transitions() {
        let rows = vec![
            span(0, 2_000, "firefox", false),
            span(1_000, 2_000, "plasmashell", false),
            span(2_000, 3_000, "code", false),
            span(5_000, 6_000, "dolphin", false),
        ];
        let (switches, eligible_ms) = super::switch_metrics(&rows, &[(0, 6_000)]);
        assert_eq!(switches, 1);
        assert_eq!(eligible_ms, 3_000);
    }

    #[test]
    fn switch_metrics_do_not_bridge_separate_reporting_intervals() {
        let rows = vec![
            span(0, 1_000, "firefox", false),
            span(5_000, 6_000, "code", false),
        ];
        let (switches, eligible_ms) = super::switch_metrics(&rows, &[(0, 1_000), (5_000, 6_000)]);
        assert_eq!(switches, 0);
        assert_eq!(eligible_ms, 2_000);
    }

    #[test]
    fn medians_handle_odd_even_and_empty_samples() {
        assert_eq!(super::median_i64(vec![9, 1, 5]), Some(5));
        assert_eq!(super::median_i64(vec![9, 1, 5, 3]), Some(4));
        assert_eq!(super::median_i64(vec![]), None);
        assert_eq!(super::median_f64(vec![9.0, 1.0, 5.0]), Some(5.0));
        assert_eq!(super::median_f64(vec![9.0, 1.0, 5.0, 3.0]), Some(4.0));
        assert_eq!(super::median_f64(vec![]), None);
    }

    #[test]
    fn focus_report_serializes_only_processed_metrics() {
        let report = super::FocusSessionReport {
            session: super::FocusSessionSummary {
                id: Some(1),
                start: 10,
                end: 20,
                planned_ms: 10,
                completed: true,
                status: "completed".into(),
                recovered: false,
            },
            metrics: crate::reconstruct::FocusSessionMetrics {
                tracked_app_ms: 10,
                observed_ms: 10,
                coverage: 1.0,
                switch_count: 0,
                switches_per_observed_hour: Some(0.0),
                median_app_run_ms: Some(10),
            },
            baseline: super::FocusSessionBaseline {
                sample_count: 0,
                median_switches_per_observed_hour: None,
                median_app_run_ms: None,
            },
        };
        let value = serde_json::to_value(report).unwrap();
        assert!(value.get("metrics").is_some());
        for raw_field in ["observations", "appTotals", "appVisits", "transitions", "gaps"] {
            assert!(value.get(raw_field).is_none());
            assert!(value["metrics"].get(raw_field).is_none());
        }
        assert_eq!(value["session"]["driftMs"], 0);
        for raw_field in ["updatedAtMs", "recoveryGapMs"] {
            assert!(value["session"].get(raw_field).is_none());
        }
    }
}
