//! Decisions of the Bluetooth service against a recorder: no radio, pairing or connection is ever
//! touched here.

use rsb_core::services::check_value;

use super::*;

#[derive(Default)]
struct Recorder {
    radio: RadioInfo,
    allowed: bool,
    classic: Vec<Dev>,
    le: Vec<Dev>,
    calls: Vec<String>,
    kind_reads: u32,
}

impl Bt for Recorder {
    fn radios(&mut self) -> Result<RadioInfo, String> {
        Ok(self.radio)
    }
    fn can_toggle(&mut self) -> bool {
        self.allowed
    }
    fn paired(&mut self, le: bool) -> Result<Vec<Dev>, String> {
        self.calls.push(format!("paired {le}"));
        Ok(if le { self.le.clone() } else { self.classic.clone() })
    }
    fn kind(&mut self, _: &str, le: bool) -> Result<&'static str, String> {
        self.kind_reads += 1;
        Ok(if le { "input" } else { "audio" })
    }
    fn battery(&mut self, address: u64) -> Option<f64> {
        (address == 1).then_some(55.0)
    }
    fn set_radios(&mut self, on: bool) -> Result<(), String> {
        self.calls.push(format!("radios {on}"));
        Ok(())
    }
    fn set_connected(&mut self, _: &GUID, connect: bool) -> Result<(), String> {
        self.calls.push(format!("connect {connect}"));
        Ok(())
    }
    fn forget(&mut self, dev: &Dev) -> Result<(), String> {
        self.calls.push(format!("forget {}", dev.id));
        Ok(())
    }
}

fn dev(id: &str, address: Option<u64>, le: bool, connected: bool) -> Dev {
    Dev {
        id: id.into(),
        name: format!("name of {id}"),
        connected,
        container: Some(GUID::from_u128(9)),
        kind: "other",
        battery: None,
        can_connect: false,
        address,
        le,
    }
}

fn on() -> RadioInfo {
    RadioInfo { available: true, enabled: true }
}

fn published(ids: &[(&str, bool)], can_toggle: bool) -> Published {
    Published { devices: ids.iter().map(|(i, c)| (i.to_string(), *c)).collect(), can_toggle }
}

#[test]
fn addresses_are_read_from_text_and_from_device_ids() {
    assert_eq!(parse_mac("aa:bb:cc:dd:ee:ff"), Some(0xaabb_ccdd_eeff));
    assert_eq!(parse_mac("AA-BB-CC-DD-EE-FF"), Some(0xaabb_ccdd_eeff));
    assert_eq!(parse_mac("aabbccddeeff"), Some(0xaabb_ccdd_eeff));
    assert_eq!(parse_mac("aabbccddeef"), None);
    assert_eq!(parse_mac("gg:bb:cc:dd:ee:ff"), None);
    assert_eq!(address_from_id("Bluetooth#Bluetooth00:11:22:33:44:55-aa:bb:cc:dd:ee:ff"), Some(0xaabb_ccdd_eeff));
    assert_eq!(address_from_id("BluetoothLE#BluetoothLE00:11:22:33:44:55-aa:bb:cc:dd:ee:ff"), Some(0xaabb_ccdd_eeff));
    assert_eq!(address_from_id("not a device id"), None);
}

#[test]
fn kinds_come_from_the_class_of_device_and_the_appearance() {
    assert_eq!(kind_from_major(4), "audio");
    assert_eq!(kind_from_major(5), "input");
    assert_eq!(kind_from_major(2), "phone");
    assert_eq!(kind_from_major(1), "computer");
    assert_eq!(kind_from_major(7), "wearable");
    assert_eq!(kind_from_major(0), "other");
    assert_eq!(kind_from_appearance(15), "input");
    assert_eq!(kind_from_appearance(3), "wearable");
    assert_eq!(kind_from_appearance(0), "other");
}

#[test]
fn a_dual_mode_device_is_listed_once_with_the_classic_entry() {
    let merged = merge(
        vec![dev("c1", Some(1), false, false), dev("c2", Some(2), false, false)],
        vec![dev("l1", Some(1), true, true), dev("l3", Some(3), true, false), dev("l4", None, true, false)],
    );
    let ids: Vec<&str> = merged.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(ids, ["c1", "c2", "l3", "l4"]);
    assert!(merged[0].connected, "connected on either list counts");
}

#[test]
fn the_list_is_capped() {
    let many = (0..200).map(|i| dev(&format!("d{i}"), Some(i + 100), false, false)).collect();
    assert_eq!(merge(many, Vec::new()).len(), MAX_DEVICES);
}

#[test]
fn a_read_fills_kind_battery_and_can_connect_and_fits_the_schema() {
    let mut bt = Recorder { radio: on(), allowed: true, ..Recorder::default() };
    bt.classic = vec![dev("c1", Some(1), false, true)];
    bt.le = vec![dev("l2", Some(2), true, false)];
    let mut kinds = HashMap::new();
    let s = read(&mut bt, &mut kinds).unwrap();
    assert_eq!(s.devices.len(), 2);
    let audio = s.devices.iter().find(|d| d.id == "c1").unwrap();
    assert_eq!((audio.kind, audio.can_connect, audio.battery), ("audio", true, Some(55.0)));
    let input = s.devices.iter().find(|d| d.id == "l2").unwrap();
    assert_eq!((input.kind, input.can_connect, input.battery), ("input", false, None));
    assert_eq!(check_value(ServiceId::Bluetooth, &value_of(&s)), Ok(()));
    // The second read opens no device object again.
    read(&mut bt, &mut kinds).unwrap();
    assert_eq!(bt.kind_reads, 2);
}

#[test]
fn no_radio_gives_an_empty_valid_value_and_no_list_call() {
    let mut bt = Recorder::default();
    let s = read(&mut bt, &mut HashMap::new()).unwrap();
    assert!(bt.calls.is_empty(), "no device list is asked for without a radio");
    let v = value_of(&s);
    assert_eq!(check_value(ServiceId::Bluetooth, &v), Ok(()));
    assert_eq!(v.get("available").and_then(Value::as_bool), Some(false));
}

#[test]
fn devices_read_as_not_connected_while_the_radio_is_off() {
    let mut bt = Recorder { radio: RadioInfo { available: true, enabled: false }, ..Recorder::default() };
    bt.classic = vec![dev("c1", Some(1), false, true)];
    let s = read(&mut bt, &mut HashMap::new()).unwrap();
    let Some(Value::List(list)) = value_of(&s).get("devices").cloned() else { panic!("a list") };
    assert_eq!(list[0].get("connected").and_then(Value::as_bool), Some(false));
}

#[test]
fn unknown_ids_are_refused_before_anything_is_queued() {
    let p = published(&[("c1", true), ("l2", false)], true);
    for bad in ["", "c1 ", "C1", "..", "C:\\Windows\\System32\\cmd.exe", "{GUID}\\cmd.exe"] {
        for c in [
            BluetoothCommand::Connect(bad.into()),
            BluetoothCommand::Disconnect(bad.into()),
            BluetoothCommand::Forget(bad.into()),
        ] {
            assert_eq!(check_command(&c, &p), Err("no Bluetooth device with that id".into()), "{c:?}");
        }
    }
    assert!(check_command(&BluetoothCommand::Connect("c1".into()), &p).is_ok());
    assert!(check_command(&BluetoothCommand::Disconnect("c1".into()), &p).is_ok());
    assert!(check_command(&BluetoothCommand::Forget("l2".into()), &p).is_ok());
    assert!(check_command(&BluetoothCommand::Connect("l2".into()), &p).is_err(), "not an audio device");
    assert!(check_command(&BluetoothCommand::OpenSettings, &p).is_ok());
    assert!(check_command(&BluetoothCommand::Rescan, &p).is_ok());
}

#[test]
fn switching_the_radio_needs_the_access_windows_grants() {
    assert!(check_command(&BluetoothCommand::SetEnabled(false), &published(&[], false)).is_err());
    assert!(check_command(&BluetoothCommand::SetEnabled(false), &published(&[], true)).is_ok());
}

#[test]
fn commands_reach_the_system_only_for_a_device_in_the_current_list() {
    let mut bt = Recorder::default();
    let mut audio = dev("c1", Some(1), false, false);
    audio.can_connect = true;
    let other = dev("l2", Some(2), true, false);
    let list = [audio, other];
    execute(&mut bt, &BluetoothCommand::Connect("c1".into()), &list).unwrap();
    execute(&mut bt, &BluetoothCommand::Disconnect("c1".into()), &list).unwrap();
    execute(&mut bt, &BluetoothCommand::Forget("l2".into()), &list).unwrap();
    execute(&mut bt, &BluetoothCommand::SetEnabled(false), &list).unwrap();
    execute(&mut bt, &BluetoothCommand::Rescan, &list).unwrap();
    assert_eq!(bt.calls, ["connect true", "connect false", "forget l2", "radios false"], "a rescan touches no radio");
    bt.calls.clear();
    assert!(execute(&mut bt, &BluetoothCommand::Connect("l2".into()), &list).is_err(), "not connectable");
    assert!(execute(&mut bt, &BluetoothCommand::Forget("gone".into()), &list).is_err());
    assert!(bt.calls.is_empty());
}

#[test]
fn open_settings_asks_the_opener_for_the_settings_page_only() {
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = seen.clone();
    let mut svc = BluetoothService::with_opener(Arc::new(move |uri| {
        log.lock().unwrap().push(uri.to_string());
        Ok(())
    }));
    // The service is stopped: every command, including this one, is refused.
    assert!(svc.command(ServiceCommand::Bluetooth(BluetoothCommand::OpenSettings)).is_err());
    svc.shared = Some(Arc::new(Shared {
        wake: auto_event().unwrap(),
        commands: Mutex::new(VecDeque::new()),
        published: Mutex::new(Published::default()),
    }));
    svc.command(ServiceCommand::Bluetooth(BluetoothCommand::OpenSettings)).unwrap();
    assert_eq!(seen.lock().unwrap().as_slice(), ["ms-settings:bluetooth"]);
    assert!(svc.shared.as_ref().unwrap().commands.lock().unwrap().is_empty());
    assert!(crate::real::apps::check_uri(SETTINGS_PAGE).is_ok());
}

#[test]
fn the_paired_device_lists_can_be_read_without_a_radio_or_a_pairing() {
    let counts = std::thread::spawn(|| {
        // SAFETY: the WinRT apartment of this test thread as MTA, balanced below.
        let ro = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        let classic = winrt::paired(false).map(|l| l.len());
        let le = winrt::paired(true).map(|l| l.len());
        let radios = winrt::bluetooth_radios().map(|r| r.len());
        if ro.is_ok() {
            // SAFETY: balances RoInitialize.
            unsafe { RoUninitialize() };
        }
        (classic, le, radios)
    })
    .join()
    .unwrap();
    println!("bluetooth lists: classic {:?}, le {:?}, radios {:?}", counts.0.is_ok(), counts.1.is_ok(), counts.2);
    assert!(counts.0.is_ok() && counts.1.is_ok() && counts.2.is_ok(), "{counts:?}");
}
