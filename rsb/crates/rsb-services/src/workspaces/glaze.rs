//! The GlazeWM-compatible WebSocket client (GlazeWM v3, Astur).
//!
//! The connection thread never polls: it waits on the socket's event
//! (`WSAEventSelect`), a command event and the stop event, with a 60 s
//! timeout that is the ping interval, so an idle connection wakes once a
//! minute and commands go out at once.
//!
//! The server is trusted only when the process listening on the port runs
//! in our session as our user: otherwise another user's process could feed
//! the bar workspaces. Messages and frames are capped at `WS_MAX_MESSAGE`.

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::os::windows::io::AsRawSocket;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rsb_core::services::ServiceStatus;
use serde_json::Value as Json;
use tungstenite::client::IntoClientRequest;
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};
use windows::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows::Win32::Networking::WinSock::{FD_CLOSE, FD_READ, SOCKET, WSAEVENT, WSAEventSelect};
use windows::Win32::System::Threading::{ResetEvent, WaitForMultipleObjects};

use super::model::from_glaze;
use super::{Shared, publish_model};
use crate::win::{Peer, classify_process};
use crate::{Publisher, WS_MAX_MESSAGE};

/// Between pings, and the longest an idle connection sleeps.
const PING_EVERY: Duration = Duration::from_secs(60);
/// A ping unanswered this long means the connection is gone.
const PONG_WITHIN: Duration = Duration::from_secs(10);
/// Events arrive in bursts; one `query monitors` follows the last of them.
const REQUERY_AFTER: Duration = Duration::from_millis(20);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) const SUBSCRIBE: &str = "sub -e workspace_activated workspace_deactivated workspace_updated focus_changed";
pub(crate) const QUERY: &str = "query monitors";

pub(crate) enum End {
    Stopped,
    /// The connection ended; try again with backoff.
    Lost(String),
    /// Not talked to at all (the port belongs to someone else).
    Refused(String),
    /// Nothing is listening.
    NotRunning,
}

/// `ws://127.0.0.1:<port>`: the only form accepted.
pub(crate) fn parse_url(url: &str) -> Result<u16, String> {
    let rest = url.strip_prefix("ws://").ok_or("the workspace provider address must start with ws://")?;
    let (host, port) =
        rest.trim_end_matches('/').rsplit_once(':').ok_or("the workspace provider address needs a port")?;
    if host != "127.0.0.1" {
        return Err("the workspace provider must be on 127.0.0.1".into());
    }
    port.parse::<u16>().map_err(|_| "bad port in the workspace provider address".to_string())
}

/// ERROR_INSUFFICIENT_BUFFER: the table grew between the size query and the read.
const TABLE_TOO_SMALL: u32 = 122;

/// The raw listener table for one address family (2 = IPv4, 23 = IPv6).
fn listener_table(family: u32) -> Result<Vec<u64>, String> {
    let mut size = 0u32;
    // SAFETY: a size query with no buffer; it fails with ERROR_INSUFFICIENT_BUFFER and sets `size`.
    let _ = unsafe { GetExtendedTcpTable(None, &mut size, false, family, TCP_TABLE_OWNER_PID_LISTENER, 0) };
    for _ in 0..5 {
        // u64 elements keep the table 8-byte aligned; room to grow avoids most retries.
        let mut buf = vec![0u64; (size as usize).div_ceil(8) + 64];
        let mut have = (buf.len() * 8) as u32;
        // SAFETY: `buf` holds `have` bytes.
        let r = unsafe {
            GetExtendedTcpTable(
                Some(buf.as_mut_ptr().cast()),
                &mut have,
                false,
                family,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        match r {
            0 => return Ok(buf),
            TABLE_TOO_SMALL => size = have,
            other => return Err(format!("the listener table could not be read (error {other})")),
        }
    }
    Err("the listener table kept changing".into())
}

/// Process ids listening at `port` on loopback or on any address, over IPv4 or IPv6
/// (a dual-stack listener shows only in the IPv6 table). An error means the answer
/// is unknown, which callers must not take as "nobody".
pub(crate) fn listener_pids(port: u16) -> Result<Vec<u32>, String> {
    let mut pids = Vec::new();
    let v4 = listener_table(2)?;
    let base = v4.as_ptr().cast::<u8>();
    // SAFETY: a successful call filled a MIB_TCPTABLE_OWNER_PID: a count, then that many rows.
    let count = unsafe { *base.cast::<u32>() } as usize;
    // SAFETY: the rows follow the 4-byte count, inside the buffer (the table is `count` rows long).
    let rows = unsafe { std::slice::from_raw_parts(base.add(4).cast::<MIB_TCPROW_OWNER_PID>(), count) };
    pids.extend(
        rows.iter()
            .filter(|r| u16::from_be((r.dwLocalPort & 0xFFFF) as u16) == port)
            .filter(|r| r.dwLocalAddr == 0 || r.dwLocalAddr == u32::from_ne_bytes([127, 0, 0, 1]))
            .map(|r| r.dwOwningPid),
    );
    // A machine without IPv6 has no second table: that is not a failure.
    if let Ok(v6) = listener_table(23) {
        let base = v6.as_ptr().cast::<u8>();
        // SAFETY: as above, for the IPv6 table.
        let count = unsafe { *base.cast::<u32>() } as usize;
        // SAFETY: as above.
        let rows = unsafe { std::slice::from_raw_parts(base.add(4).cast::<MIB_TCP6ROW_OWNER_PID>(), count) };
        let any = [0u8; 16];
        let mut loopback = [0u8; 16];
        loopback[15] = 1;
        pids.extend(
            rows.iter()
                .filter(|r| u16::from_be((r.dwLocalPort & 0xFFFF) as u16) == port)
                .filter(|r| r.ucLocalAddr == any || r.ucLocalAddr == loopback)
                .map(|r| r.dwOwningPid),
        );
    }
    pids.sort_unstable();
    pids.dedup();
    Ok(pids)
}

/// Every listener on the port must be ours. `Ok(None)` = nothing listens there;
/// `Ok(Some(true))` = at least one is an elevated or protected process of our
/// session whose user we cannot read.
pub(crate) fn check_listener(port: u16) -> Result<Option<bool>, String> {
    let pids = listener_pids(port)?;
    if pids.is_empty() {
        return Ok(None);
    }
    let mut unknown = false;
    for pid in pids {
        match classify_process(pid)? {
            Peer::SameUser => {}
            Peer::SameSessionUnknownUser => unknown = true,
            Peer::OtherUser | Peer::OtherSession => return Err(format!("port {port} belongs to another user")),
        }
    }
    Ok(Some(unknown))
}

struct Link {
    ws: WebSocket<TcpStream>,
    socket_event: crate::win::Owned,
}

fn connect(port: u16) -> Result<(WebSocket<TcpStream>, SOCKET), End> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).map_err(|_| End::NotRunning)?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT));
    let socket = SOCKET(stream.as_raw_socket() as usize);
    // No Origin header: browsers always send one, and servers like Astur refuse it.
    let request = format!("ws://127.0.0.1:{port}").into_client_request().map_err(|e| End::Lost(e.to_string()))?;
    let config = WebSocketConfig::default().max_message_size(Some(WS_MAX_MESSAGE)).max_frame_size(Some(WS_MAX_MESSAGE));
    let (ws, _response) = tungstenite::client::client_with_config(request, stream, Some(config))
        .map_err(|_| End::Lost("the WebSocket handshake failed".into()))?;
    Ok((ws, socket))
}

fn send_text(ws: &mut WebSocket<TcpStream>, text: &str) -> Result<(), String> {
    match ws.send(Message::text(text)) {
        Ok(()) => Ok(()),
        // Queued: tungstenite flushes on the next write or read.
        Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
        Err(e) => Err(format!("send failed ({e})")),
    }
}

/// One connection, from handshake to loss or stop. `commands` are drained
/// from the shared queue whenever the command event fires.
pub(crate) fn run_session(shared: &Arc<Shared>, out: &Publisher, stop: &crate::win::Owned, port: u16) -> End {
    match check_listener(port) {
        Err(reason) => return End::Refused(reason),
        Ok(None) => return End::NotRunning,
        Ok(Some(true)) => log::info!(
            "workspaces: the listener on port {port} is an elevated or protected process of this session; accepted"
        ),
        Ok(Some(false)) => {}
    }
    let (mut ws, socket) = match connect(port) {
        Ok(c) => c,
        Err(end) => return end,
    };
    // Check again now that we are connected: the listener may have changed since.
    // Nobody found now that a connection exists means we cannot say who answered: refuse.
    match check_listener(port) {
        Ok(Some(_)) => {}
        Ok(None) => return End::Refused(format!("no process could be identified as the owner of port {port}")),
        Err(reason) => return End::Refused(reason),
    }
    let socket_event = match crate::win::manual_event() {
        Ok(e) => e,
        Err(e) => return End::Lost(e),
    };
    // SAFETY: the socket belongs to `ws`, which outlives the selection; the event is live.
    if unsafe { WSAEventSelect(socket, Some(WSAEVENT(socket_event.get().0 as isize)), (FD_READ | FD_CLOSE) as i32) }
        != 0
    {
        return End::Lost("could not watch the socket".into());
    }
    // Non-blocking from here: reads return WouldBlock when empty.
    let _ = ws.get_mut().set_read_timeout(None);
    let _ = ws.get_mut().set_write_timeout(None);
    if ws.get_mut().set_nonblocking(true).is_err() {
        return End::Lost("could not make the socket non-blocking".into());
    }
    let mut link = Link { ws, socket_event };

    for first in [SUBSCRIBE, QUERY] {
        if let Err(e) = send_text(&mut link.ws, first) {
            return End::Lost(e);
        }
    }
    log::info!("workspaces: connected to a GlazeWM-compatible server");

    let mut requery_at: Option<Instant> = None;
    let mut ping_due = Instant::now() + PING_EVERY;
    let mut pong_by: Option<Instant> = None;
    loop {
        let now = Instant::now();
        let mut until = ping_due;
        if let Some(t) = requery_at {
            until = until.min(t);
        }
        if let Some(t) = pong_by {
            until = until.min(t);
        }
        let ms = until.saturating_duration_since(now).as_millis().min(u128::from(u32::MAX - 1)) as u32;
        crate::count_source_wake();
        // SAFETY: all three handles are live events.
        let waited = unsafe {
            WaitForMultipleObjects(&[stop.get(), shared.glaze_wake.get(), link.socket_event.get()], false, ms)
        };
        let now = Instant::now();
        if waited == WAIT_OBJECT_0 {
            let _ = link.ws.close(None);
            return End::Stopped;
        }
        if waited.0 == WAIT_OBJECT_0.0 + 1 {
            // A command to send.
            let cmds: Vec<String> = shared.glaze_cmds.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
            for c in cmds {
                if let Err(e) = send_text(&mut link.ws, &c) {
                    return End::Lost(e);
                }
            }
        }
        if waited.0 == WAIT_OBJECT_0.0 + 2 {
            // SAFETY: a live event.
            let _ = unsafe { ResetEvent(link.socket_event.get()) };
        }
        // Read whatever arrived (cheap when nothing did), until the socket would block.
        loop {
            match link.ws.read() {
                Ok(Message::Text(t)) => handle_text(t.as_str(), shared, out, &mut requery_at, now),
                Ok(Message::Pong(_)) => {
                    pong_by = None;
                    out.alive();
                }
                Ok(Message::Ping(_)) => {}
                Ok(Message::Close(_)) => return End::Lost("the server closed the connection".into()),
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(tungstenite::Error::Capacity(_)) => {
                    log::warn!(
                        "workspaces: a message over {WS_MAX_MESSAGE} bytes was refused; dropping the connection"
                    );
                    return End::Lost("a message over the size limit was refused".into());
                }
                Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                    return End::Lost("the connection closed".into());
                }
                Err(e) => return End::Lost(format!("read failed ({})", short_error(&e))),
            }
        }
        if requery_at.is_some_and(|t| t <= now) {
            requery_at = None;
            if let Err(e) = send_text(&mut link.ws, QUERY) {
                return End::Lost(e);
            }
        }
        if pong_by.is_some_and(|t| t <= now) {
            return End::Lost("the server stopped answering pings".into());
        }
        if waited == WAIT_TIMEOUT && ping_due <= now {
            ping_due = now + PING_EVERY;
            match link.ws.send(Message::Ping(Default::default())) {
                Ok(()) => pong_by = Some(now + PONG_WITHIN),
                Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    pong_by = Some(now + PONG_WITHIN)
                }
                Err(e) => return End::Lost(format!("ping failed ({})", short_error(&e))),
            }
        }
    }
}

fn short_error(e: &tungstenite::Error) -> &'static str {
    match e {
        tungstenite::Error::Io(_) => "i/o",
        tungstenite::Error::Protocol(_) => "protocol",
        tungstenite::Error::Utf8(_) => "text encoding",
        _ => "other",
    }
}

fn handle_text(text: &str, shared: &Arc<Shared>, out: &Publisher, requery_at: &mut Option<Instant>, now: Instant) {
    let Ok(json) = serde_json::from_str::<Json>(text) else {
        log::debug!("workspaces: a message that is not JSON was ignored");
        return;
    };
    match json.get("messageType").and_then(Json::as_str) {
        Some("client_response") => {
            let command = json.get("clientMessage").and_then(Json::as_str).unwrap_or("");
            let ok = json.get("success").and_then(Json::as_bool).unwrap_or(false);
            if !ok {
                let what = if command.starts_with("command") {
                    "command"
                } else if command.starts_with("sub") {
                    "subscription"
                } else {
                    "query"
                };
                log::warn!("workspaces: a {what} was refused by the window manager");
            } else if command == QUERY
                && let Some(data) = json.get("data")
            {
                publish_model(shared, out, from_glaze(data), ServiceStatus::Ok);
            }
        }
        // Astur's events carry no data and GlazeWM's carry deltas we do not read: re-query either way.
        Some("event_subscription") if requery_at.is_none() => *requery_at = Some(now + REQUERY_AFTER),
        _ => {}
    }
}

/// A workspace name safe to put in a text command: no spaces or control
/// characters (the protocol splits on whitespace).
pub(crate) fn command_name_ok(name: &str) -> bool {
    !name.is_empty() && !name.chars().any(|c| c.is_whitespace() || c.is_control())
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;

    #[test]
    fn only_a_loopback_websocket_address_is_accepted() {
        assert_eq!(parse_url("ws://127.0.0.1:6123"), Ok(6123));
        assert_eq!(parse_url("ws://127.0.0.1:6123/"), Ok(6123));
        assert!(parse_url("ws://localhost:6123").is_err());
        assert!(parse_url("ws://192.168.1.5:6123").is_err());
        assert!(parse_url("wss://127.0.0.1:6123").is_err());
        assert!(parse_url("ws://127.0.0.1").is_err());
        assert!(parse_url("ws://127.0.0.1:99999").is_err());
    }

    #[test]
    fn our_own_listener_passes_the_same_user_check() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let pids = listener_pids(port).unwrap();
        assert!(pids.contains(&std::process::id()), "{pids:?}");
        assert_eq!(check_listener(port), Ok(Some(false)));
    }

    #[test]
    fn an_ipv6_only_listener_is_found_too() {
        // Skipped on a machine without IPv6 loopback.
        let Ok(listener) = TcpListener::bind(("::1", 0)) else { return };
        let port = listener.local_addr().unwrap().port();
        let pids = listener_pids(port).unwrap();
        assert!(pids.contains(&std::process::id()), "{pids:?}");
        assert_eq!(check_listener(port), Ok(Some(false)));
    }

    #[test]
    fn a_port_nobody_listens_on_has_no_owner() {
        let port = {
            let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            l.local_addr().unwrap().port()
        };
        assert!(listener_pids(port).unwrap().is_empty());
        assert_eq!(check_listener(port), Ok(None), "no listener is reported as none, never as accepted");
    }

    #[test]
    fn workspace_names_in_commands_cannot_inject_text() {
        assert!(command_name_ok("1"));
        assert!(command_name_ok("web-2"));
        assert!(!command_name_ok(""));
        assert!(!command_name_ok("a b"));
        assert!(!command_name_ok(
            "a
query monitors"
        ));
        assert!(!command_name_ok("a	b"));
    }
}
