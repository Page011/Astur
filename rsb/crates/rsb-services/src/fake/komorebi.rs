//! Plays komorebi's side of `subscribe-pipe`: connects to the named pipe the
//! workspaces service created and writes one JSON notification per line,
//! `{"event":{...},"state":{...}}`, with the state fields the service reads.
//! `FakeKomorebiControl` stands in for `komorebic` and the process list.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::workspaces::KomorebiControl;

pub struct FakeKomorebi {
    file: Option<File>,
}

impl FakeKomorebi {
    /// Connects to `\\.\pipe\<pipe_name>` (retrying for up to 2 s).
    pub fn connect(pipe_name: &str) -> std::io::Result<FakeKomorebi> {
        let path = format!("\\\\.\\pipe\\{pipe_name}");
        let end = Instant::now() + Duration::from_secs(2);
        loop {
            match OpenOptions::new().write(true).open(&path) {
                Ok(file) => return Ok(FakeKomorebi { file: Some(file) }),
                Err(e) if Instant::now() >= end => return Err(e),
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    /// Writes a notification whose state has these monitors (workspace names
    /// and window counts) and focus.
    pub fn send_state(&mut self, monitors: &[Vec<(&str, u32)>], focused: (usize, usize)) -> std::io::Result<()> {
        let elements: Vec<Value> = monitors
            .iter()
            .enumerate()
            .map(|(mi, wss)| {
                let workspaces: Vec<Value> = wss
                    .iter()
                    .map(|(name, windows)| {
                        let containers: Vec<Value> =
                            (0..*windows).map(|_| json!({"windows":{"elements":[{}],"focused":0}})).collect();
                        json!({
                            "name": if name.is_empty() { Value::Null } else { json!(name) },
                            "containers": {"elements": containers, "focused": 0},
                            "monocle_container": null, "maximized_window": null,
                            "floating_windows": {"elements": []}
                        })
                    })
                    .collect();
                json!({
                    "name": format!("DISPLAY{}", mi + 1), "device": "FAKE", "device_id": format!("FAKE-{mi}"),
                    "workspaces": {"elements": workspaces, "focused": if mi == focused.0 { focused.1 } else { 0 }}
                })
            })
            .collect();
        let state = json!({"monitors": {"elements": elements, "focused": focused.0}});
        let line = json!({"event": {"type": "FocusWorkspaceNumber", "content": focused.1}, "state": state});
        self.send_raw(format!("{line}\n").as_bytes())
    }

    /// Writes these bytes as they are (oversize and malformed-input tests).
    pub fn send_raw(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        let f = self.file.as_mut().ok_or_else(|| std::io::Error::other("closed"))?;
        f.write_all(bytes)?;
        f.flush()
    }

    /// Closes the pipe (komorebi exited).
    pub fn close(self) {}
}

/// Stands in for `komorebic` and the process list: records what the
/// service asks, never starts a program.
#[derive(Default)]
pub struct FakeKomorebiControl {
    running: AtomicBool,
    /// Pipe names the service asked komorebi to subscribe to.
    pub subscribed: Mutex<Vec<String>>,
    pub unsubscribed: Mutex<Vec<String>>,
    pub focused: Mutex<Vec<String>>,
}

impl FakeKomorebiControl {
    pub fn new(running: bool) -> FakeKomorebiControl {
        let c = FakeKomorebiControl::default();
        c.running.store(running, Ordering::Release);
        c
    }

    pub fn set_running(&self, running: bool) {
        self.running.store(running, Ordering::Release);
    }

    pub fn subscribed(&self) -> Vec<String> {
        self.subscribed.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn focus_commands(&self) -> Vec<String> {
        self.focused.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl KomorebiControl for FakeKomorebiControl {
    fn running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    fn subscribe(&self, pipe: &str) -> Result<(), String> {
        self.subscribed.lock().unwrap_or_else(|e| e.into_inner()).push(pipe.to_string());
        Ok(())
    }

    fn unsubscribe(&self, pipe: &str) {
        self.unsubscribed.lock().unwrap_or_else(|e| e.into_inner()).push(pipe.to_string());
    }

    fn focus_named(&self, name: &str) -> Result<(), String> {
        self.focused.lock().unwrap_or_else(|e| e.into_inner()).push(name.to_string());
        Ok(())
    }
}
