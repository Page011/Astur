//! Keeping test runs off the real PC. A run driven by a test flag, a run
//! under `RSB_TEST_TAG`, and every `rsb render` pass each service command
//! through a recorder on the hub thread, and replace the session-ending power
//! calls with one that only reports. The recorder lets commands through only
//! when the services are the fakes (their values then change as a real PC's
//! would); with the real services a simulated click on the microphone, the
//! output, Bluetooth, the power mode or a launcher row reaches no real
//! service. A plain run sets neither, so its commands go where they say.

use std::sync::Arc;

use rsb_core::services::ServiceCommand;
use rsb_services::{CommandGate, CommandRecorder, PowerActions};
use serde_json::json;

use crate::args::RunOpts;
use crate::stats::Stats;

/// What a run does with commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    /// Commands reach the services (true only for the fakes).
    pub forward: bool,
}

/// The environment variable that gives a test run its own pipe and instance
/// lock; setting it also selects the recorder.
pub const TEST_TAG_VAR: &str = "RSB_TEST_TAG";

/// The gate for a run of the bar; None = a plain run.
pub fn select_run(opts: &RunOpts, tag_set: bool) -> Option<Selection> {
    (opts.gated || tag_set).then_some(Selection { forward: opts.fake_services })
}

/// `rsb render` always records. Its services are the fakes unless `--live`.
pub fn select_render(live: bool) -> Selection {
    Selection { forward: !live }
}

/// True when `RSB_TEST_TAG` is set in this process.
pub fn tag_set() -> bool {
    std::env::var_os(TEST_TAG_VAR).is_some()
}

/// Records each command as a `command` stats line: the service and the
/// method name only (never an id, which can be a device or an app).
struct StatsGate {
    recorder: CommandRecorder,
    stats: Arc<Stats>,
}

impl std::fmt::Debug for StatsGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatsGate").field("recorder", &self.recorder).finish()
    }
}

impl CommandGate for StatsGate {
    fn pass(&self, command: &ServiceCommand) -> bool {
        let forwarded = self.recorder.pass(command);
        self.stats.emit(
            "command",
            json!({"service": command.service().name(), "kind": command.kind(), "forwarded": forwarded}),
        );
        forwarded
    }
}

/// Stands in for Windows' lock, sleep, restart, shut down and sign out: it
/// only says which one was asked for.
struct PowerRecorder {
    stats: Arc<Stats>,
}

impl std::fmt::Debug for PowerRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PowerRecorder")
    }
}

impl PowerRecorder {
    fn record(&self, action: &str) -> Result<(), String> {
        self.stats.emit("power_action", json!({"action": action}));
        Ok(())
    }
}

impl PowerActions for PowerRecorder {
    fn lock(&self) -> Result<(), String> {
        self.record("lock")
    }
    fn sleep(&self) -> Result<(), String> {
        self.record("sleep")
    }
    fn restart(&self) -> Result<(), String> {
        self.record("restart")
    }
    fn shut_down(&self) -> Result<(), String> {
        self.record("shut_down")
    }
    fn sign_out(&self) -> Result<(), String> {
        self.record("sign_out")
    }
}

/// The two seams the hub gets for a recorded run.
#[derive(Clone, Debug)]
pub struct Gate {
    pub commands: Arc<dyn CommandGate>,
    pub power: Arc<dyn PowerActions>,
}

impl Gate {
    pub fn new(selection: Selection, stats: Arc<Stats>) -> Gate {
        Gate {
            commands: Arc::new(StatsGate {
                recorder: CommandRecorder::new(selection.forward),
                stats: Arc::clone(&stats),
            }),
            power: Arc::new(PowerRecorder { stats }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsb_core::services::{AudioCommand, PowerCommand};
    use std::time::Instant;

    fn stats_file(tag: &str) -> (crate::testutil::TempDir, Arc<Stats>) {
        let dir = crate::testutil::TempDir::new(tag);
        let stats = Arc::new(Stats::open(Some(&dir.path().join("s.jsonl")), false, Instant::now()).unwrap());
        (dir, stats)
    }

    fn lines(dir: &crate::testutil::TempDir) -> Vec<serde_json::Value> {
        std::fs::read_to_string(dir.path().join("s.jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn a_plain_run_has_no_gate_and_a_tag_or_any_test_flag_has_one() {
        assert_eq!(select_run(&RunOpts::default(), false), None);
        assert_eq!(select_run(&RunOpts::default(), true), Some(Selection { forward: false }));
        let fake = RunOpts { fake_services: true, gated: true, ..RunOpts::default() };
        assert_eq!(select_run(&fake, false), Some(Selection { forward: true }));
        let offscreen = RunOpts { test_offscreen: true, gated: true, ..RunOpts::default() };
        assert_eq!(select_run(&offscreen, false), Some(Selection { forward: false }));
        assert_eq!(select_render(false), Selection { forward: true });
        assert_eq!(select_render(true), Selection { forward: false });
    }

    #[test]
    fn a_recorded_command_is_a_stats_line_with_names_only_and_stops_unless_forwarded() {
        for forward in [false, true] {
            let (dir, stats) = stats_file("gate");
            let gate = Gate::new(Selection { forward }, stats);
            let first = ServiceCommand::Audio(AudioCommand::SetVolume(33.0));
            assert_eq!(gate.commands.pass(&first), forward);
            assert_eq!(gate.commands.pass(&ServiceCommand::Power(PowerCommand::Lock)), forward);
            let seen = lines(&dir);
            assert_eq!(seen.len(), 2);
            assert_eq!(seen[0]["event"], "command");
            assert_eq!((seen[0]["service"].as_str(), seen[0]["kind"].as_str()), (Some("audio"), Some("set_volume")));
            assert_eq!(seen[0]["forwarded"], forward);
            let mut keys: Vec<&str> = seen[0].as_object().unwrap().keys().map(String::as_str).collect();
            keys.sort_unstable();
            assert_eq!(keys, ["event", "forwarded", "kind", "service", "session", "t_ms", "wall_ms"], "no argument");
            assert_eq!(seen[1]["service"], "power");
        }
    }

    #[test]
    fn the_power_recorder_reports_each_action_and_calls_nothing() {
        let (dir, stats) = stats_file("power");
        let power = Gate::new(Selection { forward: false }, stats).power;
        for (call, name) in [
            (power.lock(), "lock"),
            (power.sleep(), "sleep"),
            (power.restart(), "restart"),
            (power.shut_down(), "shut_down"),
            (power.sign_out(), "sign_out"),
        ] {
            assert_eq!(call, Ok(()), "{name}");
        }
        let actions: Vec<String> =
            lines(&dir).iter().filter(|l| l["event"] == "power_action").map(|l| l["action"].to_string()).collect();
        assert_eq!(actions, ["\"lock\"", "\"sleep\"", "\"restart\"", "\"shut_down\"", "\"sign_out\""]);
    }
}
