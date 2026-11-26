//! The presets that keep their look (fluent, astur-classic, glass) draw
//! exactly the pixels they drew when the goldens in `golden/presets` were
//! made, at three scales, dark and light, with sample values and a fixed
//! clock. A change to the engine that moves a pixel of them fails here.

use std::path::{Path, PathBuf};
use std::process::Command;

const EXE: &str = env!("CARGO_BIN_EXE_rsb");
const NOW: &str = "2026-11-25T11:10:00";

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden").join("presets")
}

/// The PNG files themselves: the same pixels through the same pinned
/// encoder give the same bytes.
fn bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn fluent_astur_classic_and_glass_draw_their_goldens() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("goldens-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("cfg")).unwrap();
    let mut checked = 0;
    let mut failed = Vec::new();
    for preset in ["fluent", "astur-classic", "glass"] {
        for scale in ["1", "1.25", "2"] {
            for theme in ["dark", "light"] {
                let name = format!("{preset}-{scale}-{theme}.png");
                let out = dir.join(&name);
                let status = Command::new(EXE)
                    .env("RSB_TEST_TAG", format!("goldens-{}", std::process::id()))
                    .arg("--config-dir")
                    .arg(dir.join("cfg"))
                    .arg("--log-dir")
                    .arg(dir.join("logs"))
                    .args(["render", "--preset", preset, "--scale", scale, "--theme", theme, "--width", "1920"])
                    .args(["--now", NOW, "--out"])
                    .arg(&out)
                    .output()
                    .expect("rsb starts");
                assert!(status.status.success(), "{name}: {}", String::from_utf8_lossy(&status.stderr));
                if bytes(&golden_dir().join(&name)) != bytes(&out) {
                    failed.push(format!("{name} differs from its golden (new render: {})", out.display()));
                }
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 18);
    assert!(failed.is_empty(), "{failed:#?}");
}
