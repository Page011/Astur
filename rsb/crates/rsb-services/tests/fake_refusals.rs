//! A fake refuses what the real service refuses, with the same text and with nothing changed or
//! recorded. The commands go straight to the fake service (no hub), so the answer is the `Err`
//! itself and not a log line.

use rsb_core::services::{
    AppsCommand, AudioDeviceCommand, BluetoothCommand, PowerCommand, PowerMode, ServiceCommand, ServiceId, WifiCommand,
};
use rsb_core::value::Value;
use rsb_services::fake::FakeWorld;
use rsb_services::{HubOptions, Service, ServiceFactory};

fn service(world: &FakeWorld, id: ServiceId) -> Box<dyn Service> {
    world.create(id, &HubOptions::default())
}

fn refused(world: &FakeWorld, id: ServiceId, command: ServiceCommand) -> String {
    let before = world.commands().len();
    let e = service(world, id).command(command).expect_err("the fake refuses it");
    assert_eq!(world.commands().len(), before, "a refused command is not recorded");
    e
}

fn accepted(world: &FakeWorld, id: ServiceId, command: ServiceCommand) {
    let before = world.commands().len();
    service(world, id).command(command).expect("the fake accepts it");
    assert_eq!(world.commands().len(), before + 1, "an accepted command is recorded");
}

/// A path form, a GUID with a program, a parent folder, a control character, the wrong case, empty
/// and a huge id: none was ever published.
fn unpublished(of: &str) -> Vec<String> {
    vec![
        r"C:\Windows\System32\cmd.exe".into(),
        r"{00000000-0000-0000-0000-000000000000}\cmd.exe".into(),
        r"..\..\x".into(),
        format!("{of}\n"),
        of.to_uppercase(),
        String::new(),
        "a".repeat(2000),
    ]
}

#[test]
fn bluetooth_refuses_unpublished_devices_and_devices_that_cannot_connect() {
    let world = FakeWorld::with_sample_values();
    for id in unpublished("bt-1") {
        for c in [
            BluetoothCommand::Connect(id.clone()),
            BluetoothCommand::Disconnect(id.clone()),
            BluetoothCommand::Forget(id.clone()),
        ] {
            assert_eq!(
                refused(&world, ServiceId::Bluetooth, ServiceCommand::Bluetooth(c)),
                "no Bluetooth device with that id"
            );
        }
    }
    // bt-2 is a mouse: published, but not connectable from here.
    for c in [BluetoothCommand::Connect("bt-2".into()), BluetoothCommand::Disconnect("bt-2".into())] {
        assert_eq!(
            refused(&world, ServiceId::Bluetooth, ServiceCommand::Bluetooth(c)),
            "that device cannot be connected from here"
        );
    }
    accepted(&world, ServiceId::Bluetooth, ServiceCommand::Bluetooth(BluetoothCommand::Connect("bt-3".into())));
    accepted(&world, ServiceId::Bluetooth, ServiceCommand::Bluetooth(BluetoothCommand::Forget("bt-2".into())));
    accepted(&world, ServiceId::Bluetooth, ServiceCommand::Bluetooth(BluetoothCommand::OpenSettings));

    // A radio Windows does not let this app switch.
    let locked = FakeWorld::new();
    locked.set(
        ServiceId::Bluetooth,
        Value::Map(
            [
                ("available".to_string(), Value::Bool(true)),
                ("enabled".to_string(), Value::Bool(true)),
                ("can_toggle".to_string(), Value::Bool(false)),
                ("devices".to_string(), Value::List(Vec::new())),
            ]
            .into_iter()
            .collect(),
        ),
    );
    assert_eq!(
        refused(&locked, ServiceId::Bluetooth, ServiceCommand::Bluetooth(BluetoothCommand::SetEnabled(false))),
        "Windows does not let this app switch Bluetooth"
    );
}

#[test]
fn apps_refuse_ids_the_list_does_not_hold_and_pages_other_than_settings() {
    let world = FakeWorld::with_sample_values();
    for id in unpublished("sample.editor") {
        assert_eq!(
            refused(&world, ServiceId::Apps, ServiceCommand::Apps(AppsCommand::Launch(id))),
            "no app with that id"
        );
    }
    for uri in ["https://example.org", "file:///c:/x", "shell:startup", "calc.exe", "ms-settings:bluetooth now"] {
        assert_eq!(
            refused(&world, ServiceId::Apps, ServiceCommand::Apps(AppsCommand::OpenUri(uri.into()))),
            "only Windows settings and screen clip pages can be opened",
            "{uri}"
        );
    }
    accepted(&world, ServiceId::Apps, ServiceCommand::Apps(AppsCommand::Launch("sample.editor".into())));
    accepted(&world, ServiceId::Apps, ServiceCommand::Apps(AppsCommand::OpenUri("ms-settings:bluetooth".into())));
}

#[test]
fn wifi_refuses_networks_that_are_not_saved_and_scans_without_location_access() {
    let world = FakeWorld::with_sample_values();
    for ssid in ["Sample Guest", "Sample Nowhere", "SAMPLE HOME", "", &"a".repeat(40)] {
        assert_eq!(
            refused(&world, ServiceId::Wifi, ServiceCommand::Wifi(WifiCommand::Connect(ssid.into()))),
            "no saved network with that name",
            "{ssid}"
        );
    }
    accepted(&world, ServiceId::Wifi, ServiceCommand::Wifi(WifiCommand::Connect("Sample Cafe".into())));
    accepted(&world, ServiceId::Wifi, ServiceCommand::Wifi(WifiCommand::Scan));
    accepted(&world, ServiceId::Wifi, ServiceCommand::Wifi(WifiCommand::Disconnect));

    let no_access = FakeWorld::new();
    no_access.set(
        ServiceId::Wifi,
        Value::Map(
            [
                ("available".to_string(), Value::Bool(true)),
                ("scanning".to_string(), Value::Bool(false)),
                ("networks".to_string(), Value::List(Vec::new())),
                ("access".to_string(), Value::Str("needs_location".into())),
            ]
            .into_iter()
            .collect(),
        ),
    );
    for c in [WifiCommand::Scan, WifiCommand::Disconnect] {
        assert_eq!(
            refused(&no_access, ServiceId::Wifi, ServiceCommand::Wifi(c)),
            "location access is needed for Wi-Fi"
        );
    }
}

#[test]
fn audio_devices_refuse_unpublished_ids_and_a_switch_that_is_not_available() {
    let world = FakeWorld::with_sample_values();
    for id in unpublished("fake-out-headset") {
        for c in [
            AudioDeviceCommand::SetDefault(id.clone()),
            AudioDeviceCommand::SetVolume(id.clone(), 10.0),
            AudioDeviceCommand::SetMuted(id.clone(), true),
        ] {
            assert_eq!(
                refused(&world, ServiceId::Audio, ServiceCommand::AudioDevice(c)),
                "no audio device with that id"
            );
        }
    }
    assert_eq!(
        refused(
            &world,
            ServiceId::Audio,
            ServiceCommand::AudioDevice(AudioDeviceCommand::SetVolume("fake-out-headset".into(), f32::NAN))
        ),
        "the volume is not a number"
    );
    accepted(
        &world,
        ServiceId::Audio,
        ServiceCommand::AudioDevice(AudioDeviceCommand::SetVolume("fake-out-headset".into(), 30.0)),
    );

    // Windows without the interface that switches the default.
    let w = FakeWorld::new();
    w.set(ServiceId::Audio, sample_audio_without_switch());
    assert_eq!(
        refused(
            &w,
            ServiceId::Audio,
            ServiceCommand::AudioDevice(AudioDeviceCommand::SetDefault("fake-out-headset".into()))
        ),
        "switching the output is not available on this Windows"
    );
}

/// The sample audio value with `switch_available` off.
fn sample_audio_without_switch() -> Value {
    let device = |id: &str, default: bool| {
        Value::Map(
            [
                ("id".to_string(), Value::Str(id.into())),
                ("name".to_string(), Value::Str("Sample".into())),
                ("kind".to_string(), Value::Str("speakers".into())),
                ("default".to_string(), Value::Bool(default)),
                ("volume".to_string(), Value::Num(50.0)),
                ("muted".to_string(), Value::Bool(false)),
                ("bluetooth".to_string(), Value::Bool(false)),
            ]
            .into_iter()
            .collect(),
        )
    };
    Value::Map(
        [
            ("volume".to_string(), Value::Num(50.0)),
            ("muted".to_string(), Value::Bool(false)),
            ("devices".to_string(), Value::List(vec![device("fake-out-headset", true)])),
            ("inputs".to_string(), Value::List(Vec::new())),
            ("switch_available".to_string(), Value::Bool(false)),
        ]
        .into_iter()
        .collect(),
    )
}

#[test]
fn power_refuses_a_mode_change_the_pc_cannot_make_and_records_the_session_actions() {
    let world = FakeWorld::with_sample_values();
    accepted(&world, ServiceId::Power, ServiceCommand::Power(PowerCommand::SetMode(PowerMode::Performance)));
    for c in
        [PowerCommand::Lock, PowerCommand::Sleep, PowerCommand::Restart, PowerCommand::ShutDown, PowerCommand::SignOut]
    {
        accepted(&world, ServiceId::Power, ServiceCommand::Power(c));
    }

    let none = FakeWorld::new();
    none.set(
        ServiceId::Power,
        Value::Map(
            [
                ("mode".to_string(), Value::Str("balanced".into())),
                ("mode_available".to_string(), Value::Bool(false)),
                ("can_sleep".to_string(), Value::Bool(true)),
            ]
            .into_iter()
            .collect(),
        ),
    );
    assert_eq!(
        refused(&none, ServiceId::Power, ServiceCommand::Power(PowerCommand::SetMode(PowerMode::Efficiency))),
        "changing the power mode is not available on this Windows"
    );
}
