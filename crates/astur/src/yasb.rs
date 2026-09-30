//! Workspace subset of GlazeWM's WebSocket protocol, for unmodified YASB.
//! All sockets and JSON stay on this worker. The manager only publishes model
//! copies when changed and consumes focus commands through its existing queue.

use std::io;
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tungstenite::handshake::server::{ErrorResponse, Request, Response, ServerHandshake};
use tungstenite::handshake::MidHandshake;
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{accept_hdr_with_config, Error, HandshakeError, Message, WebSocket};

use super::{Cmd, Manager};

const MAX_CLIENTS: usize = 16;
const IO_TICK: Duration = Duration::from_millis(16);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, PartialEq)]
pub(super) enum Focus {
    Workspace { monitor: isize, index: usize },
    Cycle { delta: i32, on_monitor: bool },
}

#[derive(Default)]
struct Snapshot {
    monitors: Vec<MonitorState>,
    primary: usize,
    focused: usize,
    per_monitor: bool,
    labels: Vec<String>,
}

struct MonitorState {
    handle: isize,
    active: usize,
    workspaces: Vec<WorkspaceState>,
}

struct WorkspaceState {
    windows: Vec<isize>,
    floating: Vec<isize>,
    focused: isize,
}

impl Snapshot {
    fn matches(&self, mgr: &Manager) -> bool {
        self.primary == mgr.primary
            && self.focused == mgr.focused_mon
            && self.per_monitor == mgr.cfg.per_monitor
            && self.labels == mgr.cfg.workspace_names
            && self.monitors.len() == mgr.monitors.len()
            && self.monitors.iter().zip(&mgr.monitors).all(|(a, b)| {
                a.handle == b.hmon
                    && a.active == b.active
                    && a.workspaces.len() == b.workspaces.len()
                    && a.workspaces.iter().zip(&b.workspaces).all(|(a, b)| {
                        a.windows == b.windows && a.floating == b.floating && a.focused == b.focused
                    })
            })
    }

    fn from_manager(mgr: &Manager) -> Self {
        Self {
            primary: mgr.primary,
            focused: mgr.focused_mon,
            per_monitor: mgr.cfg.per_monitor,
            labels: mgr.cfg.workspace_names.clone(),
            monitors: mgr
                .monitors
                .iter()
                .map(|m| MonitorState {
                    handle: m.hmon,
                    active: m.active,
                    workspaces: m
                        .workspaces
                        .iter()
                        .map(|w| WorkspaceState {
                            windows: w.windows.clone(),
                            floating: w.floating.clone(),
                            focused: w.focused,
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    fn number(&self, mi: usize, wi: usize) -> usize {
        if self.per_monitor {
            wi + 1
        } else {
            let n = self.monitors.len();
            wi * n + (mi + n - self.primary % n) % n + 1
        }
    }

    fn name(&self, mi: usize, wi: usize) -> String {
        if self.per_monitor {
            // YASB keys buttons by name across ALL monitors. Local "1" alone
            // would merge unrelated workspaces. Display names remain friendly.
            format!("m{}-w{}", self.monitors[mi].handle, wi + 1)
        } else {
            self.number(mi, wi).to_string()
        }
    }

    fn workspace(&self, name: &str) -> Option<Focus> {
        self.monitors.iter().enumerate().find_map(|(mi, m)| {
            (0..m.workspaces.len()).find_map(|wi| {
                (self.name(mi, wi) == name).then_some(Focus::Workspace {
                    monitor: m.handle,
                    index: wi,
                })
            })
        })
    }

    fn monitors_json(&self) -> Value {
        let monitors: Vec<_> = self
            .monitors
            .iter()
            .enumerate()
            .map(|(mi, m)| {
                let workspaces: Vec<_> = m
                    .workspaces
                    .iter()
                    .enumerate()
                    .map(|(wi, w)| {
                        let number = self.number(mi, wi);
                        let name = self.name(mi, wi);
                        let label = self
                            .labels
                            .get(number - 1)
                            .filter(|s| !s.is_empty())
                            .cloned()
                            .unwrap_or_else(|| number.to_string());
                        let windows: Vec<_> = w.windows.iter().map(|h| json!({
                    "type": "window", "id": format!("window-{h}"), "handle": h,
                    "title": "", "className": "", "processName": "",
                    "displayState": if wi == m.active { "shown" } else { "hidden" },
                    "state": {"type": if w.floating.contains(h) { "floating" } else { "tiling" }},
                })).collect();
                        json!({
                            "type": "workspace", "id": name, "name": name, "displayName": label,
                            "isDisplayed": wi == m.active,
                            "hasFocus": mi == self.focused && wi == m.active,
                            "children": windows,
                        })
                    })
                    .collect();
                json!({"type": "monitor", "handle": m.handle,
                "hardwareId": format!("astur-{}", m.handle), "children": workspaces})
            })
            .collect();
        json!({"monitors": monitors})
    }
}

#[derive(Default)]
struct Published {
    port: u16, // zero = disabled
    revision: u64,
    snapshot: Arc<Snapshot>,
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static STATE: LazyLock<(Mutex<Published>, Condvar)> =
    LazyLock::new(|| (Mutex::new(Published::default()), Condvar::new()));

pub(super) fn publish(mgr: &Manager) {
    // No lock, model copy or wakeup on the default (disabled) path.
    if !mgr.cfg.yasb_enabled && !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let mut state = STATE.0.lock().unwrap();
    let port = if mgr.cfg.yasb_enabled {
        mgr.cfg.yasb_port
    } else {
        0
    };
    if state.port == port && state.snapshot.matches(mgr) {
        return;
    }
    state.port = port;
    state.snapshot = Arc::new(Snapshot::from_manager(mgr));
    state.revision = state.revision.wrapping_add(1);
    ENABLED.store(port != 0, Ordering::Relaxed);
    STATE.1.notify_one();
}

/// Runs only on the manager. Relative commands resolve here so rapid scroll
/// commands accumulate, instead of all targeting the same stale snapshot.
pub(super) unsafe fn focus_workspace(mgr: &mut Manager, focus: Focus) {
    if !mgr.cfg.yasb_enabled || mgr.monitors.is_empty() {
        return;
    }
    let target = match focus {
        Focus::Workspace { monitor, index } => mgr
            .mon_by_hmon(monitor)
            .filter(|&mi| index < mgr.monitors[mi].workspaces.len())
            .map(|mi| (mi, index)),
        Focus::Cycle { delta, on_monitor } => {
            let mut mi = mgr.focused_mon;
            if on_monitor {
                let mut point = super::POINT::default();
                if super::GetCursorPos(&mut point).is_ok() {
                    let hmon = super::MonitorFromPoint(point, super::MONITOR_DEFAULTTONEAREST);
                    mi = mgr.mon_by_hmon(hmon.0 as isize).unwrap_or(mi);
                }
            }
            cycle_target(mgr, mi, delta, on_monitor)
        }
    };
    if let Some((mi, wi)) = target {
        super::show_workspace(mgr, mi, wi);
    }
}

fn cycle_target(mgr: &Manager, mi: usize, delta: i32, on_monitor: bool) -> Option<(usize, usize)> {
    let monitor = mgr.monitors.get(mi)?;
    let mut slots: Vec<_> = mgr
        .monitors
        .iter()
        .enumerate()
        .filter(|(i, _)| !on_monitor || *i == mi)
        .flat_map(|(i, m)| (0..m.workspaces.len()).map(move |w| (i, w)))
        .collect();
    if !mgr.cfg.per_monitor {
        slots.sort_by_key(|&(i, w)| mgr.ml_to_global(i, w));
    }
    let current = slots
        .iter()
        .position(|&slot| slot == (mi, monitor.active))?;
    let next = (current as i64 + i64::from(delta)).rem_euclid(slots.len() as i64) as usize;
    Some(slots[next])
}

fn response(command: &str, result: Result<Value, &str>) -> Message {
    let (data, error) = match result {
        Ok(data) => (data, None),
        Err(error) => (json!({}), Some(error)),
    };
    Message::Text(
        json!({
            "messageType": "client_response", "clientMessage": command,
            "success": error.is_none(), "error": error, "data": data,
        })
        .to_string()
        .into(),
    )
}

fn dispatch(
    command: &str,
    snapshot: &Snapshot,
    monitors: &Value,
    subscribed: &mut bool,
    enqueue: &mut impl FnMut(Focus),
) -> Message {
    let parts: Vec<_> = command.split_whitespace().collect();
    let result = match parts.as_slice() {
        ["query", "monitors"] => Ok(monitors.clone()),
        ["sub", "-e", events @ ..]
            if (events.contains(&"workspace_updated") || events.contains(&"all"))
                && events.iter().all(|e| {
                    matches!(
                        *e,
                        "workspace_activated"
                            | "workspace_deactivated"
                            | "workspace_updated"
                            | "focus_changed"
                            | "focused_container_moved"
                            | "all"
                    )
                }) =>
        {
            *subscribed = true;
            Ok(json!({"subscriptionId": "astur-workspaces"}))
        }
        ["unsub", "astur-workspaces"] => {
            *subscribed = false;
            Ok(json!({}))
        }
        ["command", "focus", "--workspace", name] => match snapshot.workspace(name) {
            Some(focus) => {
                enqueue(focus);
                Ok(json!({}))
            }
            None => Err("unknown workspace"),
        },
        ["command", "focus", flag] => {
            let cycle = match *flag {
                "--next-active-workspace-on-monitor" => Some((1, true)),
                "--prev-active-workspace-on-monitor" => Some((-1, true)),
                "--next-active-workspace" => Some((1, false)),
                "--prev-active-workspace" => Some((-1, false)),
                _ => None,
            };
            match cycle {
                Some((delta, on_monitor)) => {
                    enqueue(Focus::Cycle { delta, on_monitor });
                    Ok(json!({}))
                }
                None => Err("unsupported focus command"),
            }
        }
        // YASB sends these after every event even for a workspace-only widget.
        ["query", "binding-modes"] => Ok(json!({"bindingModes": []})),
        _ => Err("Astur supports workspace queries, subscriptions and focus commands only"),
    };
    response(command, result)
}

type HeaderCheck = fn(&Request, Response) -> Result<Response, ErrorResponse>;
type Pending = MidHandshake<ServerHandshake<TcpStream, HeaderCheck>>;

// Qt's native QWebSocket sends no Origin by default. Browsers always do: reject
// them, including localhost pages, so visiting a site cannot control the WM.
#[allow(clippy::result_large_err)]
fn check_origin(request: &Request, response: Response) -> Result<Response, ErrorResponse> {
    if request.headers().contains_key("origin") {
        let mut denied = ErrorResponse::new(Some("Browser origins are not allowed".into()));
        *denied.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
        Err(denied)
    } else {
        Ok(response)
    }
}

enum Connection {
    Handshake(Pending),
    Open {
        socket: WebSocket<TcpStream>,
        subscribed: bool,
        revision: u64,
    },
}

struct Peer {
    connection: Option<Connection>,
    connected: Instant,
}

fn handshake_result(
    result: Result<WebSocket<TcpStream>, HandshakeError<ServerHandshake<TcpStream, HeaderCheck>>>,
) -> Option<Connection> {
    match result {
        Ok(socket) => Some(Connection::Open {
            socket,
            subscribed: false,
            revision: 0,
        }),
        Err(HandshakeError::Interrupted(pending)) => Some(Connection::Handshake(pending)),
        Err(HandshakeError::Failure(_)) => None,
    }
}

fn pending_io(result: Result<(), Error>) -> bool {
    match result {
        Ok(()) => true,
        Err(Error::Io(e)) => e.kind() == io::ErrorKind::WouldBlock,
        Err(_) => false,
    }
}

impl Peer {
    fn tick(
        &mut self,
        revision: u64,
        snapshot: &Snapshot,
        monitors: &Value,
        enqueue: &mut impl FnMut(Focus),
    ) -> bool {
        let Some(connection) = self.connection.take() else {
            return false;
        };
        self.connection = match connection {
            Connection::Handshake(pending) => {
                if self.connected.elapsed() >= HANDSHAKE_TIMEOUT {
                    return false;
                }
                handshake_result(pending.handshake())
            }
            Connection::Open {
                mut socket,
                mut subscribed,
                revision: old_revision,
            } => {
                let was_subscribed = subscribed;
                if !pending_io(socket.flush()) {
                    return false;
                }
                // Bounded work per peer: a chatty client cannot starve others.
                for _ in 0..16 {
                    match socket.read() {
                        Ok(Message::Text(text)) => {
                            let reply =
                                dispatch(&text, snapshot, monitors, &mut subscribed, enqueue);
                            if !pending_io(socket.send(reply)) {
                                return false;
                            }
                        }
                        Ok(Message::Close(_)) => {
                            let _ = socket.flush();
                            return false;
                        }
                        Ok(Message::Ping(_) | Message::Pong(_)) => {}
                        Ok(_) => return false,
                        Err(Error::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => break,
                        Err(_) => return false,
                    }
                }
                if was_subscribed && subscribed && revision != old_revision {
                    let event = Message::Text(json!({
                        "messageType": "event_subscription", "subscriptionId": "astur-workspaces",
                        "success": true, "error": null, "data": {"eventType": "workspace_updated"},
                    }).to_string().into());
                    if !pending_io(socket.send(event)) {
                        return false;
                    }
                }
                self.connection = Some(Connection::Open {
                    socket,
                    subscribed,
                    revision,
                });
                return true;
            }
        };
        self.connection.is_some()
    }
}

struct Server {
    listener: TcpListener,
    peers: Vec<Peer>,
}

impl Server {
    fn bind(port: u16) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            peers: Vec::new(),
        })
    }

    fn tick(
        &mut self,
        revision: u64,
        snapshot: &Snapshot,
        monitors: &Value,
        mut enqueue: impl FnMut(Focus),
    ) {
        for _ in 0..MAX_CLIENTS {
            let Ok((stream, _)) = self.listener.accept() else {
                break;
            };
            if self.peers.len() >= MAX_CLIENTS || stream.set_nonblocking(true).is_err() {
                continue;
            }
            let _ = stream.set_nodelay(true);
            let config = WebSocketConfig::default()
                .read_buffer_size(4096)
                .write_buffer_size(0)
                .max_write_buffer_size(1024 * 1024)
                .max_message_size(Some(4096))
                .max_frame_size(Some(4096));
            let connection = handshake_result(accept_hdr_with_config(
                stream,
                check_origin as HeaderCheck,
                Some(config),
            ));
            if connection.is_some() {
                self.peers.push(Peer {
                    connection,
                    connected: Instant::now(),
                });
            }
        }
        self.peers
            .retain_mut(|p| p.tick(revision, snapshot, monitors, &mut enqueue));
    }
}

pub(super) fn worker() {
    let mut server: Option<Server> = None;
    let mut bound_port = 0;
    let mut cached_revision = 0;
    let mut monitors = json!({"monitors": []});
    let mut failed_port = None;
    loop {
        let (port, revision, snapshot) = {
            let mut state = STATE.0.lock().unwrap();
            if state.port == 0 {
                server = None; // Close listeners and clients on hot disable.
                bound_port = 0;
                state = STATE.1.wait_while(state, |s| s.port == 0).unwrap();
            }
            (state.port, state.revision, Arc::clone(&state.snapshot))
        };
        if bound_port != port {
            server = None;
            bound_port = 0;
            match Server::bind(port) {
                Ok(new) => {
                    server = Some(new);
                    bound_port = port;
                    failed_port = None;
                }
                Err(error) => {
                    if failed_port != Some(port) {
                        if super::log_on(super::LOG_ERROR) {
                            super::log_push(
                                super::LOG_ERROR,
                                &format!("YASB cannot bind 127.0.0.1:{port}: {error}"),
                            );
                        }
                        failed_port = Some(port);
                    }
                    let state = STATE.0.lock().unwrap();
                    let _ = STATE
                        .1
                        .wait_timeout_while(state, Duration::from_secs(2), |s| s.port == port);
                    continue;
                }
            }
        }
        if revision != cached_revision {
            monitors = snapshot.monitors_json();
            cached_revision = revision;
        }
        if let Some(server) = server.as_mut() {
            server.tick(revision, &snapshot, &monitors, |focus| {
                super::push_cmd(Cmd::YasbFocus(focus))
            });
        }
        let state = STATE.0.lock().unwrap();
        // Model changes wake immediately. The short timeout services sockets;
        // no manager polling, processes spawned, or socket work on input paths.
        let wait = if server.as_ref().is_some_and(|s| s.peers.is_empty()) {
            Duration::from_millis(100)
        } else {
            IO_TICK
        };
        let _ = STATE
            .1
            .wait_timeout_while(state, wait, |s| s.revision == revision);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use tungstenite::client::IntoClientRequest;

    fn manager(per_monitor: bool) -> Manager {
        let mut mgr = Manager {
            monitors: (0..2)
                .map(|i| super::super::Monitor::new(0x1000 + i, super::super::RECT::default(), 3))
                .collect(),
            primary: 1,
            focused_mon: 1,
            tiling: true,
            cfg: super::super::Config::defaults(),
            pending_launch_mon: 0,
        };
        mgr.cfg.per_monitor = per_monitor;
        mgr.cfg.workspace_names = vec!["Code \"main\"".into(), "Chat".into()];
        mgr.monitors[1].active = 1;
        mgr.monitors[1].workspaces[1].windows = vec![123, 456];
        mgr.monitors[1].workspaces[1].floating = vec![456];
        mgr
    }

    #[test]
    fn model_matches_yasb_names_handles_occupancy_and_focus() {
        let mut mgr = manager(true);
        let state = Snapshot::from_manager(&mgr);
        let data = state.monitors_json();
        let monitors = data["monitors"].as_array().unwrap();
        let first = &monitors[0]["children"][0];
        let second = &monitors[1]["children"][0];
        assert_ne!(
            first["name"], second["name"],
            "YASB uses globally unique keys"
        );
        assert_eq!(first["displayName"], "Code \"main\"");
        assert_eq!(first["displayName"], second["displayName"]);
        assert_eq!(monitors[1]["handle"], 0x1001);
        let focused = &monitors[1]["children"][1];
        assert_eq!(focused["hasFocus"], true);
        assert_eq!(focused["isDisplayed"], true);
        assert_eq!(focused["children"].as_array().unwrap().len(), 2);
        assert_eq!(focused["children"][0]["handle"], 123);
        assert_eq!(focused["children"][1]["state"]["type"], "floating");
        assert_eq!(
            state.workspace(focused["name"].as_str().unwrap()),
            Some(Focus::Workspace {
                monitor: 0x1001,
                index: 1
            })
        );
        assert!(state.matches(&mgr));
        mgr.monitors[1].workspaces[1].windows.pop();
        assert!(!state.matches(&mgr));
        let shared = Snapshot::from_manager(&manager(false));
        assert_eq!(shared.name(1, 0), "1", "primary owns workspace 1");
        assert_eq!(shared.name(0, 0), "2");
        assert_eq!(shared.name(1, 1), "3");
        assert_eq!(cycle_target(&manager(false), 1, 1, false), Some((0, 1)));
        assert_eq!(cycle_target(&manager(false), 1, -1, false), Some((0, 0)));
        assert_eq!(cycle_target(&manager(true), 0, -1, true), Some((0, 2)));
        assert_eq!(cycle_target(&manager(true), 1, 1, true), Some((1, 2)));
    }

    #[test]
    fn commands_are_strict_and_only_enqueue_workspace_actions() {
        let state = Snapshot::from_manager(&manager(false));
        let monitors = state.monitors_json();
        let mut subscribed = false;
        let mut commands = Vec::new();
        for bad in [
            "command focus --workspace 0",
            "command focus --workspace 999",
            "command focus --workspace 1 extra",
            "command shell calc.exe",
            "launch calc.exe",
            "command toggle-tiling-direction",
            "sub -e unsupported",
            "query monitors extra",
        ] {
            let reply = dispatch(bad, &state, &monitors, &mut subscribed, &mut |f| {
                commands.push(f)
            });
            let reply: Value = serde_json::from_str(reply.to_text().unwrap()).unwrap();
            assert_eq!(reply["success"], false, "{bad}");
        }
        assert!(commands.is_empty());
        assert!(!subscribed);
        for command in [
            "command focus --workspace 1",
            "command focus --next-active-workspace-on-monitor",
            "command focus --prev-active-workspace",
        ] {
            let reply = dispatch(command, &state, &monitors, &mut subscribed, &mut |f| {
                commands.push(f)
            });
            let reply: Value = serde_json::from_str(reply.to_text().unwrap()).unwrap();
            assert_eq!(reply["success"], true);
        }
        assert_eq!(
            commands,
            vec![
                Focus::Workspace {
                    monitor: 0x1001,
                    index: 0
                },
                Focus::Cycle {
                    delta: 1,
                    on_monitor: true
                },
                Focus::Cycle {
                    delta: -1,
                    on_monitor: false
                }
            ]
        );
    }

    struct TestServer {
        address: std::net::SocketAddr,
        stop: Arc<AtomicBool>,
        join: Option<std::thread::JoinHandle<()>>,
        update: mpsc::Sender<Snapshot>,
        commands: mpsc::Receiver<Focus>,
    }

    impl TestServer {
        fn start() -> Self {
            let mut server = Server::bind(0).unwrap();
            let address = server.listener.local_addr().unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let running = Arc::clone(&stop);
            let (update, updates) = mpsc::channel();
            let (commands_tx, commands) = mpsc::channel();
            let join = std::thread::spawn(move || {
                let mut snapshot = Snapshot::from_manager(&manager(false));
                let mut monitors = snapshot.monitors_json();
                let mut revision = 1;
                while !running.load(Ordering::Relaxed) {
                    if let Ok(next) = updates.try_recv() {
                        snapshot = next;
                        monitors = snapshot.monitors_json();
                        revision += 1;
                    }
                    server.tick(revision, &snapshot, &monitors, |f| {
                        let _ = commands_tx.send(f);
                    });
                    std::thread::sleep(Duration::from_millis(2));
                }
            });
            Self {
                address,
                stop,
                join: Some(join),
                update,
                commands,
            }
        }

        fn connect(&self) -> WebSocket<TcpStream> {
            let stream = TcpStream::connect(self.address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            tungstenite::client(format!("ws://{}", self.address), stream)
                .unwrap()
                .0
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            self.join.take().unwrap().join().unwrap();
        }
    }

    fn read_json(socket: &mut WebSocket<TcpStream>) -> Value {
        serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap()
    }

    #[test]
    fn real_websocket_yasb_queries_subscribes_switches_and_reconnects() {
        let server = TestServer::start();
        // An incomplete handshake must not hold up a real YASB connection.
        let _slow_handshake = TcpStream::connect(server.address).unwrap();
        let mut socket = server.connect();
        socket.send(Message::text("sub -e workspace_activated workspace_deactivated workspace_updated focus_changed focused_container_moved")).unwrap();
        socket.send(Message::text("query monitors")).unwrap();
        let ack = read_json(&mut socket);
        assert_eq!(ack["success"], true);
        assert_eq!(ack["data"]["subscriptionId"], "astur-workspaces");
        let query = read_json(&mut socket);
        assert_eq!(query["clientMessage"], "query monitors");
        assert_eq!(query["data"]["monitors"].as_array().unwrap().len(), 2);

        socket
            .send(Message::text("command focus --workspace 2"))
            .unwrap();
        assert_eq!(read_json(&mut socket)["success"], true);
        assert_eq!(
            server
                .commands
                .recv_timeout(Duration::from_secs(3))
                .unwrap(),
            Focus::Workspace {
                monitor: 0x1000,
                index: 0
            }
        );
        let mut changed = manager(false);
        changed.focused_mon = 0;
        server
            .update
            .send(Snapshot::from_manager(&changed))
            .unwrap();
        assert_eq!(read_json(&mut socket)["messageType"], "event_subscription");
        socket.send(Message::text("query monitors")).unwrap();
        assert_eq!(
            read_json(&mut socket)["data"]["monitors"][0]["children"][0]["hasFocus"],
            true
        );
        // No duplicate events when state is unchanged: next reply is the ping.
        socket.send(Message::Ping(vec![1, 2, 3].into())).unwrap();
        assert_eq!(socket.read().unwrap(), Message::Pong(vec![1, 2, 3].into()));
        socket.close(None).unwrap();
        let mut reconnected = server.connect();
        reconnected.send(Message::text("query monitors")).unwrap();
        assert_eq!(read_json(&mut reconnected)["success"], true);
    }

    #[test]
    fn browser_origin_rejected_and_port_conflicts_do_not_panic() {
        let server = TestServer::start();
        assert!(Server::bind(server.address.port()).is_err());
        let stream = TcpStream::connect(server.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = format!("ws://{}", server.address)
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("origin", "http://localhost".parse().unwrap());
        match tungstenite::client(request, stream) {
            Err(HandshakeError::Failure(Error::Http(response))) => {
                assert_eq!(response.status(), 403)
            }
            _ => panic!("browser origin was not rejected"),
        }
        let mut socket = server.connect();
        socket.send(Message::text("query monitors")).unwrap();
        assert_eq!(read_json(&mut socket)["success"], true);
    }
}
