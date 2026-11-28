//! A GlazeWM-compatible WebSocket server for tests, answering exactly the
//! messages Astur's server answers: `query monitors`, `sub -e ...`,
//! `unsub <id>`, `command focus --workspace <name>`,
//! `command focus --next-active-workspace` and friends, `query binding-modes`.
//! Browser origins are refused, as Astur refuses them.

use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{Value, json};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::{Message, accept_hdr};

pub struct FakeGlazeServer {
    inner: Arc<Inner>,
    thread: Option<JoinHandle<()>>,
}

/// A workspace in the fake's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeWorkspace {
    pub name: String,
    pub display_name: String,
    pub windows: u32,
}

struct FakeState {
    monitors: Vec<Vec<FakeWorkspace>>,
    focused: (usize, usize),
}

enum Out {
    /// A workspace event for subscribed clients.
    Event,
    Raw(String),
}

struct Client {
    stream: TcpStream,
    tx: Sender<Out>,
}

struct Inner {
    port: u16,
    state: Mutex<FakeState>,
    received: Mutex<Vec<String>>,
    clients: Mutex<Vec<Client>>,
    stop: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl FakeGlazeServer {
    /// Binds 127.0.0.1 on a free port.
    pub fn start() -> std::io::Result<FakeGlazeServer> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let inner = Arc::new(Inner {
            port,
            state: Mutex::new(FakeState { monitors: Vec::new(), focused: (0, 0) }),
            received: Mutex::new(Vec::new()),
            clients: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
        });
        let i = inner.clone();
        let thread = std::thread::Builder::new().name("fake-glazewm".into()).spawn(move || {
            while !i.stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let i = i.clone();
                        let _ = std::thread::Builder::new()
                            .name("fake-glazewm-client".into())
                            .spawn(move || serve(&i, stream));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        })?;
        Ok(FakeGlazeServer { inner, thread: Some(thread) })
    }

    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.inner.port)
    }

    pub fn port(&self) -> u16 {
        self.inner.port
    }

    /// Monitors, each a list of workspaces; `focused` = (monitor, workspace).
    /// Subscribed clients get a workspace event.
    pub fn set_state(&self, monitors: Vec<Vec<FakeWorkspace>>, focused: (usize, usize)) {
        *lock(&self.inner.state) = FakeState { monitors, focused };
        self.broadcast(|| Out::Event);
    }

    /// Text commands received, oldest first.
    pub fn received(&self) -> Vec<String> {
        lock(&self.inner.received).clone()
    }

    /// Closes every client connection (reconnect tests).
    pub fn drop_clients(&self) {
        for c in lock(&self.inner.clients).drain(..) {
            let _ = c.stream.shutdown(std::net::Shutdown::Both);
        }
    }

    /// Sends this text to every client as a message (oversize-message tests).
    pub fn send_raw(&self, text: String) {
        self.broadcast(|| Out::Raw(text.clone()));
    }

    /// Clients currently connected.
    pub fn client_count(&self) -> usize {
        lock(&self.inner.clients).len()
    }

    fn broadcast(&self, make: impl Fn() -> Out) {
        for c in lock(&self.inner.clients).iter() {
            let _ = c.tx.send(make());
        }
    }
}

impl Drop for FakeGlazeServer {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::Release);
        self.drop_clients();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn windows_json(count: u32) -> Vec<Value> {
    // The first window is flat; the rest sit in nested split containers, as GlazeWM nests them.
    let window = |i: u32| json!({"type":"window","id":format!("w{i}"),"handle":1000 + i,"title":"","processName":""});
    match count {
        0 => vec![],
        1 => vec![window(0)],
        n => vec![window(0), json!({"type":"split","children": (1..n).map(window).collect::<Vec<_>>()})],
    }
}

fn monitors_json(st: &FakeState) -> Value {
    let monitors: Vec<Value> = st
        .monitors
        .iter()
        .enumerate()
        .map(|(mi, wss)| {
            let workspaces: Vec<Value> = wss
                .iter()
                .enumerate()
                .map(|(wi, w)| {
                    let displayed = if mi == st.focused.0 { wi == st.focused.1 } else { wi == 0 };
                    json!({
                        "type":"workspace","id":w.name,"name":w.name,
                        "displayName": if w.display_name.is_empty() { Value::Null } else { json!(w.display_name) },
                        "isDisplayed": displayed,
                        "hasFocus": mi == st.focused.0 && wi == st.focused.1,
                        "children": windows_json(w.windows),
                    })
                })
                .collect();
            json!({"type":"monitor","handle":65537 + mi,"hardwareId":format!("fake-{mi}"),"children":workspaces})
        })
        .collect();
    json!({"monitors": monitors})
}

fn response(command: &str, result: Result<Value, &str>) -> Message {
    let (data, error) = match result {
        Ok(d) => (d, None),
        Err(e) => (json!({}), Some(e)),
    };
    Message::text(
        json!({"messageType":"client_response","clientMessage":command,"success":error.is_none(),"error":error,"data":data}).to_string(),
    )
}

fn event() -> Message {
    Message::text(
        json!({"messageType":"event_subscription","subscriptionId":"fake-workspaces","success":true,"error":null,"data":{"eventType":"workspace_updated"}})
            .to_string(),
    )
}

fn dispatch(inner: &Inner, command: &str, subscribed: &mut bool) -> (Message, bool) {
    let parts: Vec<&str> = command.split_whitespace().collect();
    let mut changed = false;
    let result = match parts.as_slice() {
        ["query", "monitors"] => Ok(monitors_json(&lock(&inner.state))),
        ["sub", "-e", events @ ..] if events.contains(&"workspace_updated") || events.contains(&"all") => {
            *subscribed = true;
            Ok(json!({"subscriptionId":"fake-workspaces"}))
        }
        ["unsub", "fake-workspaces"] => {
            *subscribed = false;
            Ok(json!({}))
        }
        ["command", "focus", "--workspace", name] => {
            let mut st = lock(&inner.state);
            let found = st
                .monitors
                .iter()
                .enumerate()
                .find_map(|(mi, m)| m.iter().position(|w| w.name == *name).map(|wi| (mi, wi)));
            match found {
                Some(at) => {
                    st.focused = at;
                    changed = true;
                    Ok(json!({}))
                }
                None => Err("unknown workspace"),
            }
        }
        [
            "command",
            "focus",
            "--next-active-workspace"
            | "--prev-active-workspace"
            | "--next-active-workspace-on-monitor"
            | "--prev-active-workspace-on-monitor",
        ] => Ok(json!({})),
        ["query", "binding-modes"] => Ok(json!({"bindingModes": []})),
        _ => Err("the fake supports workspace queries, subscriptions and focus commands only"),
    };
    (response(command, result), changed)
}

// The error type is fixed by tungstenite's handshake callback.
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

fn serve(inner: &Arc<Inner>, stream: TcpStream) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    let Ok(shutdown_handle) = stream.try_clone() else { return };
    let Ok(mut ws) = accept_hdr(stream, check_origin) else { return };
    let (tx, rx): (Sender<Out>, Receiver<Out>) = mpsc::channel();
    lock(&inner.clients).push(Client { stream: shutdown_handle, tx });
    let _ = ws.get_mut().set_read_timeout(Some(Duration::from_millis(20)));
    let mut subscribed = false;
    loop {
        if inner.stop.load(Ordering::Acquire) {
            break;
        }
        match ws.read() {
            Ok(Message::Text(t)) => {
                lock(&inner.received).push(t.as_str().to_string());
                let (reply, changed) = dispatch(inner, t.as_str(), &mut subscribed);
                if ws.send(reply).is_err() {
                    break;
                }
                if changed {
                    for c in lock(&inner.clients).iter() {
                        let _ = c.tx.send(Out::Event);
                    }
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
        while let Ok(out) = rx.try_recv() {
            let sent = match out {
                Out::Event if subscribed => ws.send(event()),
                Out::Event => Ok(()),
                Out::Raw(text) => ws.send(Message::text(text)),
            };
            if sent.is_err() {
                return;
            }
        }
    }
}
