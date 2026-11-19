

use super::{exe_for, ActiveWindowSource, Backend, BackendInfo, IdleSource, LogindIdle};
use crate::model::RawWindow;
use parking_lot::Mutex;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection as WaylandConnection, Dispatch, Proxy as WaylandProxy, QueueHandle};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1, ext_idle_notifier_v1,
};
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
    wayland: Option<WaylandIdle>,
    conn: Option<Connection>,
    fallback: LogindIdle,
}

fn screen_saver_idle_ms(idle_seconds: u32) -> Option<u64> {
    Some(u64::from(idle_seconds).saturating_mul(1_000))
}

fn screen_saver_idle(conn: &Connection) -> Option<u64> {
    Proxy::new(conn, "org.freedesktop.ScreenSaver", "/ScreenSaver", "org.freedesktop.ScreenSaver")
        .ok()
        .and_then(|proxy| proxy.call::<_, _, u32>("GetSessionIdleTime", &()).ok())
        .and_then(screen_saver_idle_ms)
}

#[derive(Default)]
struct WaylandIdleState {
    idled_at: Option<Instant>,
}

impl WaylandIdleState {
    fn idled(&mut self, now: Instant) {
        self.idled_at = Some(now);
    }

    fn resumed(&mut self) {
        self.idled_at = None;
    }

    fn idle_ms(&self, threshold_ms: u64, now: Instant) -> u64 {
        self.idled_at.map_or(0, |idled_at| {
            let elapsed = u64::try_from(now.saturating_duration_since(idled_at).as_millis())
                .unwrap_or(u64::MAX);
            threshold_ms.saturating_add(elapsed)
        })
    }
}

type WaylandIdleShared = Arc<Mutex<WaylandIdleState>>;
type IdleNotification = ext_idle_notification_v1::ExtIdleNotificationV1;
type IdleNotifier = ext_idle_notifier_v1::ExtIdleNotifierV1;

struct WaylandDispatch {
    notification: Option<IdleNotification>,
    idle: WaylandIdleShared,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for WaylandDispatch {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &WaylandConnection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<IdleNotification, ()> for WaylandDispatch {
    fn event(
        state: &mut Self,
        proxy: &IdleNotification,
        event: ext_idle_notification_v1::Event,
        _data: &(),
        _conn: &WaylandConnection,
        _qh: &QueueHandle<Self>,
    ) {
        if !state.notification.as_ref().is_some_and(|active| active == proxy) {
            return;
        }
        let mut idle = state.idle.lock();
        match event {
            ext_idle_notification_v1::Event::Idled => idle.idled(Instant::now()),
            ext_idle_notification_v1::Event::Resumed => idle.resumed(),
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(WaylandDispatch: ignore IdleNotifier);
wayland_client::delegate_noop!(WaylandDispatch: ignore wl_seat::WlSeat);

fn create_idle_notification(
    notifier: &IdleNotifier,
    seat: &wl_seat::WlSeat,
    qh: &QueueHandle<WaylandDispatch>,
    threshold_ms: u64,
) -> IdleNotification {
    let timeout = threshold_ms.clamp(1, u64::from(u32::MAX)) as u32;
    if notifier.version() >= 2 {
        notifier.get_input_idle_notification(timeout, seat, qh, ())
    } else {
        notifier.get_idle_notification(timeout, seat, qh, ())
    }
}

struct WaylandIdle {
    idle: WaylandIdleShared,
    threshold_ms: Arc<AtomicU64>,
    alive: Arc<AtomicBool>,
}

impl WaylandIdle {
    fn start(initial_threshold_ms: u64) -> Result<Self, String> {
        if std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return Err("WAYLAND_DISPLAY is not set".into());
        }

        let idle = Arc::new(Mutex::new(WaylandIdleState::default()));
        let threshold_ms = Arc::new(AtomicU64::new(initial_threshold_ms));
        let alive = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker_idle = idle.clone();
        let worker_threshold = threshold_ms.clone();
        let worker_alive = alive.clone();

        std::thread::spawn(move || {
            let setup = (|| -> Result<_, String> {
                let conn = WaylandConnection::connect_to_env().map_err(|e| e.to_string())?;
                let (globals, queue) = registry_queue_init::<WaylandDispatch>(&conn)
                    .map_err(|e| e.to_string())?;
                let qh = queue.handle();
                let notifier = globals.bind::<IdleNotifier, _, _>(&qh, 1..=2, ())
                    .map_err(|e| e.to_string())?;
                let seat = globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=1, ())
                    .map_err(|e| e.to_string())?;
                let threshold = worker_threshold.load(Ordering::Relaxed);
                let notification = create_idle_notification(&notifier, &seat, &qh, threshold);
                let dispatch = WaylandDispatch {
                    notification: Some(notification),
                    idle: worker_idle.clone(),
                };
                queue.flush().map_err(|e| e.to_string())?;
                Ok((conn, queue, qh, notifier, seat, dispatch, threshold))
            })();

            let (_conn, mut queue, qh, notifier, seat, mut dispatch, mut configured_threshold) =
                match setup {
                    Ok(setup) => setup,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };

            worker_alive.store(true, Ordering::Release);
            if ready_tx.send(Ok(())).is_err() {
                worker_alive.store(false, Ordering::Release);
                return;
            }

            use std::os::fd::AsRawFd;
            loop {
                if queue.dispatch_pending(&mut dispatch).is_err() {
                    break;
                }

                let next_threshold = worker_threshold.load(Ordering::Relaxed);
                if next_threshold != configured_threshold {
                    if let Some(old) = dispatch.notification.take() {
                        old.destroy();
                    }
                    *worker_idle.lock() = WaylandIdleState::default();
                    dispatch.notification = Some(create_idle_notification(
                        &notifier,
                        &seat,
                        &qh,
                        next_threshold,
                    ));
                    configured_threshold = next_threshold;
                    if queue.flush().is_err() {
                        break;
                    }
                }

                let Some(read_guard) = queue.prepare_read() else {
                    continue;
                };
                let mut fd = libc::pollfd {
                    fd: read_guard.connection_fd().as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                let ready = unsafe { libc::poll(&mut fd, 1, 250) };
                if ready < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }
                if fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                    break;
                }
                if ready > 0 && fd.revents & libc::POLLIN != 0 && read_guard.read().is_err() {
                    break;
                }
            }
            eprintln!("[ztally] Wayland idle notification connection stopped; using D-Bus/logind fallback.");
            notifier.destroy();
            worker_alive.store(false, Ordering::Release);
        });

        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(Self { idle, threshold_ms, alive }),
            Ok(Err(error)) => Err(error),
            Err(error) => Err(format!("Wayland idle notifier did not start: {error}")),
        }
    }
}

impl IdleSource for KdeIdle {
    fn idle_ms(&mut self) -> Option<u64> {
        if let Some(wayland) = &self.wayland {
            if wayland.alive.load(Ordering::Acquire) {
                return Some(wayland.idle.lock().idle_ms(
                    wayland.threshold_ms.load(Ordering::Relaxed),
                    Instant::now(),
                ));
            }
        }
        self.conn.as_ref().and_then(screen_saver_idle).or_else(|| self.fallback.idle_ms())
    }

    fn set_threshold_ms(&mut self, threshold_ms: u64) {
        if let Some(wayland) = &self.wayland {
            wayland.threshold_ms.store(threshold_ms, Ordering::Relaxed);
        }
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
    let wayland = match WaylandIdle::start(180_000) {
        Ok(idle) => Some(idle),
        Err(error) if std::env::var_os("WAYLAND_DISPLAY").is_some() => {
            eprintln!("[ztally] Wayland idle detection unavailable: {error}");
            None
        }
        Err(_) => None,
    };
    let screensaver_supported = idle_conn.as_ref().and_then(screen_saver_idle).is_some();
    let mut fallback = LogindIdle::new();
    let logind_supported = fallback.idle_ms().is_some();
    let idle_supported = wayland.is_some() || screensaver_supported || logind_supported;
    let mut notes = Vec::new();
    if let Err(error) = &loaded {
        notes.push(format!("Could not load the KWin helper script: {error}"));
    }
    if !idle_supported {
        notes.push("Idle detection is unavailable; Wayland, ScreenSaver D-Bus and logind sources could not be initialized.".into());
    }
    Backend {
        window: Box::new(KWinWindow { shared, conn, last_resync: Instant::now(), resync_busy: Default::default() }),
        idle: Box::new(KdeIdle { wayland, conn: idle_conn, fallback }),
        info: BackendInfo {
            id: "kwin",
            label: "KDE Plasma (KWin script)".into(),
            ok: loaded.is_ok(),
            idle_supported,
            note: (!notes.is_empty()).then(|| notes.join(" ")),
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
        assert_eq!(screen_saver_idle_ms(0), Some(0));
    }

    #[test]
    fn wayland_idle_state_reports_threshold_plus_time_since_idled() {
        let now = Instant::now();
        let mut state = WaylandIdleState::default();
        state.idled(now - Duration::from_secs(5));
        assert_eq!(state.idle_ms(30_000, now), 35_000);
    }

    #[test]
    fn wayland_idle_state_resets_on_resume() {
        let mut state = WaylandIdleState::default();
        state.idled(Instant::now());
        state.resumed();
        assert_eq!(state.idle_ms(30_000, Instant::now()), 0);
    }

    #[test]
    fn wayland_idle_state_uses_updated_threshold() {
        let now = Instant::now();
        let mut state = WaylandIdleState::default();
        state.idled(now - Duration::from_secs(5));
        assert_eq!(state.idle_ms(30_000, now), 35_000);
        assert_eq!(state.idle_ms(60_000, now), 65_000);
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
