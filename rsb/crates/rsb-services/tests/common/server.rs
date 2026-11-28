//! A small HTTP/1.1 server on 127.0.0.1 for the HTTP tests: records every
//! request, answers through a closure, and counts concurrent requests.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    /// Lower-case names.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

#[derive(Clone, Debug)]
pub enum Reply {
    Respond {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        delay: Duration,
        length: bool,
    },
    /// Accept, wait, then close without an answer.
    Hang(Duration),
    /// Close at once without an answer.
    Close,
}

impl Reply {
    pub fn ok(body: &str) -> Reply {
        Reply::Respond {
            status: 200,
            headers: Vec::new(),
            body: body.as_bytes().to_vec(),
            delay: Duration::ZERO,
            length: true,
        }
    }

    pub fn bytes(body: Vec<u8>) -> Reply {
        Reply::Respond { status: 200, headers: Vec::new(), body, delay: Duration::ZERO, length: true }
    }

    pub fn status(self, code: u16) -> Reply {
        match self {
            Reply::Respond { headers, body, delay, length, .. } => {
                Reply::Respond { status: code, headers, body, delay, length }
            }
            other => other,
        }
    }

    pub fn header(self, name: &str, value: &str) -> Reply {
        match self {
            Reply::Respond { status, mut headers, body, delay, length } => {
                headers.push((name.to_string(), value.to_string()));
                Reply::Respond { status, headers, body, delay, length }
            }
            other => other,
        }
    }

    pub fn delay(self, d: Duration) -> Reply {
        match self {
            Reply::Respond { status, headers, body, length, .. } => {
                Reply::Respond { status, headers, body, delay: d, length }
            }
            other => other,
        }
    }

    /// No Content-Length: the body ends when the connection closes.
    pub fn without_length(self) -> Reply {
        match self {
            Reply::Respond { status, headers, body, delay, .. } => {
                Reply::Respond { status, headers, body, delay, length: false }
            }
            other => other,
        }
    }

    pub fn redirect(code: u16, location: &str) -> Reply {
        Reply::ok("").status(code).header("Location", location)
    }
}

type Handler = dyn Fn(&Recorded) -> Reply + Send + Sync;

struct Inner {
    requests: Mutex<Vec<Recorded>>,
    handler: Box<Handler>,
    active: AtomicUsize,
    max_active: AtomicUsize,
    stop: AtomicBool,
}

pub struct Server {
    pub port: u16,
    inner: Arc<Inner>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start(handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static) -> Server {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind a loopback port");
        listener.set_nonblocking(true).expect("non-blocking listener");
        let port = listener.local_addr().expect("local address").port();
        let inner = Arc::new(Inner {
            requests: Mutex::new(Vec::new()),
            handler: Box::new(handler),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
        });
        let i = inner.clone();
        let thread = std::thread::spawn(move || {
            while !i.stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let i = i.clone();
                        std::thread::spawn(move || serve(&i, stream));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Server { port, inner, thread: Some(thread) }
    }

    /// `host:port` as a `net` permission entry names it.
    pub fn entry(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.inner.requests.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.inner.requests.lock().unwrap().len()
    }

    /// Most requests being answered at the same moment.
    pub fn max_concurrent(&self) -> usize {
        self.inner.max_active.load(Ordering::Acquire)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Recorded> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        let n = stream.read(&mut buf).ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&buf[..n]);
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let path = first.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let want: usize =
        headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let mut body = data[head_end + 4..].to_vec();
    while body.len() < want {
        let n = stream.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
    }
    Some(Recorded { method, path, headers, body })
}

fn serve(inner: &Arc<Inner>, mut stream: TcpStream) {
    // On Windows an accepted socket inherits the listener's non-blocking mode, and a read would fail at once.
    let _ = stream.set_nonblocking(false);
    let Some(request) = read_request(&mut stream) else { return };
    inner.requests.lock().unwrap().push(request.clone());
    let now = inner.active.fetch_add(1, Ordering::AcqRel) + 1;
    inner.max_active.fetch_max(now, Ordering::AcqRel);
    match (inner.handler)(&request) {
        Reply::Close => {}
        Reply::Hang(d) => std::thread::sleep(d),
        Reply::Respond { status, headers, body, delay, length } => {
            std::thread::sleep(delay);
            let mut head = format!("HTTP/1.1 {status} X\r\nConnection: close\r\n");
            if length {
                head.push_str(&format!("Content-Length: {}\r\n", body.len()));
            }
            for (k, v) in headers {
                head.push_str(&format!("{k}: {v}\r\n"));
            }
            head.push_str("\r\n");
            if stream.write_all(head.as_bytes()).is_ok() {
                let _ = stream.write_all(&body);
            }
        }
    }
    inner.active.fetch_sub(1, Ordering::AcqRel);
    let _ = stream.shutdown(std::net::Shutdown::Both);
}
