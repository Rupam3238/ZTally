use crate::db::SpanRow;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationSpan {
    pub start_ms: i64,
    pub end_ms: i64,
    pub app_key: String,
    pub display: String,
    pub title: String,
    pub domain: Option<String>,
    pub afk: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationVisit {
    pub app_key: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub duration_ms: i64,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionGap {
    pub start_ms: i64,
    pub end_ms: i64,
    pub ms: i64,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationTransition {
    pub from_app: String,
    pub to_app: String,
    pub at_ms: i64,
    pub uncertain: bool,
    pub gap_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReliabilityFinding {
    pub code: String,
    pub severity: String,
    pub summary: String,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub duration_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReliabilityAssessment {
    pub status: String,
    pub coverage: f64,
    pub observation_quality: f64,
    pub transition_uncertainty: f64,
    pub tracking_integrity: f64,
    pub data_integrity: f64,
    pub findings: Vec<ReliabilityFinding>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconstructedSession {
    pub session_start_ms: i64,
    pub session_end_ms: i64,
    pub observed_ms: i64,
    pub afk_ms: Option<i64>,
    pub ambiguous_ms: i64,
    pub system_ui_ms: i64,
    pub untracked_ms: i64,
    pub uncertain_ms: i64,
    pub recovery_gap_ms: i64,
    pub observations: Vec<ObservationSpan>,
    pub app_totals: HashMap<String, i64>,
    pub app_visits: Vec<ApplicationVisit>,
    pub transitions: Vec<ApplicationTransition>,
    pub gaps: Vec<SessionGap>,
    pub invalid_span_count: usize,
    pub dwell_time_ms: i64,
    pub median_dwell_ms: i64,
    pub average_dwell_ms: i64,
    pub short_visits: usize,
}

fn clip_interval(start: i64, end: i64, session_start: i64, session_end: i64) -> Option<(i64, i64)> {
    let left = start.max(session_start);
    let right = end.min(session_end);
    if right <= left {
        return None;
    }
    Some((left, right))
}

fn merged_intervals(intervals: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut sorted: Vec<_> = intervals.to_vec();
    sorted.sort_by_key(|(start, _)| *start);
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for (start, end) in sorted {
        if let Some((_m_start, m_end)) = merged.last_mut() {
            if start <= *m_end {
                *m_end = (*m_end).max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    merged
}

fn covered_duration(intervals: &[(i64, i64)]) -> i64 {
    merged_intervals(intervals)
        .into_iter()
        .map(|(s, e)| e.saturating_sub(s))
        .sum()
}

fn subtract_intervals(start: i64, end: i64, covered: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut segments = Vec::new();
    let mut cursor = start;
    for (covered_start, covered_end) in covered {
        if *covered_end <= cursor {
            continue;
        }
        if *covered_start >= end {
            break;
        }
        if *covered_start > cursor {
            segments.push((cursor, (*covered_start).min(end)));
        }
        cursor = cursor.max(*covered_end);
        if cursor >= end {
            break;
        }
    }
    if cursor < end {
        segments.push((cursor, end));
    }
    segments
}

fn is_desktop_shell_app_key(app_key: &str) -> bool {
    let key = app_key.trim();
    if key.is_empty() {
        return false;
    }
    let normalized = key.to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "plasmashell"
            | "plasmawindowed"
            | "krunner"
            | "kglobalaccel5"
            | "kded5"
            | "ksmserver"
            | "ksplashqml"
            | "org.kde.plasmashell"
            | "org.kde.plasmawindowed"
            | "org.kde.krunner"
            | "org.kde.kglobalaccel5"
            | "org.kde.kded5"
            | "org.kde.ksmserver"
            | "org.kde.ksplashqml"
            | "kwin"
            | "kwin_x11"
            | "kwin_wayland"
            | "org.kde.kwin"
    )
}

fn uncovered_segments(
    session_start: i64,
    session_end: i64,
    covered: &[(i64, i64)],
) -> Vec<SessionGap> {
    let mut out = Vec::new();
    let mut cursor = session_start;
    for (s, e) in merged_intervals(covered) {
        if s > cursor {
            out.push(SessionGap {
                start_ms: cursor,
                end_ms: s,
                ms: s - cursor,
                reason: "No observation".to_string(),
            });
        }
        cursor = cursor.max(e);
    }
    if cursor < session_end {
        out.push(SessionGap {
            start_ms: cursor,
            end_ms: session_end,
            ms: session_end - cursor,
            reason: "Untracked tail".to_string(),
        });
    }
    out
}

pub fn reconstruct_session(
    session_start_ms: i64,
    session_end_ms: i64,
    spans: &[SpanRow],
    recovery_gap_ms: i64,
) -> ReconstructedSession {
    let mut observations = Vec::new();
    let mut invalid_span_count = 0;
    for span in spans {
        if span.end <= span.start || (!span.afk && span.app_key.trim().is_empty()) {
            invalid_span_count += 1;
            continue;
        }
        let Some((start_ms, end_ms)) =
            clip_interval(span.start, span.end, session_start_ms, session_end_ms)
        else {
            invalid_span_count += 1;
            continue;
        };
        observations.push(ObservationSpan {
            start_ms,
            end_ms,
            app_key: span.app_key.clone(),
            display: span.display.clone(),
            title: span.title.clone(),
            domain: span.domain.clone(),
            afk: span.afk,
        });
    }
    observations.sort_by_key(|obs| obs.start_ms);

    let system_ui_covered = merged_intervals(
        &observations
            .iter()
            .filter(|obs| !obs.afk && is_desktop_shell_app_key(&obs.app_key))
            .map(|obs| (obs.start_ms, obs.end_ms))
            .collect::<Vec<_>>(),
    );
    let system_ui_ms = covered_duration(&system_ui_covered);

    let mut events = Vec::new();
    for observation in &observations {
        if !observation.afk && is_desktop_shell_app_key(&observation.app_key) {
            continue;
        }
        for (start_ms, end_ms) in
            subtract_intervals(observation.start_ms, observation.end_ms, &system_ui_covered)
        {
            events.push((start_ms, observation.app_key.clone(), observation.afk, 1i8));
            events.push((end_ms, observation.app_key.clone(), observation.afk, -1i8));
        }
    }
    events.sort_by_key(|event| (event.0, event.3));

    let mut per_app: HashMap<String, Vec<(i64, i64)>> = HashMap::new();
    let mut afk_intervals = Vec::new();
    let mut ambiguous_ms = 0;
    let mut active_apps: HashMap<String, usize> = HashMap::new();
    let mut active_afk = 0usize;
    let mut cursor = events.first().map_or(session_start_ms, |event| event.0);
    let mut event_index = 0;
    while event_index < events.len() {
        let at_ms = events[event_index].0;
        if at_ms > cursor {
            match (active_apps.len(), active_afk > 0) {
                (1, false) => {
                    let app_key = active_apps.keys().next().unwrap().clone();
                    per_app.entry(app_key).or_default().push((cursor, at_ms));
                }
                (0, true) => afk_intervals.push((cursor, at_ms)),
                (apps, afk) if apps > 1 || (apps > 0 && afk) => {
                    ambiguous_ms += at_ms - cursor;
                }
                _ => {}
            }
        }
        while event_index < events.len() && events[event_index].0 == at_ms {
            let (_, app_key, afk, delta) = &events[event_index];
            if *afk {
                if *delta > 0 {
                    active_afk += 1;
                } else {
                    active_afk -= 1;
                }
            } else if *delta > 0 {
                *active_apps.entry(app_key.clone()).or_default() += 1;
            } else if let Some(count) = active_apps.get_mut(app_key) {
                *count -= 1;
                if *count == 0 {
                    active_apps.remove(app_key);
                }
            }
            event_index += 1;
        }
        cursor = at_ms;
    }
    let afk_ms = observations.iter().any(|observation| observation.afk).then(|| covered_duration(&afk_intervals));

    let mut app_totals: HashMap<String, i64> = HashMap::new();
    let mut app_visits = Vec::new();
    let mut app_keys: Vec<String> = per_app.keys().cloned().collect();
    app_keys.sort();
    for app_key in app_keys {
        let intervals = merged_intervals(&per_app[&app_key]);
        let duration = intervals
            .iter()
            .map(|(s, e)| e.saturating_sub(*s))
            .sum::<i64>();
        app_totals.insert(app_key.clone(), duration);
        for (start_ms, end_ms) in intervals {
            app_visits.push(ApplicationVisit {
                app_key: app_key.clone(),
                start_ms,
                end_ms,
                duration_ms: end_ms.saturating_sub(start_ms),
                title: String::new(),
            });
        }
    }
    app_visits.sort_by_key(|v| (v.start_ms, v.app_key.clone()));

    let all_obs: Vec<(i64, i64)> = observations
        .iter()
        .map(|obs| (obs.start_ms, obs.end_ms))
        .collect();
    let observed_ms = covered_duration(&all_obs);

    let gaps = uncovered_segments(session_start_ms, session_end_ms, &all_obs);
    let untracked_ms = gaps.iter().map(|g| g.ms).sum();

    let mut transitions = Vec::new();
    let mut uncertain_ms = 0;
    let mut previous_app: Option<String> = None;
    let mut previous_end = session_start_ms;
    for visit in &app_visits {
        if let Some(prev) = previous_app.clone() {
            if prev != visit.app_key {
                let gap_start = previous_end.min(visit.start_ms);
                let gap_end = visit.start_ms.max(previous_end);
                let gap_ms = gaps
                    .iter()
                    .map(|gap| gap.end_ms.min(gap_end).saturating_sub(gap.start_ms.max(gap_start)).max(0))
                    .sum::<i64>();
                let uncertain = gap_ms > 0;
                if uncertain {
                    uncertain_ms += gap_ms;
                }
                transitions.push(ApplicationTransition {
                    from_app: prev,
                    to_app: visit.app_key.clone(),
                    at_ms: visit.start_ms,
                    uncertain,
                    gap_ms,
                });
            }
        }
        previous_app = Some(visit.app_key.clone());
        previous_end = visit.end_ms.max(previous_end);
    }

    let mut dwell_times: Vec<i64> = app_visits.iter().map(|v| v.duration_ms).collect();
    dwell_times.sort();
    let dwell_time_ms = dwell_times.iter().copied().sum::<i64>();
    let median_dwell_ms = if dwell_times.is_empty() {
        0
    } else if dwell_times.len() % 2 == 0 {
        let mid = dwell_times.len() / 2;
        (dwell_times[mid - 1] + dwell_times[mid]) / 2
    } else {
        dwell_times[dwell_times.len() / 2]
    };
    let average_dwell_ms = if dwell_times.is_empty() {
        0
    } else {
        dwell_time_ms / dwell_times.len() as i64
    };
    let short_visits = dwell_times.iter().filter(|&&d| d < 60_000).count();

    ReconstructedSession {
        session_start_ms,
        session_end_ms,
        observed_ms,
        afk_ms,
        ambiguous_ms,
        system_ui_ms,
        untracked_ms,
        uncertain_ms,
        recovery_gap_ms,
        observations,
        app_totals,
        app_visits,
        transitions,
        gaps,
        invalid_span_count,
        dwell_time_ms,
        median_dwell_ms,
        average_dwell_ms,
        short_visits,
    }
}

pub fn assess_reliability(reconstruction: &ReconstructedSession) -> ReliabilityAssessment {
    let session_length = (reconstruction.session_end_ms - reconstruction.session_start_ms).max(0);
    let effective_length = session_length
        .saturating_sub(reconstruction.recovery_gap_ms)
        .max(0);
    let coverage = if effective_length > 0 {
        (reconstruction.observed_ms as f64 / effective_length as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let transition_uncertainty = if effective_length > 0 {
        (reconstruction.uncertain_ms as f64 / effective_length as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let ordinary_untracked_ms = reconstruction.untracked_ms.saturating_sub(reconstruction.recovery_gap_ms);
    let gaps_ratio = if effective_length > 0 {
        (ordinary_untracked_ms as f64 / effective_length as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let observation_quality = (1.0
        - (gaps_ratio * 0.8
            + (reconstruction.recovery_gap_ms as f64 / effective_length.max(1) as f64) * 0.2))
        .clamp(0.0, 1.0);
    let ambiguous_ratio = if effective_length > 0 {
        (reconstruction.ambiguous_ms as f64 / effective_length as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let tracking_integrity = (1.0 - gaps_ratio * 0.25 - ambiguous_ratio * 0.5)
        .clamp(0.0, 1.0);
    let data_integrity = if reconstruction.invalid_span_count == 0 {
        1.0
    } else {
        (1.0 - (reconstruction.invalid_span_count as f64 * 0.2)).clamp(0.0, 1.0)
    };
    let score = coverage * 0.5
        + observation_quality * 0.2
        + tracking_integrity * 0.15
        + data_integrity * 0.15;

    let mut findings = Vec::new();
    if reconstruction.invalid_span_count > 0 {
        findings.push(ReliabilityFinding {
            code: "invalid_span".to_string(),
            severity: "error".to_string(),
            summary: format!("{} invalid spans were skipped because they had zero duration or crossed the session boundary without valid coverage.", reconstruction.invalid_span_count),
            start_ms: None,
            end_ms: None,
            duration_ms: None,
        });
    }
    if reconstruction.recovery_gap_ms > 0 {
        findings.push(ReliabilityFinding {
            code: "recovery_gap".to_string(),
            severity: "warning".to_string(),
            summary: "Recovery metadata indicates a tracked restart or resume gap that remains unassigned to either app.".to_string(),
            start_ms: Some(reconstruction.session_start_ms),
            end_ms: Some(reconstruction.session_end_ms),
            duration_ms: Some(reconstruction.recovery_gap_ms),
        });
    }
    if reconstruction.ambiguous_ms > 0 {
        findings.push(ReliabilityFinding {
            code: "overlapping_observations".to_string(),
            severity: "warning".to_string(),
            summary: format!(
                "{} ms had overlapping evidence from different apps or AFK and app observations; this time was not assigned to an app.",
                reconstruction.ambiguous_ms
            ),
            start_ms: None,
            end_ms: None,
            duration_ms: Some(reconstruction.ambiguous_ms),
        });
    }
    if reconstruction.untracked_ms > 0 {
        findings.push(ReliabilityFinding {
            code: "untracked_time".to_string(),
            severity: "warning".to_string(),
            summary: format!(
                "{} ms of the session remain unobserved and are not attributable to any app.",
                reconstruction.untracked_ms
            ),
            start_ms: reconstruction.gaps.first().map(|g| g.start_ms),
            end_ms: reconstruction.gaps.last().map(|g| g.end_ms),
            duration_ms: Some(reconstruction.untracked_ms),
        });
    }
    if transition_uncertainty > 0.05 {
        findings.push(ReliabilityFinding {
            code: "transition_uncertainty".to_string(),
            severity: "warning".to_string(),
            summary: "The historical reconstruction contains app-to-app transitions across missing evidence, so the exact user context is uncertain.".to_string(),
            start_ms: reconstruction.transitions.first().map(|t| t.at_ms),
            end_ms: reconstruction.transitions.last().map(|t| t.at_ms),
            duration_ms: Some(reconstruction.uncertain_ms),
        });
    }
    if effective_length > 0 && coverage < 0.85 {
        findings.push(ReliabilityFinding {
            code: "low_coverage".to_string(),
            severity: if coverage < 0.5 { "error".to_string() } else { "warning".to_string() },
            summary: "Coverage is below the threshold for a high-confidence reconstruction of the full session.".to_string(),
            start_ms: Some(reconstruction.session_start_ms),
            end_ms: Some(reconstruction.session_end_ms),
            duration_ms: Some((effective_length as f64 * (1.0 - coverage)) as i64),
        });
    }
    if reconstruction.observed_ms == 0 && reconstruction.invalid_span_count == 0 {
        findings.push(ReliabilityFinding {
            code: "missing_evidence".to_string(),
            severity: "warning".to_string(),
            summary: "No valid evidence exists for this session window, so the reconstruction cannot claim any observed activity.".to_string(),
            start_ms: Some(reconstruction.session_start_ms),
            end_ms: Some(reconstruction.session_end_ms),
            duration_ms: Some(0),
        });
    }

    let status = if effective_length == 0 {
        "unknown"
    } else if score >= 0.8 {
        "high-confidence"
    } else if score >= 0.55 {
        "mixed-confidence"
    } else if reconstruction.observed_ms == 0 && reconstruction.invalid_span_count == 0 {
        "unknown"
    } else {
        "low-confidence"
    };

    ReliabilityAssessment {
        status: status.to_string(),
        coverage,
        observation_quality,
        transition_uncertainty,
        tracking_integrity,
        data_integrity,
        findings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: i64, end: i64, app: &str, domain: Option<&str>, afk: bool) -> SpanRow {
        SpanRow {
            start,
            end,
            app_key: app.to_string(),
            display: app.to_string(),
            title: format!("{app}-title"),
            domain: domain.map(str::to_string),
            afk,
        }
    }

    #[test]
    fn consecutive_same_app_segments_merge_into_one_visit() {
        let rows = vec![
            span(0, 1_000, "chrome", None, false),
            span(1_000, 2_000, "chrome", None, false),
            span(2_000, 3_000, "chrome", None, false),
        ];
        let rec = reconstruct_session(0, 4_000, &rows, 0);
        assert_eq!(rec.app_visits.len(), 1);
        assert_eq!(rec.app_visits[0].duration_ms, 3_000);
        assert_eq!(rec.app_totals.get("chrome"), Some(&3_000));
    }

    #[test]
    fn different_apps_produce_one_transition() {
        let rows = vec![
            span(0, 2_000, "a", None, false),
            span(2_000, 5_000, "b", None, false),
        ];
        let rec = reconstruct_session(0, 5_000, &rows, 0);
        assert_eq!(rec.transitions.len(), 1);
        assert_eq!(rec.transitions[0].from_app, "a");
        assert_eq!(rec.transitions[0].to_app, "b");
        assert!(!rec.transitions[0].uncertain);
    }

    #[test]
    fn domain_changes_do_not_count_as_application_switches() {
        let rows = vec![
            span(0, 1_000, "browser", Some("docs"), false),
            span(1_000, 2_000, "browser", Some("mail"), false),
        ];
        let rec = reconstruct_session(0, 2_000, &rows, 0);
        assert_eq!(rec.app_visits.len(), 1);
        assert_eq!(rec.transitions.len(), 0);
    }

    #[test]
    fn observed_afk_between_apps_does_not_make_a_transition_uncertain() {
        let rows = vec![
            span(0, 1_000, "a", None, false),
            span(1_000, 3_000, "", None, true),
            span(3_000, 4_000, "b", None, false),
        ];
        let rec = reconstruct_session(0, 4_000, &rows, 0);
        assert_eq!(rec.untracked_ms, 0);
        assert_eq!(rec.transitions.len(), 1);
        assert!(!rec.transitions[0].uncertain);
        assert_eq!(rec.uncertain_ms, 0);
    }

    #[test]
    fn gaps_are_never_attributed_to_an_app() {
        let rows = vec![
            span(0, 2_000, "a", None, false),
            span(6_000, 8_000, "b", None, false),
        ];
        let rec = reconstruct_session(0, 10_000, &rows, 0);
        assert_eq!(rec.app_totals.get("a"), Some(&2_000));
        assert_eq!(rec.app_totals.get("b"), Some(&2_000));
        assert_eq!(rec.untracked_ms, 6_000);
    }

    #[test]
    fn overlapping_spans_do_not_double_count_time() {
        let rows = vec![
            span(0, 2_000, "a", None, false),
            span(1_000, 3_000, "a", None, false),
        ];
        let rec = reconstruct_session(0, 4_000, &rows, 0);
        assert_eq!(rec.observed_ms, 3_000);
        assert_eq!(rec.app_totals.get("a"), Some(&3_000));
    }

    #[test]
    fn overlapping_different_apps_are_observed_but_not_double_attributed() {
        let rows = vec![
            span(0, 4_000, "a", None, false),
            span(2_000, 6_000, "b", None, false),
        ];
        let rec = reconstruct_session(0, 6_000, &rows, 0);
        assert_eq!(rec.observed_ms, 6_000);
        assert_eq!(rec.ambiguous_ms, 2_000);
        assert_eq!(rec.dwell_time_ms, 4_000);
        assert_eq!(rec.app_totals.get("a"), Some(&2_000));
        assert_eq!(rec.app_totals.get("b"), Some(&2_000));
        assert!(assess_reliability(&rec)
            .findings
            .iter()
            .any(|finding| finding.code == "overlapping_observations"));
    }

    #[test]
    fn empty_app_key_is_valid_for_afk_spans() {
        let rows = vec![
            span(0, 2_000, "a", None, false),
            span(2_000, 5_000, "", None, true),
        ];
        let rec = reconstruct_session(0, 6_000, &rows, 0);
        assert_eq!(rec.invalid_span_count, 0);
        assert_eq!(rec.observed_ms, 5_000);
        assert_eq!(rec.afk_ms, Some(3_000));
        assert_eq!(rec.dwell_time_ms, 2_000);
    }

    #[test]
    fn missing_afk_spans_remain_distinct_from_zero_afk_duration() {
        let rec = reconstruct_session(0, 1_000, &[span(0, 1_000, "a", None, false)], 0);
        assert_eq!(rec.afk_ms, None);
    }

    #[test]
    fn zero_duration_and_invalid_spans_are_skipped_safely() {
        let rows = vec![
            span(10, 10, "bad", None, false),
            span(5, 1, "bad2", None, false),
            span(0, 1_000, "a", None, false),
        ];
        let rec = reconstruct_session(0, 2_000, &rows, 0);
        assert_eq!(rec.invalid_span_count, 2);
        assert_eq!(rec.observed_ms, 1_000);
    }

    #[test]
    fn spans_crossing_the_session_boundary_are_clipped() {
        let rows = vec![span(1_000, 6_000, "a", None, false)];
        let rec = reconstruct_session(2_000, 5_000, &rows, 0);
        assert_eq!(rec.observed_ms, 3_000);
        assert_eq!(rec.app_totals.get("a"), Some(&3_000));
    }

    #[test]
    fn recovery_gap_is_preserved_separately_from_ordinary_gaps() {
        let rows = vec![
            span(0, 2_000, "a", None, false),
            span(7_000, 8_000, "a", None, false),
        ];
        let rec = reconstruct_session(0, 10_000, &rows, 3_000);
        assert_eq!(rec.recovery_gap_ms, 3_000);
        assert_eq!(rec.app_totals.get("a"), Some(&3_000));
        assert_eq!(rec.untracked_ms, 7_000);
    }

    #[test]
    fn recovered_session_resumes_from_the_actual_recovery_observation() {
        let rows = vec![
            span(0, 1_000, "a", None, false),
            span(5_000, 7_000, "a", None, false),
        ];
        let rec = reconstruct_session(0, 8_000, &rows, 2_000);
        assert_eq!(rec.app_visits.len(), 2);
        assert_eq!(rec.app_visits[0].end_ms, 1_000);
        assert_eq!(rec.app_visits[1].start_ms, 5_000);
        assert_eq!(rec.recovery_gap_ms, 2_000);
    }

    #[test]
    fn dwell_metrics_are_computed_from_visit_lengths() {
        let rows = vec![
            span(0, 4_000, "a", None, false),
            span(9_000, 11_000, "b", None, false),
        ];
        let rec = reconstruct_session(0, 12_000, &rows, 0);
        assert_eq!(rec.dwell_time_ms, 6_000);
        assert_eq!(rec.median_dwell_ms, 3_000);
        assert_eq!(rec.average_dwell_ms, 3_000);
        assert_eq!(rec.short_visits, 2);
    }

    #[test]
    fn coverage_and_transition_uncertainty_are_independent() {
        let rows = vec![
            span(0, 2_000, "a", None, false),
            span(7_000, 9_000, "b", None, false),
        ];
        let rec = reconstruct_session(0, 10_000, &rows, 0);
        let report = assess_reliability(&rec);
        assert!(report.coverage > 0.0);
        assert!(report.transition_uncertainty > 0.0);
        assert!(report.coverage != report.transition_uncertainty);
    }

    #[test]
    fn missing_evidence_does_not_produce_fabricated_percentages() {
        let rec = reconstruct_session(0, 10_000, &[], 0);
        let report = assess_reliability(&rec);
        assert_eq!(report.coverage, 0.0);
        assert_eq!(report.transition_uncertainty, 0.0);
        assert_eq!(report.status, "unknown");
        assert!(report.findings.iter().any(|f| f.code == "missing_evidence"));
    }

    #[test]
    fn recovery_gap_is_not_penalized_again_as_ordinary_untracked_time() {
        let rows = vec![
            span(0, 2_000, "a", None, false),
            span(7_000, 8_000, "a", None, false),
        ];
        let rec = reconstruct_session(0, 10_000, &rows, 3_000);
        let report = assess_reliability(&rec);
        assert!(report.observation_quality > 0.4);
    }

    #[test]
    fn plasma_shell_time_is_excluded_from_app_visits_and_transitions() {
        let rows = vec![
            span(0, 2_000, "firefox", None, false),
            span(2_000, 4_000, "plasmashell", None, false),
            span(4_000, 6_000, "code", None, false),
        ];
        let rec = reconstruct_session(0, 6_000, &rows, 0);
        assert_eq!(rec.app_visits.len(), 2);
        assert_eq!(rec.app_visits[0].app_key, "firefox");
        assert_eq!(rec.app_visits[1].app_key, "code");
        assert_eq!(rec.transitions.len(), 1);
        assert_eq!(rec.transitions[0].from_app, "firefox");
        assert_eq!(rec.transitions[0].to_app, "code");
        assert_eq!(rec.system_ui_ms, 2_000);
        assert_eq!(rec.app_totals.get("firefox"), Some(&2_000));
        assert_eq!(rec.app_totals.get("code"), Some(&2_000));
    }

    #[test]
    fn consecutive_plasma_shell_observations_do_not_create_app_visits() {
        let rows = vec![
            span(0, 1_000, "firefox", None, false),
            span(1_000, 3_000, "plasmashell", None, false),
            span(3_000, 5_000, "plasmashell", None, false),
            span(5_000, 7_000, "code", None, false),
        ];
        let rec = reconstruct_session(0, 7_000, &rows, 0);
        assert_eq!(rec.app_visits.len(), 2);
        assert_eq!(rec.system_ui_ms, 4_000);
        assert_eq!(rec.transitions.len(), 1);
        assert_eq!(rec.untracked_ms, 0);
    }

    #[test]
    fn overlapping_plasma_shell_masks_app_dwell_without_changing_coverage() {
        let rows = vec![
            span(0, 6_000, "firefox", None, false),
            span(2_000, 4_000, "plasmashell", None, false),
        ];
        let rec = reconstruct_session(0, 6_000, &rows, 0);
        assert_eq!(rec.system_ui_ms, 2_000);
        assert_eq!(rec.app_totals.get("firefox"), Some(&4_000));
        assert_eq!(rec.dwell_time_ms, 4_000);
        assert_eq!(rec.app_visits.len(), 2);
        assert_eq!(
            (rec.app_visits[0].start_ms, rec.app_visits[0].end_ms),
            (0, 2_000)
        );
        assert_eq!(
            (rec.app_visits[1].start_ms, rec.app_visits[1].end_ms),
            (4_000, 6_000)
        );
        assert_eq!(rec.observed_ms, 6_000);
        assert_eq!(rec.untracked_ms, 0);
        assert!(rec.transitions.is_empty());
    }

    #[test]
    fn shell_observations_at_session_boundaries_are_clipped_and_not_app_dwell() {
        let rows = vec![
            span(0, 2_500, "plasmashell", None, false),
            span(1_000, 6_000, "firefox", None, false),
            span(4_500, 7_000, "plasmashell", None, false),
        ];
        let rec = reconstruct_session(1_000, 6_000, &rows, 0);
        assert_eq!(rec.system_ui_ms, 3_000);
        assert_eq!(rec.app_totals.get("firefox"), Some(&2_000));
        assert_eq!(rec.dwell_time_ms, 2_000);
        assert_eq!(rec.app_visits.len(), 1);
        assert_eq!(
            (rec.app_visits[0].start_ms, rec.app_visits[0].end_ms),
            (2_500, 4_500)
        );
        assert_eq!(rec.observed_ms, 5_000);
        assert_eq!(rec.untracked_ms, 0);
    }

    #[test]
    fn shell_time_is_not_classified_as_untracked_or_missing_evidence() {
        let rows = vec![
            span(0, 2_000, "firefox", None, false),
            span(2_000, 4_000, "plasmashell", None, false),
            span(4_000, 6_000, "firefox", None, false),
        ];
        let rec = reconstruct_session(0, 6_000, &rows, 0);
        assert_eq!(rec.untracked_ms, 0);
        assert_eq!(rec.system_ui_ms, 2_000);
        assert_eq!(rec.observed_ms, 6_000);
        assert!(assess_reliability(&rec).findings.iter().all(|f| f.code != "untracked_time"));
    }

    #[test]
    fn ordinary_kde_apps_remain_included() {
        let rows = vec![
            span(0, 2_000, "dolphin", None, false),
            span(2_000, 4_000, "plasmashell", None, false),
            span(4_000, 6_000, "kate", None, false),
        ];
        let rec = reconstruct_session(0, 6_000, &rows, 0);
        assert_eq!(rec.app_totals.get("dolphin"), Some(&2_000));
        assert_eq!(rec.app_totals.get("kate"), Some(&2_000));
        assert_eq!(rec.system_ui_ms, 2_000);
    }
}
