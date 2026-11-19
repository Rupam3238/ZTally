

use super::{exe_for, ActiveWindowSource, Backend, BackendInfo, IdleSource, LogindIdle};
use crate::model::RawWindow;
use parking_lot::Mutex;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use zbus::blocking::{connection, Connection, Proxy};

const SERVICE: &str = "eu.zsync.ZTally";
const PATH: &str = "/eu/zsync/ZTally";
const PLUGIN: &str = "ztally-focus";
const SCRIPT: &str = include_str!("../../resources/kwin-script/ztally.js");

/// The KWin helper only pushes focus on activation/title events, so a missed
/// event would otherwise leave a stale window "focused" forever. To bound that,
/// the helper is reloaded periodically (its last step re-pushes the real
/// focused window) and cached windows whose process has exited are dropped.
const RESYNC_EVERY: Duration = Duration::from_secs(30);
/// Never reload more often than this, even if a dead window keeps coming back.
const MIN_RESYNC_GAP: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq)]
struct Cached {
    win: RawWindow,
    /// Whether the window's pid existed in our /proc when it was reported.
    /// Sandboxed apps can report a pid we cannot see; those are never "dead".
    pid_was_alive: bool,
}

#[derive(Default)]
struct State {
    cached: Option<Cached>,
    /// Set when a resync was requested; the next report is compared with the
    /// cached window so a corrected stale focus gets logged.
    awaiting_resync: bool,
}

type Shared = Arc<Mutex<State>>;

fn pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// A cached window is stale when its process was running when reported but has since exited.
fn is_stale(c: &Cached, alive: impl Fn(u32) -> bool) -> bool {
    c.pid_was_alive && c.win.pid.is_some_and(|p| !alive(p))
}

/// Deliberately omits the title: titles are private by design.
fn describe(c: Option<&Cached>) -> String {
    match c {
        None => "no window".into(),
        Some(c) => format!(
            "{} (pid {})",
            c.win.class.as_deref().or(c.win.exe.as_deref()).unwrap_or("unknown"),
            c.win.pid.map_or_else(|| "?".to_string(), |p| p.to_string())
        ),
    }
}

struct Receiver(Shared);

#[zbus::interface(name = "eu.zsync.ZTally.KWin")]
impl Receiver {
    fn update(&self, class: String, caption: String, pid: i32) {
        let new = if class.is_empty() && caption.is_empty() {
            None
        } else {
            let pid = u32::try_from(pid).ok().filter(|p| *p > 0);
            let class = Some(class).filter(|c| !c.is_empty());
            let exe = exe_for(pid, class.as_deref());
            Some(Cached { pid_was_alive: pid.is_some_and(pid_alive), win: RawWindow { title: caption, exe, class, pid } })
        };
        let mut st = self.0.lock();
        if std::mem::take(&mut st.awaiting_resync) {
            let differs = st.cached.as_ref().map(|c| (&c.win.class, c.win.pid)) != new.as_ref().map(|c| (&c.win.class, c.win.pid));
            if differs {
                eprintln!("[ztally] kwin resync corrected focus: was {}, now {}", describe(st.cached.as_ref()), describe(new.as_ref()));
            }
        }
        st.cached = new;
    }
}

pub struct KWinWindow {
    shared: Shared,
    conn: Connection,
    last_resync: Instant,
    resync_busy: Arc<AtomicBool>,
}

impl KWinWindow {
    /// Reload the helper on a background thread (D-Bus calls can block).
    fn resync(&mut self) {
        self.last_resync = Instant::now();
        if self.resync_busy.swap(true, Ordering::SeqCst) {
            return;
        }
        self.shared.lock().awaiting_resync = true;
        let conn = self.conn.clone();
        let busy = self.resync_busy.clone();
        std::thread::spawn(move || {
            if let Err(e) = load_script(&conn) {
                eprintln!("[ztally] kwin resync failed: {e}");
            }
            busy.store(false, Ordering::SeqCst);
        });
    }
}

impl ActiveWindowSource for KWinWindow {
    fn sample(&mut self) -> Option<RawWindow> {
        let stale = self.shared.lock().cached.as_ref().is_some_and(|c| is_stale(c, pid_alive));
        let since = self.last_resync.elapsed();
        if since >= RESYNC_EVERY || (stale && since >= MIN_RESYNC_GAP) {
            self.resync();
        }
        let mut st = self.shared.lock();
        if st.cached.as_ref().is_some_and(|c| is_stale(c, pid_alive)) {
            eprintln!("[ztally] kwin: dropping {}, its process has exited", describe(st.cached.as_ref()));
            st.cached = None;
        }
        st.cached.as_ref().map(|c| c.win.clone())
    }
}

fn load_script(conn: &Connection) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("ztally-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join("ztally-kwin.js");
    std::fs::write(&file, SCRIPT).map_err(|e| e.to_string())?;

    let scripting = Proxy::new(conn, "org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting").map_err(|e| e.to_string())?;
    let _: Result<bool, _> = scripting.call("unloadScript", &(PLUGIN,));
    let id: i32 = scripting.call("loadScript", &(file.to_string_lossy().as_ref(), PLUGIN)).map_err(|e| e.to_string())?;
    if id < 0 {
        return Err("KWin refused to load the script".into());
    }

    for path in [format!("/Scripting/Script{id}"), format!("/{id}")] {
        if let Ok(p) = Proxy::new(conn, "org.kde.KWin", path.as_str(), "org.kde.kwin.Script") {
            if p.call::<_, _, ()>("run", &()).is_ok() {
                return Ok(());
            }
        }
    }

    scripting.call::<_, _, ()>("start", &()).map_err(|e| e.to_string())
}

struct KdeIdle {
    conn: Connection,
    fallback: LogindIdle,
}

fn screen_saver_idle_ms(idle_seconds: u32) -> Option<u64> {
    (idle_seconds > 0).then(|| u64::from(idle_seconds).saturating_mul(1_000))
}

impl IdleSource for KdeIdle {
    fn idle_ms(&mut self) -> Option<u64> {

        let r: Option<u32> = Proxy::new(&self.conn, "org.freedesktop.ScreenSaver", "/ScreenSaver", "org.freedesktop.ScreenSaver")
            .ok()
            .and_then(|p| p.call("GetSessionIdleTime", &()).ok());
        r.and_then(screen_saver_idle_ms)
            .or_else(|| self.fallback.idle_ms())
    }
}

pub fn backend() -> Backend {
    let shared: Shared = Default::default();
    let conn = connection::Builder::session()
        .and_then(|b| b.allow_name_replacements(false).replace_existing_names(false).name(SERVICE))
        .and_then(|b| b.serve_at(PATH, Receiver(shared.clone())))
        .and_then(|b| b.build());
    let conn = match conn {
        Ok(c) => c,
        Err(e) => return Backend::none("KDE Plasma", &format!("Could not register on the session bus: {e}")),
    };
    let loaded = load_script(&conn);
    let idle_conn = Connection::session().ok();
    let idle_supported = idle_conn.is_some();
    let idle: Box<dyn IdleSource> = match idle_conn {
        Some(c) => Box::new(KdeIdle { conn: c, fallback: LogindIdle::new() }),
        None => Box::new(LogindIdle::new()),
    };
    Backend {
        window: Box::new(KWinWindow { shared, conn, last_resync: Instant::now(), resync_busy: Default::default() }),
        idle,
        info: BackendInfo {
            id: "kwin",
            label: "KDE Plasma (KWin script)".into(),
            ok: loaded.is_ok(),
            idle_supported,
            note: loaded.err().map(|e| format!("Could not load the KWin helper script: {e}")),
            fix: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screensaver_idle_seconds_convert_to_milliseconds() {
        assert_eq!(screen_saver_idle_ms(180), Some(180_000));
        assert_eq!(screen_saver_idle_ms(0), None);
    }

    fn cached(pid: Option<u32>, was_alive: bool) -> Cached {
        Cached { win: RawWindow { title: "t".into(), exe: None, class: Some("charta".into()), pid }, pid_was_alive: was_alive }
    }

    #[test]
    fn exited_process_is_stale() {
        assert!(is_stale(&cached(Some(4242), true), |_| false));
    }

    #[test]
    fn running_process_is_not_stale() {
        assert!(!is_stale(&cached(Some(4242), true), |_| true));
    }

    #[test]
    fn pid_we_never_saw_is_not_stale() {
        // sandboxed apps may report a pid that does not exist in our /proc
        assert!(!is_stale(&cached(Some(4242), false), |_| false));
    }

    #[test]
    fn window_without_pid_is_not_stale() {
        assert!(!is_stale(&cached(None, false), |_| false));
    }

    #[test]
    fn pid_alive_checks_proc() {
        assert!(pid_alive(std::process::id()));
        assert!(!pid_alive(u32::MAX));
    }

    #[test]
    fn describe_omits_title() {
        let d = describe(Some(&cached(Some(7), true)));
        assert_eq!(d, "charta (pid 7)");
        assert_eq!(describe(None), "no window");
    }

    #[test]
    fn killed_app_is_detected_through_the_receiver() {
        let shared: Shared = Default::default();
        let rx = Receiver(shared.clone());
        let mut child = std::process::Command::new("sleep").arg("60").spawn().unwrap();
        rx.update("charta".into(), "Charta".into(), child.id() as i32);

        let c = shared.lock().cached.clone().unwrap();
        assert!(c.pid_was_alive);
        assert!(!is_stale(&c, pid_alive), "still running");

        child.kill().unwrap();
        child.wait().unwrap();
        assert!(is_stale(&c, pid_alive), "exited process must be stale");
    }

    #[test]
    fn empty_report_clears_the_cache_and_resync_flag() {
        let shared: Shared = Default::default();
        let rx = Receiver(shared.clone());
        rx.update("charta".into(), "Charta".into(), std::process::id() as i32);
        assert!(shared.lock().cached.is_some());

        shared.lock().awaiting_resync = true;
        rx.update(String::new(), String::new(), 0);
        let st = shared.lock();
        assert!(st.cached.is_none());
        assert!(!st.awaiting_resync);
    }
}
