//! What the settings tests share: a scratch folder with its own config and
//! log folders, the real binary run under its own pipe name, and a bar that
//! stops itself. Nothing here reads or writes the user's own folders.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

pub const EXE: &str = env!("CARGO_BIN_EXE_rsb");

pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    /// The one line of JSON the command printed.
    pub fn json(&self) -> Value {
        serde_json::from_str(self.stdout.trim()).unwrap_or_else(|e| panic!("not one line of JSON ({e}): {self:?}"))
    }
}

impl std::fmt::Debug for Out {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "exit {} stdout {:?} stderr {:?}", self.code, self.stdout, self.stderr)
    }
}

/// A folder of its own and the pipe name that goes with it.
pub struct Env {
    pub dir: PathBuf,
    pub tag: String,
}

impl Env {
    pub fn new(name: &str) -> Env {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the scratch folder can be made");
        Env { dir, tag: format!("{name}-{}", std::process::id()) }
    }

    pub fn cfg(&self) -> PathBuf {
        self.dir.join("cfg")
    }

    pub fn logs(&self) -> PathBuf {
        self.dir.join("logs")
    }

    /// The binary under this folder's pipe name, so it never reaches a real bar.
    pub fn command(&self) -> Command {
        let mut c = Command::new(EXE);
        c.env("RSB_TEST_TAG", &self.tag);
        c
    }

    fn finish(out: std::process::Output) -> Out {
        Out {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    /// `rsb <words>` with no folders.
    pub fn plain(&self, words: &[&str]) -> Out {
        Env::finish(self.command().args(words).output().expect("rsb runs"))
    }

    /// `rsb settings <words>` in this folder, logging to this folder's logs.
    pub fn settings(&self, words: &[&str]) -> Out {
        Env::finish(
            self.command()
                .arg("settings")
                .args(words)
                .arg("--config-dir")
                .arg(self.cfg())
                .arg("--log-dir")
                .arg(self.logs())
                .output()
                .expect("rsb runs"),
        )
    }

    /// `rsb inspect <words>`: the bar answers on this folder's pipe name.
    pub fn inspect(&self, words: &[&str]) -> Out {
        Env::finish(self.command().arg("inspect").args(words).output().expect("rsb runs"))
    }

    /// A bar that stops itself after `seconds`, offscreen, on the fakes.
    pub fn start_bar(&self, seconds: u32) -> Child {
        self.command()
            .arg("--config-dir")
            .arg(self.cfg())
            .arg("--log-dir")
            .arg(self.logs())
            .arg("--stats-out")
            .arg(self.dir.join("stats.jsonl"))
            .args(["--no-reserve", "--test-offscreen", "--no-hook", "--no-activate", "--fake-services"])
            .args(["--seconds", &seconds.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the bar starts")
    }

    /// Waits until the bar answers on its pipe.
    pub fn wait_for_bar(&self) {
        let until = Instant::now() + Duration::from_secs(15);
        loop {
            if self.plain(&["status", "--json"]).code == 0 {
                return;
            }
            assert!(Instant::now() < until, "the bar never answered");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn read(&self, file: &str) -> String {
        std::fs::read_to_string(self.cfg().join(file)).unwrap_or_else(|e| panic!("{file}: {e}"))
    }

    /// The version `get` reports for a file.
    pub fn hash_of(&self, file: &str) -> String {
        let got = self.settings(&["get"]).json();
        got["files"][file]["hash"].as_str().unwrap_or_else(|| panic!("no hash for {file}: {got}")).to_string()
    }
}

/// The lines of `before` that are comments, trimmed of their line ending.
pub fn comment_lines(text: &str, css: bool) -> Vec<String> {
    text.lines()
        .map(|l| l.trim_end_matches('\r').to_string())
        .filter(|l| {
            let t = l.trim_start();
            if css { t.starts_with("/*") || t.starts_with('*') || t.ends_with("*/") } else { t.starts_with('#') }
        })
        .collect()
}

/// The text of the lines a reply says it turned live, read from the file as it
/// was before that change.
pub fn turned_live(reply: &Value, before: &str) -> Vec<String> {
    let lines: Vec<&str> = before.lines().collect();
    reply["notices"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter(|n| n["code"] == "example_turned_live")
                .filter_map(|n| n["line"].as_u64())
                .filter_map(|n| lines.get(n as usize - 1))
                .map(|l| l.trim_end_matches('\r').to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The lines of the log that are settings changes and refusals.
pub fn config_log(env: &Env) -> (Vec<String>, Vec<String>) {
    let text = std::fs::read_to_string(env.logs().join("rsb.log")).unwrap_or_default();
    let mut changes = Vec::new();
    let mut refusals = Vec::new();
    for line in text.lines() {
        let Some((_, rest)) = line.split_once(" INFO config: ") else { continue };
        if rest.contains(": refused ") {
            refusals.push(rest.to_string());
        } else {
            changes.push(rest.to_string());
        }
    }
    (changes, refusals)
}
