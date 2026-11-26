//! The Start menu is read in a short-lived child process: `rsb --apps-reader <icon side>`.
//!
//! Listing the shell's AppsFolder and asking it for icons loads some fifty shell libraries and
//! grows the heap by tens of megabytes, and Windows keeps them loaded for as long as the process
//! lives. Done in a child, all of it goes when the child exits and the bar stays small.
//!
//! The child reads the icon keys it may skip from stdin (one per line, until stdin closes) and
//! writes frames to stdout:
//!
//! - `L`: the list: a count, then per app a length-prefixed id and name (UTF-8, `u16` lengths);
//! - `I`: one icon: the id, width, height, then `width * height * 4` bytes of RGBA;
//! - `E`: the end.
//!
//! Every number is little-endian. The reader side checks every length before it allocates:
//! the child is this program, but what comes through the pipe is still not trusted.

use std::collections::HashSet;
use std::io::{self, BufRead, IsTerminal, Read, Write};

use rsb_core::pixels::RgbaImage;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};

pub use super::cache::icon_key;
use super::shell::{self, AppEntry};
use super::{MAX_ID_CHARS, launchable_id};

/// The first argument of the child.
pub const FLAG: &str = "--apps-reader";

/// Most apps one list may hold (the shell side cuts at the same number).
const MAX_LIST: usize = 4096;
/// Longest id or name on the wire, in bytes (UTF-8 is at most four bytes a char).
const MAX_FIELD: usize = MAX_ID_CHARS * 4;
/// Largest icon side accepted.
const MAX_SIDE: u32 = 256;
/// Most icon keys the child reads from stdin.
const MAX_SKIP: usize = 8192;

/// One thing the child said.
#[derive(Debug, PartialEq)]
pub enum Frame {
    List(Vec<AppEntry>),
    Icon { id: String, image: RgbaImage },
    End,
}

fn put_str(w: &mut impl Write, s: &str) -> io::Result<()> {
    let len = u16::try_from(s.len()).map_err(|_| io::Error::other("a field is too long"))?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(s.as_bytes())
}

pub(crate) fn write_list(w: &mut impl Write, apps: &[AppEntry]) -> io::Result<()> {
    w.write_all(b"L")?;
    w.write_all(&(apps.len() as u32).to_le_bytes())?;
    for a in apps {
        put_str(w, &a.id)?;
        put_str(w, &a.name)?;
    }
    w.flush()
}

pub(crate) fn write_icon(w: &mut impl Write, id: &str, image: &RgbaImage) -> io::Result<()> {
    w.write_all(b"I")?;
    put_str(w, id)?;
    w.write_all(&image.width.to_le_bytes())?;
    w.write_all(&image.height.to_le_bytes())?;
    w.write_all(&image.data)?;
    w.flush()
}

pub(crate) fn write_end(w: &mut impl Write) -> io::Result<()> {
    w.write_all(b"E")?;
    w.flush()
}

fn bad(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.to_string())
}

fn get_u16(r: &mut impl Read) -> io::Result<u16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}

fn get_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn get_str(r: &mut impl Read) -> io::Result<String> {
    let len = usize::from(get_u16(r)?);
    if len > MAX_FIELD {
        return Err(bad("a field is too long"));
    }
    let mut b = vec![0u8; len];
    r.read_exact(&mut b)?;
    String::from_utf8(b).map_err(|_| bad("a field is not text"))
}

/// The next frame. An error ends the read: the stream is cut, or what came through is not ours.
pub fn read_frame(r: &mut impl Read) -> io::Result<Frame> {
    let mut tag = [0u8; 1];
    r.read_exact(&mut tag)?;
    match tag[0] {
        b'L' => {
            let count = get_u32(r)? as usize;
            if count > MAX_LIST {
                return Err(bad("too many apps"));
            }
            let mut apps = Vec::with_capacity(count);
            for _ in 0..count {
                let (id, name) = (get_str(r)?, get_str(r)?);
                if !launchable_id(&id, MAX_ID_CHARS) {
                    return Err(bad("an id that cannot be launched"));
                }
                apps.push(AppEntry { id, name });
            }
            Ok(Frame::List(apps))
        }
        b'I' => {
            let id = get_str(r)?;
            let (w, h) = (get_u32(r)?, get_u32(r)?);
            if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
                return Err(bad("an icon of a size that is not allowed"));
            }
            let mut data = vec![0u8; w as usize * h as usize * 4];
            r.read_exact(&mut data)?;
            let image = RgbaImage::new(w, h, data).ok_or_else(|| bad("an icon with the wrong byte count"))?;
            Ok(Frame::Icon { id, image })
        }
        b'E' => Ok(Frame::End),
        _ => Err(bad("an unknown frame")),
    }
}

/// The keys on stdin, at most `MAX_SKIP`.
fn skip_keys(input: impl BufRead) -> HashSet<String> {
    input.lines().map_while(Result::ok).take(MAX_SKIP).map(|l| l.trim().to_string()).collect()
}

/// What the child does: list, then every icon not in `skip`, each as soon as it is ready.
fn read_start_menu(out: &mut impl Write, side: i32, skip: &HashSet<String>) -> Result<(), String> {
    let listed = shell::list_apps(MAX_ID_CHARS)?;
    let entries: Vec<AppEntry> = listed.iter().map(|l| l.entry.clone()).collect();
    write_list(out, &entries).map_err(|e| e.to_string())?;
    for l in &listed {
        if skip.contains(&icon_key(&l.entry.id)) {
            continue;
        }
        // An icon the shell already has first; only an item with none cached gets the full call.
        if let Ok(image) = shell::icon_of(&l.item, side, true).or_else(|_| shell::icon_of(&l.item, side, false)) {
            write_icon(out, &l.entry.id, &image).map_err(|e| e.to_string())?;
        }
    }
    // The shell items are released while the apartment still exists.
    drop(listed);
    write_end(out).map_err(|e| e.to_string())
}

/// The child's whole life. `args` are what follows `--apps-reader`: the icon side in px.
/// The exit code is 0 when it said everything it had to.
pub fn main(args: &[String]) -> i32 {
    if io::stdout().is_terminal() {
        eprintln!("rsb {FLAG} is for rsb's own use: it writes binary frames to a pipe");
        return 2;
    }
    let side = args.first().and_then(|s| s.parse::<i32>().ok()).filter(|s| (16..=MAX_SIDE as i32).contains(s));
    let Some(side) = side else {
        eprintln!("rsb {FLAG}: the icon side is a number from 16 to {MAX_SIDE}");
        return 2;
    };
    let skip = skip_keys(io::stdin().lock());
    // SAFETY: initialises COM for this thread as an STA (the shell's item objects need one); balanced below.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let result = read_start_menu(&mut io::stdout().lock(), side, &skip);
    if com.is_ok() {
        // SAFETY: balances the successful CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
    match result {
        Ok(()) => 0,
        Err(e) => {
            // The parent logs this; stdout is a pipe and the list may be half written.
            eprintln!("rsb {FLAG}: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, name: &str) -> AppEntry {
        AppEntry { id: id.into(), name: name.into() }
    }

    fn image(side: u32, fill: u8) -> RgbaImage {
        RgbaImage::new(side, side, vec![fill; (side * side * 4) as usize]).unwrap()
    }

    #[test]
    fn frames_survive_the_pipe_in_order() {
        let mut wire = Vec::new();
        let apps = vec![entry("Sample.App_1!App", "Sample"), entry("{GUID}\\a.exe", "\u{e9}t\u{e9} \u{3042}")];
        write_list(&mut wire, &apps).unwrap();
        write_icon(&mut wire, "Sample.App_1!App", &image(4, 7)).unwrap();
        write_end(&mut wire).unwrap();
        let mut r = wire.as_slice();
        assert_eq!(read_frame(&mut r).unwrap(), Frame::List(apps));
        assert_eq!(read_frame(&mut r).unwrap(), Frame::Icon { id: "Sample.App_1!App".into(), image: image(4, 7) });
        assert_eq!(read_frame(&mut r).unwrap(), Frame::End);
        assert!(read_frame(&mut r).is_err(), "nothing after the end");
    }

    #[test]
    fn a_cut_or_forged_stream_is_refused_before_anything_large_is_allocated() {
        // Unknown tag.
        assert!(read_frame(&mut &b"X"[..]).is_err());
        // A list that claims more apps than the limit.
        let mut wire = vec![b'L'];
        wire.extend((MAX_LIST as u32 + 1).to_le_bytes());
        assert!(read_frame(&mut wire.as_slice()).is_err());
        // An icon of 60000 x 60000 px: refused on the size, never read.
        let mut wire = vec![b'I', 1, 0, b'a'];
        wire.extend(60000u32.to_le_bytes());
        wire.extend(60000u32.to_le_bytes());
        assert!(read_frame(&mut wire.as_slice()).is_err());
        // An icon cut short.
        let mut wire = Vec::new();
        write_icon(&mut wire, "a", &image(8, 1)).unwrap();
        wire.truncate(wire.len() - 5);
        assert!(read_frame(&mut wire.as_slice()).is_err());
        // An id that could never be launched (a control character) is not taken into the list.
        let mut wire = Vec::new();
        write_list(&mut wire, &[entry("bad\nid", "x")]).unwrap();
        assert!(read_frame(&mut wire.as_slice()).is_err());
        // A field longer than the wire allows cannot be written either.
        assert!(write_list(&mut Vec::new(), &[entry(&"a".repeat(70_000), "x")]).is_err());
    }

    #[test]
    fn the_keys_to_skip_are_read_line_by_line_up_to_a_limit() {
        let text = "rsb-image:apps/0001\n  rsb-image:apps/0002  \n\n";
        let keys = skip_keys(text.as_bytes());
        assert!(keys.contains("rsb-image:apps/0001") && keys.contains("rsb-image:apps/0002"));
        let many: String = (0..MAX_SKIP + 50).map(|i| format!("k{i}\n")).collect();
        assert_eq!(skip_keys(many.as_bytes()).len(), MAX_SKIP);
    }
}
