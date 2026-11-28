//! Writes presented frames to a folder: one PNG per present of every surface
//! and an `index.jsonl` that says when, which surface and what changed. Used
//! by `--capture` on an offscreen run and by `rsb render --timeline`.
//!
//! A capture is only ever written under a scratch or build-output folder, so a
//! picture of the user's screen (or of a real launcher, device list or track
//! title) can never land in a place anyone keeps files.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use rsb_core::geom::RectPx;
use rsb_render::Canvas;
use serde_json::json;

/// Most frames one capture writes.
pub const MAX_FRAMES: u32 = 2000;
/// Longest a capture runs, milliseconds.
pub const MAX_MS: u64 = 120_000;

/// True for a folder with a `target`, `scratchpad` or `scratch` component. A
/// relative folder is judged by where it is (the current folder is joined
/// first), and one with a `..` in it is refused: it could leave the folder
/// whose name made it acceptable.
pub fn dir_allowed(dir: &Path) -> bool {
    if dir.as_os_str().is_empty() || dir.components().any(|c| matches!(c, Component::ParentDir)) {
        return false;
    }
    let full = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    full.components().any(|c| {
        let name = c.as_os_str().to_string_lossy().to_ascii_lowercase();
        name == "target" || name == "scratchpad" || name == "scratch"
    })
}

/// Why a capture stopped taking frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    Frames,
    Time,
}

impl Limit {
    pub fn name(self) -> &'static str {
        match self {
            Limit::Frames => "frames",
            Limit::Time => "time",
        }
    }
}

pub struct Capture {
    dir: PathBuf,
    index: BufWriter<File>,
    frames: u32,
    started: Instant,
    limit: Option<Limit>,
    last_ms: u64,
    /// Frames of the same surface in the same millisecond share a number.
    repeat: u32,
    last_key: (String, u64),
}

/// `bar:main` as a file name part: `bar-main`.
fn file_part(surface: &str) -> String {
    surface.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect()
}

impl Capture {
    /// Makes the folder (and the index file). The caller has checked `dir_allowed`.
    pub fn create(dir: &Path) -> Result<Capture, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create the capture folder: {e}"))?;
        let index = File::create(dir.join("index.jsonl")).map_err(|e| format!("cannot create index.jsonl: {e}"))?;
        Ok(Capture {
            dir: dir.to_path_buf(),
            index: BufWriter::new(index),
            frames: 0,
            started: Instant::now(),
            limit: None,
            last_ms: 0,
            repeat: 0,
            last_key: (String::new(), 0),
        })
    }

    /// Milliseconds since the capture began, on the system clock.
    pub fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    pub fn frames(&self) -> u32 {
        self.frames
    }

    pub fn limit(&self) -> Option<Limit> {
        self.limit
    }

    /// Writes one frame of `surface`: the whole surface as a PNG and a line in
    /// the index naming the part that changed. Returns the limit once one is
    /// reached (nothing is written then, and every later call says so too).
    pub fn record(
        &mut self,
        t_ms: u64,
        surface: &str,
        dirty: RectPx,
        canvas: &Canvas,
    ) -> Result<Option<Limit>, String> {
        if let Some(l) = self.limit {
            return Ok(Some(l));
        }
        if self.frames >= MAX_FRAMES {
            self.limit = Some(Limit::Frames);
            return Ok(self.limit);
        }
        if t_ms > MAX_MS {
            self.limit = Some(Limit::Time);
            return Ok(self.limit);
        }
        let key = (surface.to_string(), t_ms);
        self.repeat = if key == self.last_key { self.repeat + 1 } else { 0 };
        self.last_key = key;
        let name = if self.repeat == 0 {
            format!("{}-{t_ms}.png", file_part(surface))
        } else {
            format!("{}-{t_ms}_{}.png", file_part(surface), self.repeat)
        };
        let png = canvas.to_png().map_err(|e| format!("cannot encode a frame: {e}"))?;
        std::fs::write(self.dir.join(&name), png).map_err(|e| format!("cannot write a frame: {e}"))?;
        let line = json!({"t_ms": t_ms, "surface": surface, "rect": [dirty.x0, dirty.y0, dirty.x1, dirty.y1],
                          "size": [canvas.width, canvas.height], "file": name});
        writeln!(self.index, "{line}").map_err(|e| format!("cannot write index.jsonl: {e}"))?;
        self.frames += 1;
        self.last_ms = t_ms;
        Ok(None)
    }

    pub fn finish(&mut self) {
        let _ = self.index.flush();
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn canvas() -> Canvas {
        Canvas::new(4, 2)
    }

    #[test]
    fn only_scratch_and_target_folders_are_allowed() {
        assert!(dir_allowed(Path::new("C:\\work\\target\\frames")));
        assert!(dir_allowed(Path::new("C:\\Temp\\scratchpad\\x")));
        assert!(dir_allowed(Path::new("D:\\SCRATCH\\x")));
        assert!(!dir_allowed(Path::new("D:\\photos")));
        assert!(!dir_allowed(Path::new("C:\\targets\\x")));
        assert!(!dir_allowed(Path::new("")));
        assert!(!dir_allowed(Path::new(r"C:\work\target\..\..\photos")), "a .. can leave the folder");
        assert!(!dir_allowed(Path::new(r"..\target\x")));
    }

    #[test]
    fn a_frame_is_a_png_and_an_index_line() {
        let dir = TempDir::new("capture");
        let out = dir.path().join("target").join("c");
        let mut cap = Capture::create(&out).unwrap();
        let r = RectPx::new(0, 0, 4, 2);
        assert_eq!(cap.record(120, "bar:main", r, &canvas()).unwrap(), None);
        assert_eq!(cap.record(120, "bar:main", r, &canvas()).unwrap(), None, "same ms, same surface");
        assert_eq!(cap.record(136, "popup:dash", r, &canvas()).unwrap(), None);
        cap.finish();
        assert_eq!(cap.frames(), 3);
        for name in ["bar-main-120.png", "bar-main-120_1.png", "popup-dash-136.png"] {
            let bytes = std::fs::read(out.join(name)).unwrap();
            assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "{name}");
        }
        let text = std::fs::read_to_string(out.join("index.jsonl")).unwrap();
        let lines: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["t_ms"], 120);
        assert_eq!(lines[0]["surface"], "bar:main");
        assert_eq!(lines[0]["rect"], json!([0, 0, 4, 2]));
        assert_eq!(lines[2]["surface"], "popup:dash");
    }

    #[test]
    fn the_frame_and_time_limits_stop_the_writing() {
        let dir = TempDir::new("capture-limit");
        let mut cap = Capture::create(&dir.path().join("target")).unwrap();
        let r = RectPx::new(0, 0, 4, 2);
        assert_eq!(cap.record(MAX_MS + 1, "bar:main", r, &canvas()).unwrap(), Some(Limit::Time));
        assert_eq!(cap.frames(), 0);
        assert_eq!(cap.record(1, "bar:main", r, &canvas()).unwrap(), Some(Limit::Time), "it stays stopped");
        let mut cap = Capture::create(&dir.path().join("target2")).unwrap();
        for t in 0..u64::from(MAX_FRAMES) {
            assert_eq!(cap.record(t, "bar:main", r, &canvas()).unwrap(), None);
        }
        assert_eq!(cap.frames(), MAX_FRAMES);
        assert_eq!(cap.record(2001, "bar:main", r, &canvas()).unwrap(), Some(Limit::Frames));
        assert_eq!(cap.frames(), MAX_FRAMES, "the 2001st frame is not written");
    }
}
