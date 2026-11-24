//! What a widget may do beyond drawing and reading services.
//!
//! A widget declares its needs in `permissions = { ... }`:
//!
//! - `exec = { "git", "C:/Tools/x.exe" }`: bare program names looked up on
//!   PATH (`.exe` only; `git` and `git.exe` are the same entry, case does not
//!   matter), or exact absolute paths to an `.exe`. `"$option:exec"` grants
//!   the program named by the widget's own `exec` option, but only when the
//!   user set that option in `bar.toml` (a widget's own default never grants
//!   anything): the first element of an array, or the first word of a string
//!   split by the Windows argument rules (`io::split_command_line`); no shell
//!   is involved. The built-in `text` widget uses this. Programs that can run
//!   any other program (`BROAD_PROGRAMS`: cmd, powershell, explorer, ...)
//!   are marked as such in the grant log line.
//! - `file = { "~/notes.txt", "%APPDATA%/app/*.json" }`: read and watch.
//!   Patterns start with `~`, `%APPDATA%`, `%LOCALAPPDATA%`, `%TEMP%` or a
//!   drive (`D:/data/*.csv`); `*` matches within one folder level, `**`
//!   across levels (never directly under a drive root). Network paths
//!   (`\\server\share`), device paths (`\\?\`, `\\.\`), alternate data
//!   streams (`a.txt:s`), short 8.3 names (`PROGRA~1`), trailing dots or
//!   spaces and device names (`NUL`, `COM1`) are refused, both in patterns
//!   and in the paths a widget asks for. A widget in its own folder
//!   (`widgets/<name>/`) can always read that folder; built-ins and files
//!   directly in `widgets\` have none. The services crate checks the final path of the opened file
//!   again (junctions and links), from the same handle it reads.
//! - `net = { "api.open-meteo.com", "*.example.org", "localhost:8080" }`:
//!   HTTPS hosts, lower case. A non-default port is part of the entry
//!   (`host:8443`); IP literals are listed exactly (`192.168.1.10`,
//!   `[::1]:3000`); a leading `*.` covers every host under the rest, so it
//!   is refused over a whole top-level domain or a public suffix such as
//!   `co.uk`; loopback entries (`localhost`, `127.0.0.1`, `[::1]`) must
//!   name a port. Plain `http://` is allowed only to loopback.
//! - `control = { "audio", "media", "workspaces" }`: service commands
//!   (change volume, pause media, switch workspace).
//! - `read = { "window", "media", "network" }`: services whose values are
//!   private (window titles, track names, Wi-Fi names). User widgets in the
//!   config folder get all three without declaring them (logged); built-ins
//!   declare what they use. Packages will need them declared and approved.
//!
//! An undeclared call fails, is logged, and shows a plate such as
//! `PERMISSION - my/weather needs net:api.open-meteo.com; add it to permissions`.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::io::{ProgramRef, split_command_line};
use crate::services::ServiceId;
use crate::value::Value;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Permissions {
    pub exec: Vec<String>,
    /// Full command lines (program first) that a user wrote in `bar.toml` for
    /// an `$option:` entry with arguments: only exactly these run.
    pub exec_exact: Vec<Vec<String>>,
    pub file: Vec<String>,
    /// Lower case, as written after validation.
    pub net: Vec<String>,
    pub control: Vec<ServiceId>,
    pub read: Vec<ServiceId>,
}

/// A missing permission, written the way the plate and docs write it:
/// `exec:git`, `file:~/notes.txt`, `net:api.open-meteo.com`, `control:audio`,
/// `read:window`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Need {
    Exec(String),
    File(String),
    Net(String),
    Control(ServiceId),
    Read(ServiceId),
}

impl fmt::Display for Need {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Need::Exec(p) => write!(f, "exec:{p}"),
            Need::File(p) => write!(f, "file:{p}"),
            Need::Net(h) => write!(f, "net:{h}"),
            Need::Control(s) => write!(f, "control:{}", s.name()),
            Need::Read(s) => write!(f, "read:{}", s.name()),
        }
    }
}

impl Need {
    /// The same need with an absolute program path shown relative to a known
    /// folder, so a log line does not carry the user name.
    pub fn folded(self, env: &PathEnv) -> Need {
        match self {
            Need::Exec(p) if is_absolute_text(&p) => Need::Exec(fold_known_folder(&p, env)),
            other => other,
        }
    }

    /// `"<widget> needs <need>; add it to permissions"`.
    pub fn message(&self, widget: &str) -> String {
        format!("{widget} needs {self}; add it to permissions")
    }
}

/// Folders the `file` patterns may name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathEnv {
    pub home: PathBuf,
    pub appdata: PathBuf,
    pub local_appdata: PathBuf,
    pub temp: PathBuf,
}

/// Services `control` may name.
pub const CONTROL_SERVICES: &[ServiceId] = &[ServiceId::Audio, ServiceId::Media, ServiceId::Workspaces];
/// Services `read` may name.
pub const READ_SERVICES: &[ServiceId] = &[ServiceId::Window, ServiceId::Media, ServiceId::Network];

/// Programs that can start any other program. Granting one is granting
/// everything; the grant log and the docs say so.
pub const BROAD_PROGRAMS: &[&str] = &[
    "cmd",
    "powershell",
    "pwsh",
    "explorer",
    "rundll32",
    "mshta",
    "wscript",
    "cscript",
    "conhost",
    "wsl",
    "bash",
    "start",
    // Not shells, but each runs arbitrary code from its arguments (git -c, python -c, ...).
    "git",
    "python",
    "python3",
    "py",
    "node",
    "forfiles",
    "schtasks",
    "wmic",
    "msiexec",
    "regsvr32",
    "certutil",
    "pcalua",
    "msbuild",
];

const DEVICE_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1", "lpt2",
    "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9", "conin$", "conout$",
];

fn is_device_name(segment: &str) -> bool {
    let lower = segment.to_ascii_lowercase();
    let stem = lower.split('.').next().unwrap_or("").trim_end();
    DEVICE_NAMES.contains(&stem)
}

/// `Git.EXE` -> `git`: how program names compare.
pub fn program_key(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    lower.strip_suffix(".exe").map(str::to_string).unwrap_or(lower)
}

/// True for the programs in `BROAD_PROGRAMS` (any case, `.exe` or not).
pub fn is_broad_program(name: &str) -> bool {
    BROAD_PROGRAMS.contains(&program_key(name).as_str())
}

/// A bare program name: no folders, no drive, no `..`, no device name, `.exe`
/// or no extension.
pub fn check_program_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("empty program name".into());
    }
    if name.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(format!("program name '{name}' has spaces or control characters"));
    }
    if name.contains(['/', '\\', ':']) || name.contains("..") {
        return Err(format!("'{name}' is not a bare program name; use a name found on PATH or a full path to an .exe"));
    }
    if is_device_name(name) {
        return Err(format!("'{name}' is a device name"));
    }
    if let Some((_, ext)) = name.rsplit_once('.')
        && !ext.eq_ignore_ascii_case("exe")
    {
        return Err(format!(
            "only .exe programs run; for '{name}' use {{\"cmd\", \"/d\", \"/c\", \"{name}\"}} and the exec permission \"cmd\""
        ));
    }
    Ok(())
}

/// Lexical checks shared by file patterns and requested paths. `pattern`
/// allows `*` and the `~` / `%VAR%` first segment.
pub fn check_path_text(text: &str, pattern: bool) -> Result<(), String> {
    if text.is_empty() {
        return Err("empty path".into());
    }
    if text.chars().any(char::is_control) {
        return Err("a path cannot hold control characters".into());
    }
    if text.starts_with("\\\\") || text.starts_with("//") || text.starts_with("\\/") || text.starts_with("/\\") {
        return Err("network and device paths (\\\\server, \\\\?\\, \\\\.\\) are not allowed".into());
    }
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b':' && !(i == 1 && bytes[0].is_ascii_alphabetic()) {
            return Err("':' is only allowed after a drive letter (no alternate data streams)".into());
        }
    }
    if !pattern && text.contains(['*', '?']) {
        return Err("a path cannot hold * or ?".into());
    }
    if text.contains(['<', '>', '"', '|']) || (pattern && text.contains('?')) {
        return Err("a path cannot hold < > \" | or ?".into());
    }
    for (i, seg) in text.split(['/', '\\']).enumerate() {
        if i == 0 && (seg == "~" || seg.starts_with('%') || (seg.len() == 2 && seg.ends_with(':'))) {
            continue;
        }
        if seg.is_empty() {
            if i == 0 {
                return Err("a path must start with ~, %APPDATA%, %LOCALAPPDATA%, %TEMP% or a drive".into());
            }
            continue;
        }
        if seg == "." || seg == ".." {
            return Err("'.' and '..' are not allowed in a path".into());
        }
        if seg.ends_with('.') || seg.ends_with(' ') {
            return Err(format!("'{seg}' ends with a dot or a space, which Windows drops"));
        }
        if is_device_name(seg) {
            return Err(format!("'{seg}' is a device name"));
        }
        let b = seg.as_bytes();
        if b.windows(2).any(|w| w[0] == b'~' && w[1].is_ascii_digit()) {
            return Err(format!("'{seg}' looks like a short (8.3) name; use the long name"));
        }
    }
    Ok(())
}

/// A URL a widget asked for, as far as permissions care.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedUrl {
    pub https: bool,
    /// Lower case, trailing dot removed; IPv6 literals keep their brackets.
    pub host: String,
    pub port: u16,
    /// The port was written in the URL.
    pub explicit_port: bool,
    /// Everything after the authority (`/v1/forecast?x=1`), `/` when empty.
    pub path_and_query: String,
}

impl ParsedUrl {
    pub fn default_port(&self) -> u16 {
        if self.https { 443 } else { 80 }
    }

    /// `host`, or `host:port` for a non-default port: what a `net` entry
    /// must say.
    pub fn authority(&self) -> String {
        if self.port == self.default_port() { self.host.clone() } else { format!("{}:{}", self.host, self.port) }
    }

    pub fn is_loopback(&self) -> bool {
        is_loopback_host(&self.host)
    }
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

fn check_host(host: &str) -> Result<(), String> {
    if host.is_empty() {
        return Err("no host".into());
    }
    if !host.is_ascii() {
        return Err("host names must be ASCII (write internationalised names in their xn-- form)".into());
    }
    if let Some(inner) = host.strip_prefix('[') {
        let inner = inner.strip_suffix(']').ok_or("unclosed '[' in host")?;
        if inner.is_empty() || !inner.bytes().all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.') {
            return Err(format!("bad IPv6 address '{host}'"));
        }
        return Ok(());
    }
    if host.len() > 253 {
        return Err("host name too long".into());
    }
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(format!("bad host name '{host}'"));
        }
        if !label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_') {
            return Err(format!("bad host name '{host}'"));
        }
    }
    // Other spellings of an address (2130706433, 0x7f.1, 127.1) reach hosts a name check would not expect.
    let labels: Vec<&str> = host.split('.').collect();
    if labels.iter().any(|l| l.starts_with("0x")) {
        return Err(format!("'{host}' is not a host name or a dotted IPv4 address"));
    }
    if labels.iter().all(|l| is_numeric_label(l)) && !is_dotted_quad(&labels) {
        return Err(format!("'{host}' is not a dotted IPv4 address like 192.168.1.10"));
    }
    Ok(())
}

fn is_numeric_label(label: &str) -> bool {
    !label.is_empty() && label.bytes().all(|b| b.is_ascii_digit())
}

fn is_dotted_quad(labels: &[&str]) -> bool {
    labels.len() == 4
        && labels.iter().all(|l| {
            is_numeric_label(l) && (l.len() == 1 || !l.starts_with('0')) && l.parse::<u16>().is_ok_and(|n| n <= 255)
        })
}

/// Splits `host[:port]` (host already lower case). IPv6 in brackets.
fn split_host_port(auth: &str) -> Result<(String, Option<u16>), String> {
    let (host, port) = if auth.starts_with('[') {
        match auth.find(']') {
            Some(end) => {
                let rest = &auth[end + 1..];
                let port = match rest.strip_prefix(':') {
                    Some(p) => Some(p),
                    None if rest.is_empty() => None,
                    None => return Err(format!("bad host '{auth}'")),
                };
                (&auth[..=end], port)
            }
            None => return Err("unclosed '[' in host".into()),
        }
    } else {
        match auth.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (auth, None),
        }
    };
    let port = match port {
        None => None,
        Some(p) => match p.parse::<u16>() {
            Ok(n) if n > 0 && p.bytes().all(|b| b.is_ascii_digit()) => Some(n),
            _ => return Err(format!("bad port '{p}'")),
        },
    };
    let host = host.strip_suffix('.').unwrap_or(host);
    check_host(host)?;
    Ok((host.to_string(), port))
}

/// Strict URL parsing for permission checks. Refuses what URL parsers
/// disagree on: user info (`@`), backslashes, `%` escapes in the host,
/// spaces and control characters anywhere, non-ASCII hosts, and plain
/// `http://` to anything but loopback.
pub fn parse_url(url: &str) -> Result<ParsedUrl, String> {
    if url.len() > 8192 {
        return Err("URL longer than 8192 bytes".into());
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("URL has spaces or control characters".into());
    }
    if url.contains('\\') {
        return Err("URL has a backslash; use /".into());
    }
    let starts = |p: &str| url.get(..p.len()).is_some_and(|h| h.eq_ignore_ascii_case(p));
    let (https, rest) = if starts("https://") {
        (true, &url[8..])
    } else if starts("http://") {
        (false, &url[7..])
    } else {
        return Err("URL must start with https:// (or http:// for localhost)".into());
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (auth, tail) = rest.split_at(end);
    if auth.contains('@') {
        return Err("URL user names and passwords ('@') are not allowed".into());
    }
    if auth.contains('%') {
        return Err("'%' escapes are not allowed in the host".into());
    }
    let (host, port) = split_host_port(&auth.to_ascii_lowercase())?;
    let explicit_port = port.is_some();
    let port = port.unwrap_or(if https { 443 } else { 80 });
    if !https && !is_loopback_host(&host) {
        return Err("plain http:// is only allowed to localhost; use https://".into());
    }
    let tail = tail.split('#').next().unwrap_or("");
    let path_and_query = if tail.is_empty() {
        "/".to_string()
    } else if tail.starts_with('?') {
        format!("/{tail}")
    } else {
        tail.to_string()
    };
    Ok(ParsedUrl { https, host, port, explicit_port, path_and_query })
}

/// An address handed to the shell must not carry characters that a browser
/// launcher may read as command-line syntax. Spaces, control characters and
/// backslashes are already refused by `parse_url`.
pub fn open_url_text_ok(url: &str) -> Result<(), String> {
    match url.chars().find(|c| matches!(c, '"' | '\'' | '<' | '>' | '^' | '`' | '{' | '}' | '|')) {
        Some(c) => Err(format!("the address has '{c}'; write it as a % escape")),
        None => Ok(()),
    }
}

/// One `net` entry: `host`, `*.domain`, `host:port`, `[v6]:port`.
/// Two-label public suffixes that a `*.` entry may not stand directly in
/// front of: the entry would grant every registrant under them. Not the
/// whole public suffix list, only the common ones.
const PUBLIC_SUFFIXES: &[&str] = &[
    "co.uk",
    "org.uk",
    "ac.uk",
    "gov.uk",
    "me.uk",
    "ltd.uk",
    "plc.uk",
    "net.uk",
    "sch.uk",
    "com.au",
    "net.au",
    "org.au",
    "edu.au",
    "gov.au",
    "asn.au",
    "id.au",
    "co.nz",
    "org.nz",
    "net.nz",
    "govt.nz",
    "ac.nz",
    "school.nz",
    "co.jp",
    "or.jp",
    "ne.jp",
    "ac.jp",
    "go.jp",
    "co.in",
    "net.in",
    "org.in",
    "co.za",
    "org.za",
    "com.br",
    "com.cn",
    "com.mx",
    "com.sg",
    "com.hk",
    "co.kr",
    "com.tr",
    "com.ar",
    "co.id",
    "co.il",
];

fn parse_net_entry(entry: &str) -> Result<(String, Option<u16>), String> {
    let lower = entry.to_ascii_lowercase();
    if lower.contains(['/', '@', '%', '\\']) || lower.contains("://") {
        return Err(format!("'{entry}' must be a host name like \"api.example.com\", without https:// or a path"));
    }
    let (host, port) = match lower.strip_prefix("*.") {
        Some(rest) => {
            let (h, p) = split_host_port(rest)?;
            (format!("*.{h}"), p)
        }
        None => split_host_port(&lower)?,
    };
    if host == "*" || host.contains('*') && !host.starts_with("*.") {
        return Err(format!("'{entry}': only a leading '*.' is allowed"));
    }
    if let Some(domain) = host.strip_prefix("*.") {
        let labels: Vec<&str> = domain.split('.').collect();
        if domain.starts_with('[') || labels.len() < 2 {
            return Err(format!(
                "'{entry}' would cover a whole top-level domain; name a domain like \"*.example.com\""
            ));
        }
        if PUBLIC_SUFFIXES.contains(&domain) {
            return Err(format!(
                "'{entry}' would cover every site under a public suffix; name a domain like \"*.example.{domain}\""
            ));
        }
        if labels.iter().all(|l| is_numeric_label(l)) {
            return Err(format!("'{entry}': a wildcard cannot be used with an IP address; list the address"));
        }
    }
    if is_loopback_host(&host) && port.is_none() {
        return Err(format!("'{entry}' must name a port, e.g. \"{host}:8080\""));
    }
    Ok((host, port))
}

fn file_prefix(first: &str, env: &PathEnv) -> Option<PathBuf> {
    if first == "~" {
        return Some(env.home.clone());
    }
    match first.to_ascii_uppercase().as_str() {
        "%APPDATA%" => Some(env.appdata.clone()),
        "%LOCALAPPDATA%" => Some(env.local_appdata.clone()),
        "%TEMP%" => Some(env.temp.clone()),
        d if d.len() == 2 && d.ends_with(':') && d.as_bytes()[0].is_ascii_alphabetic() => {
            Some(PathBuf::from(format!("{d}\\")))
        }
        _ => None,
    }
}

/// Lower-case path components, drive first (`c:`), separators dropped.
fn components(path: &Path) -> Vec<String> {
    path.to_string_lossy().split(['/', '\\']).filter(|s| !s.is_empty()).map(str::to_lowercase).collect()
}

fn segment_matches(pat: &str, seg: &str) -> bool {
    match pat.split_once('*') {
        None => pat == seg,
        Some((head, rest)) => {
            let Some(after) = seg.strip_prefix(head) else { return false };
            if rest.is_empty() {
                return true;
            }
            (0..=after.len()).filter(|&i| after.is_char_boundary(i)).any(|i| segment_matches(rest, &after[i..]))
        }
    }
}

fn parts_match(pats: &[String], parts: &[String]) -> bool {
    match pats.split_first() {
        None => parts.is_empty(),
        Some((p, rest)) if p == "**" => (0..=parts.len()).any(|i| parts_match(rest, &parts[i..])),
        Some((p, rest)) => match parts.split_first() {
            Some((s, more)) => segment_matches(p, s) && parts_match(rest, more),
            None => false,
        },
    }
}

impl Permissions {
    /// From the Luau table (as a `Value`). Each key takes a list of strings
    /// (a single string is a list of one). Unknown keys and bad entries are
    /// errors naming the key and entry: `permissions.net[1]: ...`.
    pub fn from_value(v: &Value) -> Result<Permissions, String> {
        let map = match v {
            Value::Nil => return Ok(Permissions::default()),
            Value::Map(m) => m,
            Value::List(l) if l.is_empty() => return Ok(Permissions::default()),
            other => return Err(format!("permissions must be a table, got {}", other.type_name())),
        };
        let mut p = Permissions::default();
        for (key, val) in map {
            if !matches!(key.as_str(), "exec" | "file" | "net" | "control" | "read") {
                return Err(format!("unknown permission '{key}' (exec, file, net, control, read)"));
            }
            let items: Vec<&str> = match val {
                Value::Str(s) => vec![s.as_str()],
                Value::List(l) => l
                    .iter()
                    .enumerate()
                    .map(|(i, x)| x.as_str().ok_or_else(|| format!("permissions.{key}[{}] must be a string", i + 1)))
                    .collect::<Result<_, _>>()?,
                Value::Map(m) if m.is_empty() => Vec::new(),
                other => return Err(format!("permissions.{key} must be a list of strings, got {}", other.type_name())),
            };
            for (i, item) in items.into_iter().enumerate() {
                let at = |e: String| format!("permissions.{key}[{}]: {e}", i + 1);
                match key.as_str() {
                    "exec" => {
                        if let Some(opt) = item.strip_prefix("$option:") {
                            if opt.is_empty()
                                || !opt.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                            {
                                return Err(at(format!("'{item}': option names use a-z, 0-9 and _")));
                            }
                        } else if is_absolute_text(item) {
                            check_path_text(item, false).map_err(at)?;
                            if !item.to_ascii_lowercase().ends_with(".exe") {
                                return Err(at(format!("'{item}' must be an .exe")));
                            }
                        } else {
                            check_program_name(item).map_err(at)?;
                        }
                        p.exec.push(item.to_string());
                    }
                    "file" => {
                        check_path_text(item, true).map_err(at)?;
                        let mut segs = item.split(['/', '\\']);
                        let first = segs.next().unwrap_or("");
                        let known = first == "~"
                            || ["%APPDATA%", "%LOCALAPPDATA%", "%TEMP%"].iter().any(|v| first.eq_ignore_ascii_case(v))
                            || (first.len() == 2 && first.ends_with(':'));
                        if !known {
                            return Err(at(format!(
                                "'{item}' must start with ~, %APPDATA%, %LOCALAPPDATA%, %TEMP% or a drive like D:"
                            )));
                        }
                        let second = segs.find(|s| !s.is_empty());
                        if first.ends_with(':') && second.is_none_or(|s| s.contains('*')) {
                            return Err(at(format!("'{item}' would allow a whole drive; name a folder")));
                        }
                        p.file.push(item.to_string());
                    }
                    "net" => {
                        let (host, port) = parse_net_entry(item).map_err(at)?;
                        p.net.push(match port {
                            Some(n) => format!("{host}:{n}"),
                            None => host,
                        });
                    }
                    "control" | "read" => {
                        let allowed = if key == "control" { CONTROL_SERVICES } else { READ_SERVICES };
                        let id = ServiceId::from_name(item).filter(|id| allowed.contains(id)).ok_or_else(|| {
                            let names: Vec<&str> = allowed.iter().map(|s| s.name()).collect();
                            at(format!("'{item}' is not one of {}", names.join(", ")))
                        })?;
                        let list = if key == "control" { &mut p.control } else { &mut p.read };
                        if !list.contains(&id) {
                            list.push(id);
                        }
                    }
                    other => {
                        return Err(format!("unknown permission '{other}' (exec, file, net, control, read)"));
                    }
                }
            }
        }
        Ok(p)
    }

    /// Replaces each `$option:<key>` exec entry with the program the user
    /// set for that option in `bar.toml` (`user_option(key)`, None when the
    /// user did not set it: then nothing is granted). Returns the resolved
    /// permissions and one note per resolved option for the grant log
    /// (`$option:exec = git`).
    ///
    /// An option that is only a program (`"git"`) grants that program with
    /// any arguments; one with arguments (`"git status"`) grants exactly
    /// that command line, so a user who writes a command does not hand the
    /// widget the whole program.
    pub fn with_options(
        &self,
        env: &PathEnv,
        user_option: &dyn Fn(&str) -> Option<Value>,
    ) -> (Permissions, Vec<String>) {
        let mut out = self.clone();
        let mut notes = Vec::new();
        out.exec.clear();
        for e in &self.exec {
            let Some(key) = e.strip_prefix("$option:") else {
                out.exec.push(e.clone());
                continue;
            };
            let argv: Vec<String> = match user_option(key) {
                Some(Value::Str(s)) => split_command_line(&s),
                Some(Value::List(l)) => l.iter().map_while(|v| v.as_str().map(str::to_string)).collect(),
                _ => Vec::new(),
            };
            let Some(program) = argv.first().cloned() else { continue };
            let ok = if is_absolute_text(&program) {
                check_path_text(&program, false).is_ok() && program.to_ascii_lowercase().ends_with(".exe")
            } else {
                check_program_name(&program).is_ok()
            };
            let shown = fold_known_folder(&program, env);
            if ok && argv.len() == 1 {
                notes.push(format!("{e} = {shown}"));
                if !out.exec.contains(&program) {
                    out.exec.push(program);
                }
            } else if ok {
                notes.push(format!("{e} = {shown} with {} argument(s), only that command line", argv.len() - 1));
                out.exec_exact.push(argv);
            } else {
                notes.push(format!("{e} not granted: '{shown}' is not a program name"));
            }
        }
        (out, notes)
    }

    pub fn check_exec(&self, program: &ProgramRef) -> Result<(), Need> {
        match program {
            ProgramRef::Name(name) => {
                let key = program_key(name);
                let ok = check_program_name(name).is_ok()
                    && self.exec.iter().any(|e| !e.starts_with('$') && !is_absolute_text(e) && program_key(e) == key);
                if ok { Ok(()) } else { Err(Need::Exec(key)) }
            }
            ProgramRef::Path(path) => {
                let want = normalise_path_text(&path.to_string_lossy());
                let ok = self.exec.iter().any(|e| is_absolute_text(e) && normalise_path_text(e) == want);
                if ok { Ok(()) } else { Err(Need::Exec(path.to_string_lossy().replace('\\', "/"))) }
            }
        }
    }

    /// `check_exec` plus the command lines a user fixed in `bar.toml`.
    /// `args` is None when the call is a raw command line (`rsb.exec.shell`),
    /// which only a grant without argument limits can allow.
    pub fn check_exec_args(&self, program: &ProgramRef, args: Option<&[String]>) -> Result<(), Need> {
        let denied = match self.check_exec(program) {
            Ok(()) => return Ok(()),
            Err(need) => need,
        };
        let Some(args) = args else { return Err(denied) };
        let same_program = |first: &str| match program {
            ProgramRef::Name(n) => !is_absolute_text(first) && program_key(first) == program_key(n),
            ProgramRef::Path(p) => {
                is_absolute_text(first) && normalise_path_text(first) == normalise_path_text(&p.to_string_lossy())
            }
        };
        let mut named = false;
        for line in &self.exec_exact {
            let Some((first, rest)) = line.split_first() else { continue };
            if same_program(first) {
                if rest == args {
                    return Ok(());
                }
                named = true;
            }
        }
        match (named, denied) {
            (true, Need::Exec(p)) => Err(Need::Exec(format!("{p} (only the arguments set in bar.toml)"))),
            (_, other) => Err(other),
        }
    }

    /// `url` comes from `parse_url`.
    pub fn check_net(&self, url: &ParsedUrl) -> Result<(), Need> {
        let ok = self.net.iter().any(|e| {
            let Ok((host, port)) = parse_net_entry(e) else { return false };
            let port_ok = port.unwrap_or(url.default_port()) == url.port;
            let host_ok = match host.strip_prefix("*.") {
                Some(domain) => url.host.len() > domain.len() + 1 && url.host.ends_with(&format!(".{domain}")),
                None => host == url.host,
            };
            port_ok && host_ok
        });
        if ok { Ok(()) } else { Err(Need::Net(url.authority())) }
    }

    /// `path` must already be absolute and normalised (no `..`). Refused
    /// outright when it fails `check_path_text`.
    pub fn check_file(&self, path: &Path, widget_dir: &Path, env: &PathEnv) -> Result<(), Need> {
        let text = path.to_string_lossy();
        let need = || Need::File(display_path(path, env));
        if check_path_text(&text, false).is_err() || !is_absolute_text(&text) {
            return Err(need());
        }
        let parts = components(path);
        let dir = components(widget_dir);
        if !dir.is_empty() && parts.len() >= dir.len() && parts[..dir.len()] == dir[..] {
            return Ok(());
        }
        for pattern in &self.file {
            let mut segs = pattern.split(['/', '\\']).filter(|s| !s.is_empty());
            let Some(base) = segs.next().and_then(|f| file_prefix(f, env)) else { continue };
            let mut pats = components(&base);
            pats.extend(segs.map(str::to_lowercase));
            if parts_match(&pats, &parts) {
                return Ok(());
            }
        }
        Err(need())
    }

    pub fn check_control(&self, service: ServiceId) -> Result<(), Need> {
        if self.control.contains(&service) { Ok(()) } else { Err(Need::Control(service)) }
    }

    /// Reading `window`, `media` or `network` values. Other services need
    /// nothing.
    pub fn check_read(&self, service: ServiceId) -> Result<(), Need> {
        if !READ_SERVICES.contains(&service) || self.read.contains(&service) {
            Ok(())
        } else {
            Err(Need::Read(service))
        }
    }

    /// What user widgets in the config folder get without declaring it.
    pub fn grant_user_defaults(&mut self) {
        for s in READ_SERVICES {
            if !self.read.contains(s) {
                self.read.push(*s);
            }
        }
    }

    /// The granted list for the log line `granted my/weather: <this>`, e.g.
    /// `net:api.open-meteo.com, exec:cmd (can run any program)`; `nothing`
    /// when empty.
    pub fn describe(&self) -> String {
        self.describe_in(None)
    }

    /// `describe`, with program paths shown relative to the known folders
    /// when `env` is given.
    pub fn describe_in(&self, env: Option<&PathEnv>) -> String {
        let show = |t: &str| match env {
            Some(env) if is_absolute_text(t) => fold_known_folder(t, env),
            _ => t.to_string(),
        };
        let mut parts = Vec::new();
        for e in &self.exec {
            if is_broad_program(e) {
                parts.push(format!("exec:{} (can run any program)", show(e)));
            } else {
                parts.push(format!("exec:{} (any arguments)", show(e)));
            }
        }
        for line in &self.exec_exact {
            let shown: Vec<String> = line.iter().map(|w| show(w)).collect();
            parts.push(format!("exec:{} (only these arguments)", shown.join(" ")));
        }
        parts.extend(self.file.iter().map(|f| {
            // The grant as typed, and as it reads once the home folder and
            // the other known folders are folded: `C:/Users/Pat/**` is `~/**`
            // for the user whose home that is.
            let broad = reads_every_file_below(f)
                || env.is_some_and(|env| is_absolute_text(f) && reads_every_file_below(&fold_known_folder(f, env)));
            if broad { format!("file:{f} (can read every file there)") } else { format!("file:{f}") }
        }));
        parts.extend(self.net.iter().map(|n| format!("net:{n}")));
        parts.extend(self.control.iter().map(|s| format!("control:{}", s.name())));
        parts.extend(self.read.iter().map(|s| format!("read:{}", s.name())));
        // exec and click may name the same program: say it once.
        let mut seen = std::collections::HashSet::new();
        parts.retain(|p| seen.insert(p.clone()));
        if parts.is_empty() { "nothing".into() } else { parts.join(", ") }
    }
}

/// A pattern that is a known folder (or `C:/Users`, or `~/AppData`, which
/// holds `%APPDATA%` and `%LOCALAPPDATA%`) followed only by wildcard levels
/// and then `**` reaches every file below that folder, one level in
/// (`~/*/**`) or not. A wildcard level is any segment made only of `*` and
/// `.` (`*`, `*.*`, `.*`): `~/*.*/**` is every dot folder, `.ssh` and `.aws`
/// among them. Under `C:/Users` the level after it is a user's home folder
/// (named or wildcarded), so `C:/Users/Pat/**` and `C:/Users/*/AppData/**`
/// read a whole profile just as `~/**` and `~/AppData/**` do.
fn reads_every_file_below(pattern: &str) -> bool {
    let mut segs = pattern.split(['/', '\\']).filter(|s| !s.is_empty());
    let first = segs.next().unwrap_or("").to_ascii_lowercase();
    let mut rest: Vec<&str> = segs.collect();
    let known = ["~", "%appdata%", "%localappdata%", "%temp%"].contains(&first.as_str());
    let users =
        first.len() == 2 && first.ends_with(':') && rest.first().is_some_and(|s| s.eq_ignore_ascii_case("users"));
    if users {
        rest.remove(0);
        if rest.first().is_some_and(|s| *s != "**") {
            rest.remove(0);
            strip_appdata(&mut rest);
        }
    } else if !known {
        return false;
    } else if first == "~" {
        strip_appdata(&mut rest);
    }
    rest.iter().find(|s| !is_wildcard_level(s)).is_some_and(|s| *s == "**")
}

/// `AppData`, `AppData/Local`, `AppData/Roaming`, `AppData/LocalLow` and
/// `AppData/Local/Temp` under the home folder are the known folders by
/// another name: drop them so the rest is judged the same way.
fn strip_appdata(rest: &mut Vec<&str>) {
    let is = |rest: &Vec<&str>, name: &str| rest.first().is_some_and(|s| s.eq_ignore_ascii_case(name));
    if !is(rest, "appdata") {
        return;
    }
    rest.remove(0);
    if is(rest, "local") {
        rest.remove(0);
        if is(rest, "temp") {
            rest.remove(0);
        }
    } else if is(rest, "roaming") || is(rest, "locallow") {
        rest.remove(0);
    }
}

fn is_wildcard_level(seg: &str) -> bool {
    seg != "**" && seg.contains('*') && seg.chars().all(|c| c == '*' || c == '.')
}

fn is_absolute_text(t: &str) -> bool {
    let b = t.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\')
}

fn normalise_path_text(t: &str) -> String {
    t.replace('/', "\\").to_lowercase()
}

/// A path as a plate shows it: `~/...` under the home folder, else with `/`.
fn display_path(path: &Path, env: &PathEnv) -> String {
    let parts = components(path);
    let home = components(&env.home);
    if !home.is_empty() && parts.len() > home.len() && parts[..home.len()] == home[..] {
        let original: Vec<String> =
            path.to_string_lossy().split(['/', '\\']).filter(|s| !s.is_empty()).map(str::to_string).collect();
        return format!("~/{}", original[home.len()..].join("/"));
    }
    path.to_string_lossy().replace('\\', "/")
}

/// `C:/Users/me/AppData/Local/Temp/x.exe` -> `%TEMP%/x.exe`, and likewise
/// for the other known folders; any other path keeps its text with `/`.
pub fn fold_known_folder(text: &str, env: &PathEnv) -> String {
    let lower = |s: &str| s.replace('/', "\\").to_lowercase();
    let full = lower(text);
    for (prefix, label) in
        [(&env.temp, "%TEMP%"), (&env.local_appdata, "%LOCALAPPDATA%"), (&env.appdata, "%APPDATA%"), (&env.home, "~")]
    {
        let base = lower(&prefix.to_string_lossy());
        let base = base.trim_end_matches('\\');
        if !base.is_empty() && full.len() > base.len() && full.starts_with(base) && full.as_bytes()[base.len()] == b'\\'
        {
            let original = text.replace('\\', "/");
            let rest = original.get(base.len()..).unwrap_or("").trim_start_matches('/');
            return format!("{label}/{rest}");
        }
    }
    text.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn list(items: &[&str]) -> Value {
        Value::List(items.iter().map(|s| Value::Str(s.to_string())).collect())
    }

    fn perms(pairs: &[(&str, &[&str])]) -> Result<Permissions, String> {
        let m: BTreeMap<String, Value> = pairs.iter().map(|(k, v)| (k.to_string(), list(v))).collect();
        Permissions::from_value(&Value::Map(m))
    }

    fn env() -> PathEnv {
        PathEnv {
            home: PathBuf::from("C:\\Users\\Pat"),
            appdata: PathBuf::from("C:\\Users\\Pat\\AppData\\Roaming"),
            local_appdata: PathBuf::from("C:\\Users\\Pat\\AppData\\Local"),
            temp: PathBuf::from("C:\\Users\\Pat\\AppData\\Local\\Temp"),
        }
    }

    #[test]
    fn exec_names_and_paths() {
        let p = perms(&[("exec", &["git", "C:/Tools/x.exe"])]).unwrap();
        assert!(p.check_exec(&ProgramRef::Name("git".into())).is_ok());
        assert!(p.check_exec(&ProgramRef::Name("GIT.exe".into())).is_ok());
        assert_eq!(p.check_exec(&ProgramRef::Name("curl".into())), Err(Need::Exec("curl".into())));
        assert!(p.check_exec(&ProgramRef::Path(PathBuf::from("c:\\tools\\X.EXE"))).is_ok());
        assert!(p.check_exec(&ProgramRef::Path(PathBuf::from("C:\\Tools\\y.exe"))).is_err());
        assert!(p.check_exec(&ProgramRef::Name("..\\git".into())).is_err());
        for bad in ["git.bat", "a/b", "nul", "x:y", "C:/x.bat", "\\\\srv\\x.exe", "two words", ""] {
            assert!(perms(&[("exec", &[bad])]).is_err(), "{bad}");
        }
        assert!(perms(&[("exec", &["git.bat"])]).unwrap_err().contains("cmd"));
    }

    #[test]
    fn option_programs_need_a_user_value() {
        let p = perms(&[("exec", &["$option:exec", "$option:click"])]).unwrap();
        let user = |k: &str| match k {
            "exec" => Some(Value::Str("\"C:/Program Files/Git/bin/git.exe\" status --short".into())),
            "click" => None,
            _ => None,
        };
        let (r, notes) = p.with_options(&env(), &user);
        assert!(r.exec.is_empty(), "arguments were written, so only that command line is granted");
        assert_eq!(
            r.exec_exact,
            vec![vec!["C:/Program Files/Git/bin/git.exe".to_string(), "status".into(), "--short".into()]]
        );
        assert_eq!(notes.len(), 1);
        let (r, _) = p.with_options(&env(), &|_| Some(Value::List(vec![Value::Str("whoami".into())])));
        assert!(r.check_exec(&ProgramRef::Name("whoami".into())).is_ok());
        let (r, notes) = p.with_options(&env(), &|_| Some(Value::Str("..\\evil".into())));
        assert!(r.exec.is_empty() && notes[0].contains("not granted"));
        assert!(perms(&[("exec", &["$option:Bad"])]).is_err());
    }

    #[test]
    fn an_option_with_arguments_grants_only_that_command_line() {
        let p = perms(&[("exec", &["$option:exec"])]).unwrap();
        let word = |s: &str| {
            let s = s.to_string();
            move |_: &str| Some(Value::Str(s.clone()))
        };
        let (r, notes) = p.with_options(&env(), &word("git status --short"));
        assert!(r.exec.is_empty(), "no blanket grant for git");
        let git = ProgramRef::Name("git".into());
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(r.check_exec_args(&git, Some(&args(&["status", "--short"]))).is_ok());
        assert!(r.check_exec_args(&git, Some(&args(&["-c", "core.pager=calc", "log"]))).is_err());
        assert!(r.check_exec_args(&git, Some(&args(&[]))).is_err());
        assert!(r.check_exec_args(&git, None).is_err(), "a raw command line never matches");
        let Err(Need::Exec(why)) = r.check_exec_args(&git, Some(&args(&["log"]))) else { panic!() };
        assert!(why.contains("only the arguments set in bar.toml"), "{why}");
        assert!(notes[0].contains("only that command line"), "{notes:?}");
        assert!(r.describe().contains("exec:git status --short (only these arguments)"), "{}", r.describe());
        // A program on its own is still the whole program, any arguments.
        let (r, _) = p.with_options(&env(), &word("whoami"));
        assert!(r.check_exec_args(&ProgramRef::Name("whoami".into()), Some(&args(&["/all"]))).is_ok());
        // A list works like the string form.
        let (r, _) =
            p.with_options(&env(), &|_| Some(Value::List(vec![Value::Str("git".into()), Value::Str("log".into())])));
        assert!(r.check_exec_args(&git, Some(&args(&["log"]))).is_ok());
        assert!(r.check_exec_args(&git, Some(&args(&["log", "-p"]))).is_err());
    }

    #[test]
    fn the_same_grant_is_listed_once() {
        let p = perms(&[("exec", &["$option:exec", "$option:click"])]).unwrap();
        let (r, _) = p.with_options(&env(), &|_| Some(Value::Str("cmd".into())));
        assert_eq!(r.describe(), "exec:cmd (can run any program)");
    }

    #[test]
    fn programs_that_run_code_from_their_arguments_are_marked_broad() {
        for name in ["git", "python", "node", "certutil", "msbuild", "regsvr32", "WMIC.exe"] {
            assert!(is_broad_program(name), "{name}");
        }
        assert!(!is_broad_program("whoami"));
        let p = perms(&[("exec", &["git", "whoami"])]).unwrap();
        assert_eq!(p.describe(), "exec:git (can run any program), exec:whoami (any arguments)");
    }

    #[test]
    fn program_paths_in_logs_and_needs_are_folded_to_known_folders() {
        let e = env();
        assert_eq!(fold_known_folder("C:/Users/Pat/AppData/Local/Temp/x.exe", &e), "%TEMP%/x.exe");
        assert_eq!(fold_known_folder("c:/USERS/pat/tools/y.exe", &e), "~/tools/y.exe");
        assert_eq!(fold_known_folder("D:\\Tools\\z.exe", &e), "D:/Tools/z.exe");
        let p = perms(&[("exec", &["C:/Users/Pat/tools/y.exe"])]).unwrap();
        assert_eq!(p.describe_in(Some(&e)), "exec:~/tools/y.exe (any arguments)");
        let need = Need::Exec("C:/Users/Pat/tools/y.exe".into()).folded(&e);
        assert_eq!(need.to_string(), "exec:~/tools/y.exe");
        let (_, notes) = perms(&[("exec", &["$option:exec"])])
            .unwrap()
            .with_options(&e, &|_| Some(Value::Str("\"C:/Users/Pat/tools/y.exe\"".into())));
        assert_eq!(notes, vec!["$option:exec = ~/tools/y.exe".to_string()]);
    }

    #[test]
    fn broad_file_grants_say_so() {
        let p = perms(&[("file", &["~/**", "%APPDATA%/**", "~/Documents/**", "~/a.txt"])]).unwrap();
        let want = [
            "file:~/** (can read every file there)",
            "file:%APPDATA%/** (can read every file there)",
            "file:~/Documents/**",
            "file:~/a.txt",
        ];
        assert_eq!(p.describe(), want.join(", "));
    }

    #[test]
    fn a_double_star_under_a_known_folder_or_the_users_folder_says_so() {
        let p =
            perms(&[("file", &["~/*/**", "C:/Users/**", "c:\\users\\*\\**", "C:/Users/*/notes.txt", "D:/data/**"])])
                .unwrap();
        let want = [
            "file:~/*/** (can read every file there)",
            "file:C:/Users/** (can read every file there)",
            "file:c:\\users\\*\\** (can read every file there)",
            "file:C:/Users/*/notes.txt",
            "file:D:/data/**",
        ];
        assert_eq!(p.describe(), want.join(", "));
    }

    #[test]
    fn appdata_and_dot_folder_patterns_say_so() {
        let every = [
            "~/AppData/**",
            "~/appdata/*/**",
            "~/AppData/Local/**",
            "~/AppData/Roaming/**",
            "~/AppData/LocalLow/**",
            "~/AppData/Local/Temp/**",
            "~/*.*/**",
            "~/.*/**",
            "~/.*/*/**",
        ];
        for pat in every {
            let p = perms(&[("file", &[pat])]).unwrap();
            assert_eq!(p.describe(), format!("file:{pat} (can read every file there)"), "{pat}");
        }
        let narrow =
            ["~/AppData/Roaming/Pat/**", "~/AppData/Local/Temp/rsb/**", "~/.config/**", "~/.*/config.toml", "~/*.txt"];
        for pat in narrow {
            let p = perms(&[("file", &[pat])]).unwrap();
            assert_eq!(p.describe(), format!("file:{pat}"), "{pat}");
        }
    }

    #[test]
    fn a_users_home_by_absolute_path_says_so_like_the_tilde_forms() {
        let e = env();
        let every = [
            "C:/Users/Pat/**",
            "c:\\users\\pat\\**",
            "C:/Users/Pat/*/**",
            "C:/Users/Pat/AppData/**",
            "C:/Users/Pat/AppData/Local/**",
            "C:/Users/Pat/AppData/Roaming/**",
            "C:/Users/Pat/AppData/Local/Temp/**",
            "C:/Users/*/AppData/**",
            "C:/Users/*/AppData/Local/**",
            "C:/Users/*/AppData/Roaming/**",
            "C:/Users/*/AppData/LocalLow/**",
            "C:/Users/*/.*/**",
            "C:/Users/P*/**",
        ];
        for pat in every {
            let p = perms(&[("file", &[pat])]).unwrap();
            let want = format!("file:{pat} (can read every file there)");
            assert_eq!(p.describe(), want, "{pat}");
            assert_eq!(p.describe_in(Some(&e)), want, "{pat} with the folders known");
        }
        let narrow = [
            "C:/Users/Pat/Documents/**",
            "C:/Users/Pat/AppData/Roaming/Pat/**",
            "C:/Users/Pat/AppData/Local/Temp/rsb/**",
            "C:/Users/*/Documents/**",
            "C:/Users/*/AppData/Roaming/Pat/**",
            "C:/Users/*/notes.txt",
            "C:/Users/Pat/*.txt",
        ];
        for pat in narrow {
            let p = perms(&[("file", &[pat])]).unwrap();
            assert_eq!(p.describe(), format!("file:{pat}"), "{pat}");
            assert_eq!(p.describe_in(Some(&e)), format!("file:{pat}"), "{pat} with the folders known");
        }
    }

    #[test]
    fn a_home_folder_outside_users_is_judged_through_the_known_folders() {
        let e = PathEnv {
            home: PathBuf::from("D:\\home\\Pat"),
            appdata: PathBuf::from("D:\\home\\Pat\\AppData\\Roaming"),
            local_appdata: PathBuf::from("D:\\home\\Pat\\AppData\\Local"),
            temp: PathBuf::from("D:\\home\\Pat\\AppData\\Local\\Temp"),
        };
        for pat in ["D:/home/Pat/**", "D:/home/Pat/*/**", "D:/home/Pat/AppData/**", "D:/home/Pat/.*/**"] {
            let p = perms(&[("file", &[pat])]).unwrap();
            assert_eq!(p.describe_in(Some(&e)), format!("file:{pat} (can read every file there)"), "{pat}");
            assert_eq!(p.describe(), format!("file:{pat}"), "without the folders there is nothing to compare: {pat}");
        }
        let p = perms(&[("file", &["D:/home/Pat/Documents/**", "D:/home/Other/**"])]).unwrap();
        assert_eq!(p.describe_in(Some(&e)), "file:D:/home/Pat/Documents/**, file:D:/home/Other/**");
    }

    #[test]
    fn drive_wide_patterns_are_refused() {
        for bad in ["C:/**", "C:/*/**", "C:/*/**/*", "C:/*", "C:/u*/x", "C:", "C:/"] {
            assert!(perms(&[("file", &[bad])]).is_err(), "{bad}");
        }
        assert!(perms(&[("file", &["C:/Users/*/notes.txt", "D:/data/**/*.csv"])]).is_ok());
    }

    #[test]
    fn the_widgets_own_folder_itself_is_allowed_as_a_working_folder() {
        let e = env();
        let wdir = PathBuf::from("C:\\Users\\Pat\\AppData\\Roaming\\rsb\\widgets\\my");
        let p = Permissions::default();
        assert!(p.check_file(&wdir, &wdir, &e).is_ok());
        assert!(p.check_file(&wdir.join("x.txt"), &wdir, &e).is_ok());
        assert!(p.check_file(wdir.parent().unwrap(), &wdir, &e).is_err());
        assert!(p.check_file(&wdir, Path::new(""), &e).is_err(), "no folder of its own: nothing is implicit");
    }

    #[test]
    fn wildcards_and_numeric_hosts_cannot_reach_what_they_do_not_name() {
        for bad in [
            "*.com",
            "*.uk",
            "*.co.uk",
            "*.com.au",
            "*.CO.NZ:8443",
            "*.1",
            "*.0.0.1",
            "*.127.0.0.1",
            "*.[::1]",
            "2130706433",
            "0x7f.1",
            "0x7f000001",
            "127.1",
            "1.2.3",
            "010.1.1.1",
            "256.1.1.1",
        ] {
            assert!(perms(&[("net", &[bad])]).is_err(), "{bad}");
        }
        for bad in ["https://2130706433/", "https://0x7f.1/", "https://127.1/"] {
            assert!(parse_url(bad).is_err(), "{bad}");
        }
        let p = perms(&[("net", &["192.168.1.10", "*.example.co.uk", "localhost:8080"])]).unwrap();
        assert!(p.check_net(&parse_url("https://192.168.1.10/").unwrap()).is_ok());
        assert!(p.check_net(&parse_url("https://a.example.co.uk/").unwrap()).is_ok());
        assert!(p.check_net(&parse_url("http://127.0.0.1/").unwrap()).is_err());
    }

    #[test]
    fn text_handed_to_the_shell_has_no_command_line_syntax() {
        assert!(open_url_text_ok("https://example.com/a?b=c&d=%22x%22").is_ok());
        for bad in [
            "https://e.com/\"%20--x",
            "https://e.com/a^b",
            "https://e.com/<a>",
            "https://e.com/a|b",
            "https://e.com/`x`",
            "https://e.com/'x'",
        ] {
            assert!(open_url_text_ok(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn net_rules() {
        let p = perms(&[("net", &["API.Example.com", "*.example.org", "localhost:8080", "host:8443", "[::1]:3000"])])
            .unwrap();
        let ok = |u: &str| p.check_net(&parse_url(u).unwrap()).is_ok();
        assert!(ok("https://api.example.com/v1?x=1"));
        assert!(ok("HTTPS://api.example.com./"));
        assert!(ok("https://a.b.example.org"));
        assert!(!ok("https://example.org"), "*.domain is subdomains only");
        assert!(!ok("https://api.example.com:444/"));
        assert!(ok("http://localhost:8080/x"));
        assert!(!ok("http://localhost:8081/x"));
        assert!(ok("https://host:8443"));
        assert!(ok("http://[::1]:3000/"));
        assert_eq!(p.check_net(&parse_url("https://evil.com:8443/").unwrap()), Err(Need::Net("evil.com:8443".into())));
        for bad in [
            "https://user@api.example.com/",
            "https://api.example.com\\@evil.com/",
            "https://api%2eexample.com/",
            "http://example.com/",
            "ftp://x/",
            "https://exa mple.com/",
            "https://b\u{fc}cher.de/",
            "https://:443/",
            "https://host:0/",
            "https://host:99999/",
            "https://[::1/",
        ] {
            assert!(parse_url(bad).is_err(), "{bad}");
        }
        for bad in ["localhost", "127.0.0.1", "*", "a*.b.com", "https://x.com", "x.com/path", "*.", ""] {
            assert!(perms(&[("net", &[bad])]).is_err(), "{bad}");
        }
        let u = parse_url("https://x.com?q").unwrap();
        assert_eq!((u.path_and_query.as_str(), u.port, u.explicit_port), ("/?q", 443, false));
    }

    #[test]
    fn file_rules() {
        let e = env();
        let wdir = PathBuf::from("C:\\Users\\Pat\\AppData\\Roaming\\rsb\\widgets\\my");
        let p = perms(&[("file", &["~/notes.txt", "%APPDATA%/app/*.json", "D:/data/**/*.csv"])]).unwrap();
        let ok = |s: &str| p.check_file(Path::new(s), &wdir, &e).is_ok();
        assert!(ok("C:\\Users\\Pat\\notes.txt"));
        assert!(ok("c:\\users\\pat\\NOTES.txt"));
        assert!(ok("C:\\Users\\Pat\\AppData\\Roaming\\app\\a.json"));
        assert!(!ok("C:\\Users\\Pat\\AppData\\Roaming\\app\\sub\\a.json"));
        assert!(ok("D:\\data\\x.csv") && ok("D:\\data\\a\\b\\x.csv"));
        assert!(ok("C:\\Users\\Pat\\AppData\\Roaming\\rsb\\widgets\\my\\icon.png"), "own folder");
        assert!(!ok("C:\\Users\\Pat\\AppData\\Roaming\\rsb\\widgets\\other.luau"));
        for bad in [
            "C:\\Users\\Pat\\notes.txt:secret",
            "\\\\server\\share\\notes.txt",
            "\\\\?\\C:\\Users\\Pat\\notes.txt",
            "C:\\Users\\Pat\\notes.txt.",
            "C:\\PROGRA~1\\x.txt",
            "C:\\Users\\Pat\\nul",
            "C:\\Users\\Pat\\..\\Pat\\notes.txt",
        ] {
            assert!(!ok(bad), "{bad}");
        }
        assert_eq!(
            p.check_file(Path::new("C:\\Users\\Pat\\secret.txt"), &wdir, &e),
            Err(Need::File("~/secret.txt".into()))
        );
        for bad in ["notes.txt", "C:/**", "\\\\srv\\x", "~/a:b", "~/../x", "%WINDIR%/x", "~/x?"] {
            assert!(perms(&[("file", &[bad])]).is_err(), "{bad}");
        }
    }

    #[test]
    fn control_read_and_describe() {
        let p = perms(&[("control", &["audio"]), ("read", &["window"])]).unwrap();
        assert!(p.check_control(ServiceId::Audio).is_ok());
        assert_eq!(p.check_control(ServiceId::Media), Err(Need::Control(ServiceId::Media)));
        assert!(p.check_read(ServiceId::Window).is_ok() && p.check_read(ServiceId::System).is_ok());
        assert_eq!(p.check_read(ServiceId::Media), Err(Need::Read(ServiceId::Media)));
        assert!(perms(&[("control", &["system"])]).is_err());
        assert!(perms(&[("read", &["audio"])]).is_err());
        assert!(perms(&[("netw", &["x.com"])]).unwrap_err().contains("unknown permission 'netw'"));
        // An unknown key is refused even when its list is empty.
        assert!(perms(&[("nett", &[])]).unwrap_err().contains("unknown permission 'nett'"));
        let mut u = Permissions::default();
        assert_eq!(u.describe(), "nothing");
        u.grant_user_defaults();
        assert_eq!(u.describe(), "read:window, read:media, read:network");
        let b = perms(&[("exec", &["cmd"]), ("net", &["a.com"])]).unwrap();
        assert_eq!(b.describe(), "exec:cmd (can run any program), net:a.com");
        assert_eq!(
            Need::Net("api.open-meteo.com".into()).message("my/weather"),
            "my/weather needs net:api.open-meteo.com; add it to permissions"
        );
        assert_eq!(Permissions::from_value(&Value::Nil), Ok(Permissions::default()));
        assert!(Permissions::from_value(&Value::Str("x".into())).is_err());
    }
}
