//! I/O a script asks for (exec, HTTP, files, opening a URL) and what comes
//! back. The script runtime checks permissions before sending a request; the
//! services crate runs it off every other thread, enforces the caps below,
//! and checks again what only it can see (each redirect hop, the final path
//! of an opened file) against the `IoGrant` that travels with the request.
//!
//! Error and result text never holds an absolute path or anything else from
//! the machine beyond what the widget itself asked for.

use std::path::PathBuf;
use std::time::Duration;

use crate::element::InstanceId;
use crate::permissions::{PathEnv, Permissions};

/// Unique per request for the life of the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IoId(pub u64);

pub mod limits {
    use std::time::Duration;

    pub const EXEC_DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
    pub const EXEC_MAX_TIMEOUT: Duration = Duration::from_secs(600);
    /// Per stream (stdout, stderr) of one `exec.run`.
    pub const EXEC_DEFAULT_OUTPUT: usize = 1 << 20;
    pub const EXEC_MAX_OUTPUT: usize = 8 << 20;
    /// Longer lines of a streamed process are cut (and marked) at this.
    pub const EXEC_MAX_LINE: usize = 64 << 10;
    /// Streamed lines delivered per second per process; more are dropped
    /// and counted.
    pub const EXEC_MAX_LINES_PER_SEC: u32 = 200;
    pub const EXEC_MAX_PROCESSES_PER_INSTANCE: usize = 8;
    /// Every widget together.
    pub const EXEC_MAX_PROCESSES_TOTAL: usize = 32;

    pub const HTTP_DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
    pub const HTTP_MAX_TIMEOUT: Duration = Duration::from_secs(60);
    pub const HTTP_DEFAULT_BYTES: usize = 2 << 20;
    pub const HTTP_MAX_BYTES: usize = 8 << 20;
    pub const HTTP_MAX_IN_FLIGHT_PER_INSTANCE: usize = 4;
    /// Worker threads; at most one request per host runs at a time, so a
    /// slow host delays only its own widgets.
    pub const HTTP_WORKERS: usize = 4;
    /// Redirect hops followed (each checked against the grant).
    pub const HTTP_MAX_REDIRECTS: usize = 5;
    /// Failures back off per URL: 1 s, doubling, at most this.
    pub const HTTP_MAX_BACKOFF: Duration = Duration::from_secs(300);
    pub const HTTP_CACHE_MAX_ENTRIES: usize = 64;
    pub const HTTP_CACHE_MAX_BYTES: usize = 8 << 20;
    /// Request headers a widget may add, and their total size.
    pub const HTTP_MAX_HEADERS: usize = 32;
    pub const HTTP_MAX_HEADER_BYTES: usize = 8 << 10;

    pub const FILE_DEFAULT_BYTES: usize = 1 << 20;
    pub const FILE_MAX_BYTES: usize = 8 << 20;
    pub const FILE_MAX_WATCHES_PER_INSTANCE: usize = 16;
    /// Every widget together.
    pub const FILE_MAX_WATCHES_TOTAL: usize = 64;
    /// File-change events closer together than this are merged.
    pub const FILE_WATCH_DEBOUNCE: Duration = Duration::from_millis(50);
    /// An editor still writing holds the file: retry the read this often,
    /// this far apart, on a sharing violation.
    pub const FILE_SHARING_RETRIES: u32 = 3;
    pub const FILE_SHARING_RETRY_DELAY: Duration = Duration::from_millis(20);

    /// `rsb.json.decode` input.
    pub const JSON_MAX_BYTES: usize = 2 << 20;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgramRef {
    /// Looked up on PATH only (never the current folder or the app folder),
    /// `.exe` only, added when missing. Permission entries name it the same
    /// way (`git`), compared without case and without `.exe`.
    Name(String),
    /// An absolute path to an `.exe`; its permission entry must be the same
    /// path.
    Path(PathBuf),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExecSpec {
    pub program: ProgramRef,
    /// Passed as an argument array, quoted for Windows by the services
    /// crate (the inverse of `split_command_line`).
    pub args: Vec<String>,
    /// Set only for `rsb.exec.shell`: the program is `cmd` and this text is
    /// appended after `/d /c ` exactly as written (cmd does its own parsing,
    /// so quoting it again would change the user's command).
    pub raw_tail: Option<String>,
    /// None = the program's own folder.
    pub cwd: Option<PathBuf>,
    /// `run`: whole run. `spawn`: None = until killed or the widget unloads.
    pub timeout: Option<Duration>,
    pub max_output: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    Get,
    Post,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HttpRequest {
    pub method: HttpMethod,
    /// `https://` (any permitted host) or `http://` (loopback only); already
    /// passed `permissions::parse_url` and `check_net`.
    pub url: String,
    /// Passed `check_headers`.
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
    pub max_bytes: usize,
    /// Serve a cached 2xx response younger than this without a request.
    /// The cache key is (widget file, method, URL, headers): one widget never
    /// gets a response another widget fetched.
    pub cache_for: Option<Duration>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    /// Lower-case names.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub from_cache: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileRequest {
    /// Absolute, already resolved, lexically checked and permission-checked.
    /// The services crate opens it, checks the handle's final path against
    /// the grant again and reads from that same handle.
    pub path: PathBuf,
    pub max_bytes: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum IoRequest {
    /// Run to completion, collect output.
    ExecRun(ExecSpec),
    /// Start and stream lines until exit, kill or unload.
    ExecSpawn(ExecSpec),
    Http(HttpRequest),
    ReadFile(FileRequest),
    /// Sends the content now and again after every change, until cancelled.
    WatchFile(FileRequest),
    /// Opens an `https://` URL in the default browser (`rsb.open_url`;
    /// needs `net` for its host). Tests use the fakes only.
    OpenUrl(String),
}

/// What the requesting widget is allowed, sent with every request so the
/// services crate can check what the script runtime cannot see: redirect
/// targets, and the final path of an opened file (junctions, links).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IoGrant {
    /// The widget ref text (`my/weather`), for messages and the HTTP cache key.
    pub widget: String,
    /// Already resolved (`Permissions::with_options`).
    pub permissions: Permissions,
    /// Absolute folder of the widget's file, always readable and the base of
    /// relative paths. Empty for a built-in and for a file directly in
    /// `widgets\`: such a widget has no folder of its own, because that
    /// folder would hold other widgets' files.
    pub widget_dir: PathBuf,
    pub env: PathEnv,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IoError {
    NotFound,
    /// Refused by the OS (access denied).
    Denied,
    /// Refused by the grant at a point only the services crate sees: a
    /// redirect to another host, a file whose final path is outside the
    /// grant. The text is the `Need` (`net:evil.example`).
    NotPermitted(String),
    Timeout,
    TooLarge {
        limit: usize,
    },
    /// Recent failures for this URL; try again after the delay.
    Backoff {
        retry_in: Duration,
    },
    /// Too many requests or processes for this widget, or for all widgets,
    /// at once.
    Busy,
    BadRequest(String),
    Network(String),
    Spawn(String),
    Io(String),
    Cancelled,
}

impl std::fmt::Display for IoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IoError::NotFound => f.write_str("not found"),
            IoError::Denied => f.write_str("access denied"),
            IoError::NotPermitted(need) => write!(f, "not permitted: needs {need}"),
            IoError::Timeout => f.write_str("timed out"),
            IoError::TooLarge { limit } => write!(f, "larger than {limit} bytes"),
            IoError::Backoff { retry_in } => write!(f, "failing; retry in {} s", retry_in.as_secs()),
            IoError::Busy => f.write_str("too many at once"),
            IoError::BadRequest(m) => write!(f, "bad request: {m}"),
            IoError::Network(m) => write!(f, "network: {m}"),
            IoError::Spawn(m) => write!(f, "cannot start: {m}"),
            IoError::Io(m) => write!(f, "{m}"),
            IoError::Cancelled => f.write_str("cancelled"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExecResult {
    /// None when killed or timed out.
    pub code: Option<i32>,
    /// Lossy UTF-8.
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    /// Output was cut at `max_output`.
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum IoEventKind {
    ExecLine {
        stderr: bool,
        line: String,
    },
    /// Sent after both output readers have finished, so it always follows
    /// the last `ExecLine`.
    ExecDone(Result<ExecResult, IoError>),
    Http(Result<HttpResponse, IoError>),
    /// Lossy UTF-8 text. For a watch: sent first and after every change;
    /// `Err(NotFound)` while the file is missing.
    File(Result<String, IoError>),
    /// `OpenUrl` handed to the shell (or refused).
    Opened(Result<(), IoError>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct IoEvent {
    pub id: IoId,
    pub owner: InstanceId,
    pub kind: IoEventKind,
}

/// Splits a command line into words by the Windows argument rules (the C
/// runtime's): whitespace separates words; `"` toggles quoting; `\` is
/// literal unless it precedes `"`, where 2n backslashes give n and start or
/// end quoting, and 2n+1 give n plus a literal `"`; inside quotes `""` is a
/// literal `"`. The first word is a program name: backslashes in it are
/// always literal. No shell features (`|`, `&`, `%VAR%`) are interpreted.
pub fn split_command_line(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut words = Vec::new();
    let mut i = 0;
    let is_space = |c: char| c == ' ' || c == '\t';
    // Program name.
    while i < chars.len() && is_space(chars[i]) {
        i += 1;
    }
    if i < chars.len() {
        let mut w = String::new();
        if chars[i] == '"' {
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                w.push(chars[i]);
                i += 1;
            }
            i += 1;
        }
        while i < chars.len() && !is_space(chars[i]) {
            if chars[i] != '"' {
                w.push(chars[i]);
            }
            i += 1;
        }
        words.push(w);
    }
    loop {
        while i < chars.len() && is_space(chars[i]) {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let mut w = String::new();
        let mut quoted = false;
        while i < chars.len() && (quoted || !is_space(chars[i])) {
            match chars[i] {
                '\\' => {
                    let start = i;
                    while i < chars.len() && chars[i] == '\\' {
                        i += 1;
                    }
                    let n = i - start;
                    if i < chars.len() && chars[i] == '"' {
                        w.extend(std::iter::repeat_n('\\', n / 2));
                        if n % 2 == 1 {
                            w.push('"');
                            i += 1;
                        }
                    } else {
                        w.extend(std::iter::repeat_n('\\', n));
                    }
                }
                '"' => {
                    if quoted && chars.get(i + 1) == Some(&'"') {
                        w.push('"');
                        i += 2;
                    } else {
                        quoted = !quoted;
                        i += 1;
                    }
                }
                c => {
                    w.push(c);
                    i += 1;
                }
            }
        }
        words.push(w);
    }
    words
}

/// Request headers a widget may not set: they would change how the request
/// is framed or routed, or which host it reaches.
pub const FORBIDDEN_HEADERS: &[&str] =
    &["host", "content-length", "transfer-encoding", "connection", "upgrade", "te", "trailer"];

/// Checks widget-supplied headers: token names, no CR, LF or NUL anywhere,
/// none of `FORBIDDEN_HEADERS` or `proxy-*`, within the count and size caps.
pub fn check_headers(headers: &[(String, String)]) -> Result<(), String> {
    if headers.len() > limits::HTTP_MAX_HEADERS {
        return Err(format!("more than {} headers", limits::HTTP_MAX_HEADERS));
    }
    let total: usize = headers.iter().map(|(k, v)| k.len() + v.len() + 4).sum();
    if total > limits::HTTP_MAX_HEADER_BYTES {
        return Err(format!("headers larger than {} bytes", limits::HTTP_MAX_HEADER_BYTES));
    }
    for (name, value) in headers {
        let token = |b: u8| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b);
        if name.is_empty() || !name.bytes().all(token) {
            return Err(format!("bad header name '{}'", name.escape_default()));
        }
        let lower = name.to_ascii_lowercase();
        if FORBIDDEN_HEADERS.contains(&lower.as_str()) || lower.starts_with("proxy-") {
            return Err(format!("the {name} header is set by rsb, not by widgets"));
        }
        if value.contains(['\r', '\n', '\0']) {
            return Err(format!("the {name} header has a line break"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn command_line_rules() {
        assert_eq!(split_command_line("git status --short"), s(&["git", "status", "--short"]));
        assert_eq!(split_command_line("  git   status "), s(&["git", "status"]));
        assert_eq!(split_command_line("\"C:\\Program Files\\x.exe\" a"), s(&["C:\\Program Files\\x.exe", "a"]));
        assert_eq!(split_command_line("x \"a b\" c"), s(&["x", "a b", "c"]));
        assert_eq!(split_command_line(r#"x a\\\"b"#), s(&["x", r#"a\"b"#]));
        assert_eq!(split_command_line(r#"x a\\"b c""#), s(&["x", r"a\b c"]));
        assert_eq!(split_command_line(r"x a\b\\c"), s(&["x", r"a\b\\c"]));
        assert_eq!(split_command_line(r#"x "a""b""#), s(&["x", r#"a"b"#]));
        assert_eq!(split_command_line(r#"x """#), s(&["x", ""]));
        assert_eq!(split_command_line("x a|b & c %PATH%"), s(&["x", "a|b", "&", "c", "%PATH%"]));
        assert_eq!(split_command_line(r"C:\dir\x.exe"), s(&[r"C:\dir\x.exe"]));
        assert!(split_command_line("   ").is_empty());
    }

    #[test]
    fn header_rules() {
        let h = |k: &str, v: &str| vec![(k.to_string(), v.to_string())];
        assert!(check_headers(&h("Authorization", "Bearer x")).is_ok());
        assert!(check_headers(&h("X-Api-Key", "abc")).is_ok());
        for (k, v) in [("Host", "x"), ("content-length", "1"), ("Proxy-Authorization", "x"), ("Connection", "close")] {
            assert!(check_headers(&h(k, v)).unwrap_err().contains("set by rsb"), "{k}");
        }
        assert!(check_headers(&h("X-A", "a\r\nHost: evil")).is_err());
        assert!(check_headers(&h("X A", "a")).is_err());
        assert!(check_headers(&h("", "a")).is_err());
        let many: Vec<(String, String)> = (0..33).map(|i| (format!("x-{i}"), "v".into())).collect();
        assert!(check_headers(&many).is_err());
        assert!(check_headers(&h("x", &"v".repeat(9000))).is_err());
    }
}
