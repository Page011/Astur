//! Power mode (the documented effective-power-mode notification to read it,
//! `power_mode` to change it) and the session actions, which go through
//! `PowerActions` so a test can never lock, sleep, restart, shut down or
//! sign out the PC.

use std::ffi::c_void;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::Duration;

use rsb_core::services::{PowerCommand, PowerMode, ServiceCommand, ServiceId, ServiceStatus};
use rsb_core::value::Value;
use windows::Win32::Foundation::{ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE, LUID, WAIT_OBJECT_0};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Power::{
    EFFECTIVE_POWER_MODE, EFFECTIVE_POWER_MODE_V2, GetPwrCapabilities, PowerRegisterForEffectivePowerModeNotifications,
    PowerUnregisterFromEffectivePowerModeNotifications, SYSTEM_POWER_CAPABILITIES, SetSuspendState,
};
use windows::Win32::System::Shutdown::{
    EWX_LOGOFF, EWX_POWEROFF, EWX_REBOOT, EWX_SHUTDOWN, ExitWindowsEx, LockWorkStation, SHTDN_REASON_FLAG_PLANNED,
    SHTDN_REASON_MAJOR_OTHER, SHUTDOWN_REASON,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken, SetEvent, WaitForMultipleObjects};

use super::{Worker, power_mode};
use crate::util::{map, text};
use crate::win::{Owned, auto_event};
use crate::{PowerActions, Publisher, Service};

const REREAD: Duration = Duration::from_secs(60);
/// The first notification arrives as the registration is made; wait this long for it before reading the mode
/// another way.
const FIRST_NOTIFICATION: Duration = Duration::from_millis(300);
/// The variable the binary's test runs set; the real actions refuse under it.
pub(crate) const TEST_TAG_VAR: &str = "RSB_TEST_TAG";

/// The effective power mode as it last arrived, and the event that says so.
/// Process-wide and never freed: the system calls the callback from its own
/// thread, and it may still be running as a registration is removed.
struct Notifier {
    mode: AtomicI32,
    wake: Owned,
}

const NO_MODE: i32 = -1;

fn notifier() -> Result<&'static Notifier, String> {
    static NOTIFIER: OnceLock<Result<Notifier, String>> = OnceLock::new();
    NOTIFIER
        .get_or_init(|| Ok(Notifier { mode: AtomicI32::new(NO_MODE), wake: auto_event()? }))
        .as_ref()
        .map_err(Clone::clone)
}

unsafe extern "system" fn on_mode(mode: EFFECTIVE_POWER_MODE, _context: *const c_void) {
    if let Ok(n) = notifier() {
        n.mode.store(mode.0, Ordering::Release);
        // SAFETY: `wake` is a live event that lives for the whole process.
        let _ = unsafe { SetEvent(n.wake.get()) };
    }
}

/// The three modes from Windows' effective power mode: the battery
/// saver and better-battery levels are power efficiency, the performance
/// levels (and game mode) are performance.
pub(crate) fn mode_name(effective: i32) -> &'static str {
    match effective {
        0 | 1 => "efficiency",
        2 => "balanced",
        3..=6 => "performance",
        _ => "unknown",
    }
}

/// Whether the machine can sleep: any sleep state, or modern standby.
fn can_sleep() -> bool {
    let mut caps = SYSTEM_POWER_CAPABILITIES::default();
    // SAFETY: a live out pointer of the right type.
    let read = unsafe { GetPwrCapabilities(&mut caps) };
    read && (caps.SystemS1 || caps.SystemS2 || caps.SystemS3 || caps.AoAc)
}

fn value(mode: &str, mode_available: bool, can_sleep: bool) -> Value {
    map([("mode", text(mode)), ("mode_available", Value::Bool(mode_available)), ("can_sleep", Value::Bool(can_sleep))])
}

fn run(out: Publisher, stop: Arc<Owned>) {
    let n = match notifier() {
        Ok(n) => n,
        Err(e) => {
            out.status(ServiceStatus::Error, Some(e));
            return;
        }
    };
    n.mode.store(NO_MODE, Ordering::Release);
    let mut handle: *mut c_void = std::ptr::null_mut();
    // SAFETY: the callback and its (absent) context stay valid for the process; `handle` receives the registration.
    let registered = unsafe {
        PowerRegisterForEffectivePowerModeNotifications(EFFECTIVE_POWER_MODE_V2, Some(on_mode), None, &mut handle)
    }
    .is_ok();
    let mode_available = power_mode::available();
    let can_sleep = can_sleep();
    if registered {
        // SAFETY: both handles are live events.
        let _ = unsafe {
            WaitForMultipleObjects(&[stop.get(), n.wake.get()], false, FIRST_NOTIFICATION.as_millis() as u32)
        };
    }
    loop {
        let effective = n.mode.load(Ordering::Acquire);
        let mode = if effective != NO_MODE {
            mode_name(effective)
        } else {
            power_mode::current().map_or("unknown", PowerMode::name)
        };
        out.publish(value(mode, mode_available, can_sleep));
        crate::count_source_wake();
        // SAFETY: both handles are live events.
        let waited = unsafe { WaitForMultipleObjects(&[stop.get(), n.wake.get()], false, REREAD.as_millis() as u32) };
        if waited == WAIT_OBJECT_0 {
            break;
        }
    }
    if registered {
        // SAFETY: the handle came from the registration above and is released once.
        let _ = unsafe { PowerUnregisterFromEffectivePowerModeNotifications(handle) };
    }
}

pub(crate) struct PowerService {
    actions: Option<Arc<dyn PowerActions>>,
    worker: Option<Worker>,
    /// Set while the service runs: where an action reports a failure, and the thread that runs them.
    running: Option<(Publisher, ActionThread)>,
}

impl PowerService {
    pub(crate) fn new(actions: Option<Arc<dyn PowerActions>>) -> PowerService {
        PowerService { actions, worker: None, running: None }
    }
}

type Job = Box<dyn FnOnce() + Send>;

/// One thread that runs the actions in the order they were asked for (two mode changes in a row
/// must end on the second). `run` only queues and returns at once: the hub thread serves every
/// service and must never wait on an action, and sleeping returns only when the PC wakes. The thread
/// ends when this is dropped, after the action it is in.
struct ActionThread {
    queue: mpsc::Sender<Job>,
}

impl ActionThread {
    fn start() -> Result<ActionThread, String> {
        let (queue, jobs) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("rsb-power-action".into())
            .spawn(move || jobs.into_iter().for_each(|job| job()))
            .map_err(|e| format!("could not start the action thread: {e}"))?;
        Ok(ActionThread { queue })
    }

    /// Queues `job`; a failure, however late, goes to `fail`.
    fn run(
        &self,
        job: impl FnOnce() -> Result<(), String> + Send + 'static,
        fail: impl FnOnce(String) + Send + 'static,
    ) -> Result<(), String> {
        let job: Job = Box::new(move || {
            if let Err(e) = job() {
                fail(e);
            }
        });
        self.queue.send(job).map_err(|_| "the action thread has stopped".to_string())
    }
}

impl Service for PowerService {
    fn id(&self) -> ServiceId {
        ServiceId::Power
    }

    fn start(&mut self, out: Publisher) -> Result<(), String> {
        self.running = Some((out.clone(), ActionThread::start()?));
        self.worker = Some(Worker::spawn("rsb-power", move |stop| run(out, stop))?);
        Ok(())
    }

    fn command(&mut self, command: ServiceCommand) -> Result<(), String> {
        let (ServiceCommand::Power(c), Some((out, thread))) = (command, &self.running) else {
            return Err("not a power command, or the service is stopped".into());
        };
        let out = out.clone();
        let actions: Arc<dyn PowerActions> = self.actions.clone().unwrap_or_else(|| Arc::new(WindowsPowerActions));
        thread.run(
            move || match c {
                PowerCommand::SetMode(mode) => power_mode::set_mode(mode),
                PowerCommand::Lock => actions.lock(),
                PowerCommand::Sleep => actions.sleep(),
                PowerCommand::Restart => actions.restart(),
                PowerCommand::ShutDown => actions.shut_down(),
                PowerCommand::SignOut => actions.sign_out(),
            },
            // The reason names nothing of this PC. The next published value clears the error.
            move |reason| out.status(ServiceStatus::Error, Some(reason)),
        )
    }

    fn stop(&mut self) {
        self.running = None;
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
    }

    fn stale_after(&self) -> Duration {
        Duration::from_secs(180)
    }
}

/// The five session actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Lock,
    Sleep,
    Restart,
    ShutDown,
    SignOut,
}

/// Runs `exec` for `action` unless the process runs under a test tag: the
/// last of the defences that keep a test from ending the session.
pub(crate) fn guarded(
    action: Action,
    tag_set: bool,
    exec: &dyn Fn(Action) -> Result<(), String>,
) -> Result<(), String> {
    if tag_set {
        return Err("refused: this process runs under a test tag".into());
    }
    exec(action)
}

fn tag_set() -> bool {
    std::env::var_os(TEST_TAG_VAR).is_some()
}

/// Enables the shutdown privilege for `f` only, then puts the token back
/// the way it was.
fn with_shutdown_privilege<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo handle is always valid; `token` receives a new handle, owned below.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) }
        .map_err(|e| format!("the process token could not be opened (0x{:08X})", e.code().0 as u32))?;
    let token = Owned(token);
    let mut luid = LUID::default();
    // SAFETY: a NUL-terminated constant name and a live out pointer.
    unsafe { LookupPrivilegeValueW(None, SE_SHUTDOWN_NAME, &mut luid) }
        .map_err(|e| format!("the shutdown privilege is unknown (0x{:08X})", e.code().0 as u32))?;
    let wanted = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
    };
    let mut previous = TOKEN_PRIVILEGES::default();
    let mut returned = 0u32;
    // SAFETY: live pointers to structures of the sizes passed.
    unsafe {
        AdjustTokenPrivileges(
            token.get(),
            false,
            Some(&wanted),
            size_of::<TOKEN_PRIVILEGES>() as u32,
            Some(&mut previous),
            Some(&mut returned),
        )
    }
    .map_err(|e| format!("the shutdown privilege could not be enabled (0x{:08X})", e.code().0 as u32))?;
    // SAFETY: reads the thread's last error right after the call above.
    if unsafe { GetLastError() } == ERROR_NOT_ALL_ASSIGNED {
        return Err("this account may not shut the PC down".into());
    }
    let result = f();
    // SAFETY: puts back the state saved above.
    let _ = unsafe { AdjustTokenPrivileges(token.get(), false, Some(&previous), 0, None, None) };
    Ok(result)
}

fn exit_windows(
    flags: windows::Win32::System::Shutdown::EXIT_WINDOWS_FLAGS,
    reason: SHUTDOWN_REASON,
) -> Result<(), String> {
    // SAFETY: plain flags; never EWX_FORCE or EWX_FORCEIFHUNG, so Windows' own "apps are preventing" screen
    // still protects unsaved work.
    unsafe { ExitWindowsEx(flags, reason) }
        .map_err(|e| format!("Windows refused the request (0x{:08X})", e.code().0 as u32))
}

fn real(action: Action) -> Result<(), String> {
    let planned = SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_FLAG_PLANNED;
    match action {
        // SAFETY: no arguments.
        Action::Lock => unsafe { LockWorkStation() }
            .map_err(|e| format!("Windows refused the request (0x{:08X})", e.code().0 as u32)),
        Action::Sleep => with_shutdown_privilege(|| {
            // SAFETY: plain flags; suspend, not hibernate, no force.
            if unsafe { SetSuspendState(false, false, false) } {
                Ok(())
            } else {
                Err("the PC could not sleep".to_string())
            }
        })?,
        Action::Restart => with_shutdown_privilege(|| exit_windows(EWX_REBOOT, planned))?,
        Action::ShutDown => with_shutdown_privilege(|| exit_windows(EWX_SHUTDOWN | EWX_POWEROFF, planned))?,
        Action::SignOut => exit_windows(EWX_LOGOFF, SHUTDOWN_REASON(0)),
    }
}

/// The real calls: `LockWorkStation`, `SetSuspendState`, `ExitWindowsEx`
/// (with the shutdown privilege enabled for that call only). Each refuses
/// when `RSB_TEST_TAG` is set in this process.
#[derive(Debug)]
pub struct WindowsPowerActions;

impl PowerActions for WindowsPowerActions {
    fn lock(&self) -> Result<(), String> {
        guarded(Action::Lock, tag_set(), &real)
    }

    fn sleep(&self) -> Result<(), String> {
        guarded(Action::Sleep, tag_set(), &real)
    }

    fn restart(&self) -> Result<(), String> {
        guarded(Action::Restart, tag_set(), &real)
    }

    fn shut_down(&self) -> Result<(), String> {
        guarded(Action::ShutDown, tag_set(), &real)
    }

    fn sign_out(&self) -> Result<(), String> {
        guarded(Action::SignOut, tag_set(), &real)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn effective_power_modes_fold_into_the_three_names() {
        assert_eq!(mode_name(0), "efficiency", "battery saver");
        assert_eq!(mode_name(1), "efficiency", "better battery");
        assert_eq!(mode_name(2), "balanced");
        assert_eq!(mode_name(3), "performance", "high performance");
        assert_eq!(mode_name(4), "performance", "max performance");
        assert_eq!(mode_name(5), "performance", "game mode");
        assert_eq!(mode_name(6), "performance", "mixed reality");
        assert_eq!(mode_name(7), "unknown");
        assert_eq!(mode_name(-1), "unknown");
    }

    #[test]
    fn the_test_tag_stops_every_action_before_it_runs() {
        let ran = Mutex::new(Vec::new());
        let exec = |a: Action| {
            ran.lock().unwrap().push(a);
            Ok(())
        };
        for a in [Action::Lock, Action::Sleep, Action::Restart, Action::ShutDown, Action::SignOut] {
            let e = guarded(a, true, &exec).unwrap_err();
            assert!(e.contains("test tag"), "{e}");
        }
        assert!(ran.lock().unwrap().is_empty(), "nothing ran under the tag");
        guarded(Action::Lock, false, &exec).unwrap();
        assert_eq!(*ran.lock().unwrap(), vec![Action::Lock], "without the tag the action runs once");
    }

    #[test]
    fn an_action_never_holds_the_caller_runs_in_order_and_a_failure_is_reported_whenever_it_comes() {
        let (tx, rx) = std::sync::mpsc::channel();
        let failures = |tx: std::sync::mpsc::Sender<String>| move |e: String| tx.send(e).unwrap();
        let thread = ActionThread::start().unwrap();
        assert_eq!(thread.run(|| Err("no".into()), failures(tx.clone())), Ok(()));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).as_deref(), Ok("no"), "a quick failure is reported");
        assert_eq!(thread.run(|| Ok(()), failures(tx.clone())), Ok(()));
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err(), "success reports nothing");

        let t = std::time::Instant::now();
        let slow = thread.run(
            || {
                std::thread::sleep(Duration::from_millis(400));
                Err("late".into())
            },
            failures(tx.clone()),
        );
        let returned_in = t.elapsed();
        assert_eq!(slow, Ok(()), "a call still running is not waited for");
        assert!(returned_in < Duration::from_millis(300), "{returned_in:?}");
        // Queued behind the slow one, run after it.
        assert_eq!(thread.run(|| Err("second".into()), failures(tx)), Ok(()));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).as_deref(), Ok("late"), "a late failure is reported");
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).as_deref(), Ok("second"), "in the order asked");
    }
}
