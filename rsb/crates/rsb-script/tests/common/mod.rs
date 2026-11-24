//! Test harness: a `Runtime` with a manual clock, in-memory widget sources, and
//! a hub and sink that record everything.
#![allow(dead_code)]

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use rsb_core::config::{Spanned, WidgetOptions, WidgetRef};
use rsb_core::diag::{Diagnostic, Plate, SourcePos};
use rsb_core::element::{Element, InstanceId, WidgetTree};
use rsb_core::hit::{Button, HandlerCall, InputKind};
use rsb_core::io::{IoEvent, IoEventKind, IoId};
use rsb_core::msg::{
    Heartbeat, HubEvent, HubPort, HubRequest, InstanceSpec, MonitorRef, ScriptIn, ScriptOut, ScriptSink, WidgetState,
    WidgetStatus,
};
use rsb_core::permissions::PathEnv;
use rsb_core::time::{LocalTime, ManualClock, ThreadTime};
use rsb_core::value::Value;
use rsb_script::{Limits, Runtime, ScriptOptions, SourceStore};

pub struct MemStore {
    pub user: Mutex<HashMap<String, String>>,
    pub builtin: Mutex<HashMap<String, String>>,
}

impl SourceStore for MemStore {
    fn builtin(&self, rel_path: &str) -> Option<Cow<'static, str>> {
        self.builtin.lock().unwrap().get(rel_path).map(|s| Cow::Owned(s.clone()))
    }
    fn builtin_widgets(&self) -> Vec<String> {
        self.builtin
            .lock()
            .unwrap()
            .keys()
            .filter_map(|k| k.strip_prefix("widgets/").and_then(|r| r.strip_suffix(".luau")))
            .filter(|r| !r.contains('/'))
            .map(str::to_string)
            .collect()
    }
    fn user(&self, rel_path: &str) -> std::io::Result<Option<String>> {
        Ok(self.user.lock().unwrap().get(rel_path).cloned())
    }
}

pub struct FakeHub(pub Arc<Mutex<Vec<HubRequest>>>, pub Arc<Heartbeat>, pub Arc<Mutex<Vec<bool>>>);

impl HubPort for FakeHub {
    fn send(&self, request: HubRequest) {
        // Every request arrives from inside a call into a VM, except teardown ones.
        self.2.lock().unwrap().push(self.1.busy(std::time::Instant::now()).is_some());
        self.0.lock().unwrap().push(request);
    }
}

pub struct FakeSink(pub Arc<Mutex<Vec<ScriptOut>>>);

impl ScriptSink for FakeSink {
    fn send(&self, out: ScriptOut) {
        self.0.lock().unwrap().push(out);
    }
}

/// A thread clock tests move by hand; `step` is added on every read.
pub struct StepTime {
    pub now: Mutex<Duration>,
    pub step: Duration,
}

impl ThreadTime for StepTime {
    fn now(&self) -> Duration {
        let mut n = self.now.lock().unwrap();
        *n += self.step;
        *n
    }
}

static LOGGER: Once = Once::new();
static LOG_LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Capture;
impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        LOG_LINES.lock().unwrap().push(format!("{} {}", r.level(), r.args()));
    }
    fn flush(&self) {}
}

pub fn init_log() {
    LOGGER.call_once(|| {
        let _ = log::set_logger(&Capture);
        log::set_max_level(log::LevelFilter::Info);
    });
}

/// Log lines (all tests together) containing `needle`.
pub fn log_lines(needle: &str) -> Vec<String> {
    LOG_LINES.lock().unwrap().iter().filter(|l| l.contains(needle)).cloned().collect()
}

pub struct Harness {
    pub rt: Runtime,
    pub store: Arc<MemStore>,
    pub clock: Arc<ManualClock>,
    pub hub: Arc<Mutex<Vec<HubRequest>>>,
    pub hub_busy: Arc<Mutex<Vec<bool>>>,
    pub out: Arc<Mutex<Vec<ScriptOut>>>,
    pub heartbeat: Arc<Heartbeat>,
    pub latest: HashMap<InstanceId, Arc<WidgetTree>>,
    pub plates: HashMap<InstanceId, Option<Plate>>,
    pub diags: Vec<Diagnostic>,
    pub tree_batches: usize,
    pub config_dir: PathBuf,
}

pub fn start_time() -> LocalTime {
    LocalTime {
        year: 2026,
        month: 11,
        day: 24,
        weekday: 2,
        hour: 9,
        minute: 5,
        second: 7,
        millis: 250,
        utc_offset_min: 600,
    }
}

pub fn path_env() -> PathEnv {
    PathEnv {
        home: PathBuf::from("C:\\Users\\tester"),
        appdata: PathBuf::from("C:\\Users\\tester\\AppData\\Roaming"),
        local_appdata: PathBuf::from("C:\\Users\\tester\\AppData\\Local"),
        temp: PathBuf::from("C:\\Users\\tester\\AppData\\Local\\Temp"),
    }
}

impl Harness {
    pub fn new() -> Harness {
        Harness::with(Limits::default(), None)
    }

    pub fn with(limits: Limits, thread_time: Option<Arc<dyn ThreadTime>>) -> Harness {
        init_log();
        let store = Arc::new(MemStore { user: Default::default(), builtin: Default::default() });
        let clock = Arc::new(ManualClock::new(start_time()));
        let hub = Arc::new(Mutex::new(Vec::new()));
        let hub_busy = Arc::new(Mutex::new(Vec::new()));
        let out = Arc::new(Mutex::new(Vec::new()));
        let heartbeat = Arc::new(Heartbeat::new());
        let config_dir = PathBuf::from("C:\\Users\\tester\\AppData\\Roaming\\rsb");
        let rt = Runtime::new(
            ScriptOptions {
                sources: store.clone(),
                clock: clock.clone(),
                config_dir: config_dir.clone(),
                path_env: path_env(),
                limits,
                dark: true,
                thread_time,
                heartbeat: heartbeat.clone(),
            },
            Box::new(FakeHub(hub.clone(), heartbeat.clone(), hub_busy.clone())),
            Box::new(FakeSink(out.clone())),
        );
        Harness {
            rt,
            store,
            clock,
            hub,
            hub_busy,
            out,
            heartbeat,
            latest: HashMap::new(),
            plates: HashMap::new(),
            diags: Vec::new(),
            tree_batches: 0,
            config_dir,
        }
    }

    pub fn user(&self, rel: &str, src: &str) {
        self.store.user.lock().unwrap().insert(rel.to_string(), src.to_string());
    }

    pub fn builtin(&self, rel: &str, src: &str) {
        self.store.builtin.lock().unwrap().insert(rel.to_string(), src.to_string());
    }

    pub fn spec(id: u32, widget: &str) -> InstanceSpec {
        InstanceSpec {
            instance: InstanceId(id),
            widget: WidgetRef::parse(widget).unwrap(),
            options: WidgetOptions::default(),
            bar: "main".into(),
            monitor: MonitorRef { index: 0, handle: 1, device: "\\\\.\\DISPLAY1".into(), primary: true, scale: 1.0 },
            pos: None,
        }
    }

    pub fn spec_with(id: u32, widget: &str, options: &[(&str, Value)]) -> InstanceSpec {
        let mut s = Harness::spec(id, widget);
        for (i, (k, v)) in options.iter().enumerate() {
            s.options
                .values
                .insert(k.to_string(), Spanned { value: v.clone(), pos: SourcePos::new("bar.toml", 7 + i as u32, 3) });
        }
        s
    }

    pub fn drain(&mut self) {
        let msgs: Vec<ScriptOut> = std::mem::take(&mut *self.out.lock().unwrap());
        for m in msgs {
            match m {
                ScriptOut::Trees(ts) => {
                    self.tree_batches += 1;
                    for t in ts {
                        self.latest.insert(t.instance, t);
                    }
                }
                ScriptOut::Plate { instance, plate } => {
                    self.plates.insert(instance, plate);
                }
                ScriptOut::Diagnostics(d) => self.diags.extend(d),
            }
        }
    }

    pub fn send(&mut self, msg: ScriptIn) -> bool {
        let r = self.rt.handle(msg);
        self.drain();
        r
    }

    pub fn start(&mut self, widgets: &[(u32, &str)]) {
        let specs = widgets.iter().map(|(i, w)| Harness::spec(*i, w)).collect();
        self.send(ScriptIn::SetInstances(specs));
    }

    pub fn start_specs(&mut self, specs: Vec<InstanceSpec>) {
        self.send(ScriptIn::SetInstances(specs));
    }

    pub fn run_due(&mut self) {
        self.rt.run_due();
        self.drain();
    }

    pub fn advance(&mut self, ms: u64) {
        self.clock.advance(Duration::from_millis(ms));
        self.run_due();
    }

    pub fn root(&self, id: u32) -> Option<&Element> {
        self.latest.get(&InstanceId(id)).and_then(|t| t.root.as_ref())
    }

    pub fn texts(&self, id: u32) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(r) = self.root(id) {
            collect_text(r, &mut out);
        }
        out
    }

    pub fn plate(&self, id: u32) -> Option<String> {
        self.plates.get(&InstanceId(id)).and_then(|p| p.as_ref()).map(|p| p.display_text())
    }

    pub fn status(&mut self) -> Vec<WidgetStatus> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.send(ScriptIn::Status { reply: tx });
        rx.recv().unwrap()
    }

    pub fn state_of(&mut self, id: u32) -> WidgetState {
        self.status().into_iter().find(|s| s.instance == InstanceId(id)).unwrap().state
    }

    pub fn requests(&self) -> Vec<String> {
        self.hub.lock().unwrap().iter().map(|r| format!("{r:?}")).collect()
    }

    /// Takes the recorded hub requests.
    pub fn take_requests(&self) -> Vec<HubRequest> {
        std::mem::take(&mut *self.hub.lock().unwrap())
    }

    pub fn io_reply(&mut self, id: IoId, owner: u32, kind: IoEventKind) {
        self.send(ScriptIn::Hub(HubEvent::Io(IoEvent { id, owner: InstanceId(owner), kind })));
    }

    pub fn click(&mut self, id: u32, handler: u32) {
        self.send(ScriptIn::Input(HandlerCall {
            instance: InstanceId(id),
            handler: rsb_core::element::HandlerId(handler),
            kind: InputKind::Click(Button::Left),
            x: 1.0,
            y: 2.0,
        }));
    }
}

pub fn collect_text(e: &Element, out: &mut Vec<String>) {
    if let Some(t) = &e.text {
        out.push(t.to_string());
    }
    for c in &e.children {
        collect_text(c, out);
    }
}

pub fn find<'a>(e: &'a Element, pred: &dyn Fn(&Element) -> bool) -> Option<&'a Element> {
    if pred(e) {
        return Some(e);
    }
    e.children.iter().find_map(|c| find(c, pred))
}

pub fn first_click(e: &Element) -> Option<u32> {
    find(e, &|n| n.handlers.click.is_some()).and_then(|n| n.handlers.click.map(|h| h.0))
}

pub fn map(pairs: &[(&str, Value)]) -> Value {
    Value::Map(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
}
