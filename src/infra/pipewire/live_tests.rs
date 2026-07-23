// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::*;
use crate::domain::routing::{CaptureConfig, CaptureMode, DeviceSelection};
use crate::dsp::ChannelPosition;
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct Process(Child);

impl Process {
    fn spawn(mut command: Command, description: &str) -> Self {
        Self(
            command
                .spawn()
                .unwrap_or_else(|err| panic!("failed to start {description}: {err}")),
        )
    }

    fn loopback(
        server: &IsolatedPipeWire,
        name: &str,
        channels: usize,
        channel_map: &str,
        capture_props: &str,
        playback_props: &str,
    ) -> Self {
        let mut command = server.client_command("pw-loopback");
        command
            .args([
                "--name",
                name,
                "--group",
                name,
                "--channels",
                &channels.to_string(),
                "--channel-map",
                channel_map,
                "--capture-props",
                capture_props,
                "--playback-props",
                playback_props,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        Self::spawn(command, "pw-loopback")
    }

    fn daemon(mut command: Command, description: &str) -> Self {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        Self::spawn(command, description)
    }

    fn assert_running(&mut self, description: &str) {
        if let Some(status) = self.0.try_wait().expect("query process status") {
            panic!("{description} exited with {status}");
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn checked_output(command: &mut Command, description: &str) -> std::process::Output {
    let output = command
        .output()
        .unwrap_or_else(|err| panic!("failed to run {description}: {err}"));
    assert!(
        output.status.success(),
        "{description} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

struct LingeringNode {
    id: u32,
    cleanup: Command,
}

impl Drop for LingeringNode {
    fn drop(&mut self) {
        let _ = self
            .cleanup
            .args(["destroy", &self.id.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

struct GraphDump(Vec<Value>);

impl GraphDump {
    fn objects<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Value> {
        self.0
            .iter()
            .filter(move |object| object.get("type").and_then(Value::as_str) == Some(kind))
    }

    fn node(&self, name: &str) -> Option<&Value> {
        self.objects("PipeWire:Interface:Node").find(|object| {
            object
                .pointer("/info/props/node.name")
                .and_then(Value::as_str)
                == Some(name)
        })
    }

    fn node_id(&self, name: &str) -> Option<u32> {
        self.node(name)?.get("id")?.as_u64()?.try_into().ok()
    }

    fn link_count(&self, output_node: u32, input_node: u32) -> usize {
        self.objects("PipeWire:Interface:Link")
            .filter(|object| {
                object
                    .pointer("/info/output-node-id")
                    .and_then(Value::as_u64)
                    == Some(u64::from(output_node))
                    && object
                        .pointer("/info/input-node-id")
                        .and_then(Value::as_u64)
                        == Some(u64::from(input_node))
            })
            .count()
    }

    fn ports(&self, node: u32, direction: &str) -> Vec<u32> {
        let mut ports: Vec<_> = self
            .objects("PipeWire:Interface:Port")
            .filter(|object| {
                object
                    .pointer("/info/props/node.id")
                    .and_then(Value::as_u64)
                    == Some(u64::from(node))
                    && object
                        .pointer("/info/props/port.direction")
                        .and_then(Value::as_str)
                        == Some(direction)
            })
            .filter_map(|object| object.get("id")?.as_u64()?.try_into().ok())
            .collect();
        ports.sort_unstable();
        ports
    }

    fn inactive(&self, name: &str) -> bool {
        self.node(name).is_none_or(|node| {
            node.pointer("/info/state").and_then(Value::as_str) != Some("running")
        })
    }
}

struct IsolatedPipeWire {
    runtime: TempDir,
    remote: PathBuf,
    pipewire: Option<Process>,
    wireplumber: Option<Process>,
}

impl IsolatedPipeWire {
    fn new() -> Self {
        let runtime = tempfile::tempdir().expect("create isolated PipeWire runtime");
        let config_dir = runtime.path().join("pipewire/pipewire.conf.d");
        std::fs::create_dir_all(&config_dir).expect("create PipeWire test config");
        std::fs::write(
            config_dir.join("10-audiotestsrc.conf"),
            "context.spa-libs = { audiotestsrc = audiotestsrc/libspa-audiotestsrc }\n",
        )
        .expect("configure PipeWire audio test source");
        let remote = runtime.path().join("pipewire-0");
        let mut server = Self {
            runtime,
            remote,
            pipewire: None,
            wireplumber: None,
        };
        server.restart();
        server
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        for key in [
            "DBUS_SESSION_BUS_ADDRESS",
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "PIPEWIRE_CONFIG_DIR",
            "PIPEWIRE_CONFIG_NAME",
            "PIPEWIRE_CONFIG_PREFIX",
            "PIPEWIRE_REMOTE",
            "SPA_PLUGIN_DIR",
            "WIREPLUMBER_CONFIG_DIR",
            "WIREPLUMBER_DATA_DIR",
            "WIREPLUMBER_MODULE_DIR",
        ] {
            command.env_remove(key);
        }
        let root = self.runtime.path();
        command.envs([
            ("HOME", root),
            ("PIPEWIRE_RUNTIME_DIR", root),
            ("XDG_RUNTIME_DIR", root),
            ("XDG_CONFIG_HOME", root),
            ("XDG_DATA_HOME", root),
            ("XDG_STATE_HOME", root),
            ("XDG_CACHE_HOME", root),
        ]);
        command
    }

    fn client_command(&self, program: &str) -> Command {
        let mut command = self.command(program);
        command.env("PIPEWIRE_REMOTE", &self.remote);
        command
    }

    fn restart(&mut self) {
        assert!(self.pipewire.is_none() && self.wireplumber.is_none());
        self.pipewire = Some(Process::daemon(self.command("pipewire"), "pipewire"));
        wait_for("isolated PipeWire socket", || {
            self.pipewire
                .as_mut()
                .expect("pipewire process")
                .assert_running("pipewire");
            self.remote.exists().then_some(())
        });
        self.wireplumber = Some(Process::daemon(
            self.client_command("wireplumber"),
            "wireplumber",
        ));
        wait_for("isolated WirePlumber", || {
            self.pipewire
                .as_mut()
                .expect("pipewire process")
                .assert_running("pipewire");
            self.wireplumber
                .as_mut()
                .expect("wireplumber process")
                .assert_running("wireplumber");
            self.server_available().then_some(())
        });
    }

    fn stop(&mut self) {
        self.wireplumber.take();
        self.pipewire.take();
    }

    fn server_available(&self) -> bool {
        self.client_command("pw-dump")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn dump(&self) -> GraphDump {
        let output = checked_output(&mut self.client_command("pw-dump"), "pw-dump");
        GraphDump(serde_json::from_slice(&output.stdout).expect("invalid pw-dump JSON"))
    }

    fn wait_for_node(&self, name: &str) {
        wait_for(&format!("node {name}"), || {
            self.dump().node(name).map(|_| ())
        });
    }

    fn create_node(&self, name: &str, props: &str) -> LingeringNode {
        checked_output(
            self.client_command("pw-cli")
                .args(["create-node", "adapter", props]),
            "create test node",
        );
        let id = wait_for(&format!("test node {name}"), || self.dump().node_id(name));
        LingeringNode {
            id,
            cleanup: self.client_command("pw-cli"),
        }
    }

    fn test_source(
        &self,
        name: &str,
        media_class: &str,
        channels: usize,
        channel_map: &str,
    ) -> LingeringNode {
        self.create_node(
            name,
            &format!(
                "{{ factory.name = audiotestsrc node.name = \"{name}\" media.class = \"{media_class}\" application.name = \"OpenMeters Live Signal\" application.id = \"org.openmeters.LiveSignal.{name}\" object.linger = true audio.channels = {channels} audio.position = {channel_map} node.param.Props = {{ live = true wave = 0 volume = 0.25 }} }}"
            ),
        )
    }

    fn passive_sink(&self, name: &str) -> LingeringNode {
        self.create_node(
            name,
            &format!(
                "{{ factory.name = support.null-audio-sink node.name = \"{name}\" media.class = \"Audio/Sink\" object.linger = true audio.position = [ FL, FR ] node.passive = true }}"
            ),
        )
    }

    fn link_nodes(&self, output_node: u32, input_node: u32, channels: usize, passive: bool) {
        let (outputs, inputs) = wait_for("test fixture ports", || {
            let graph = self.dump();
            let outputs = graph.ports(output_node, "out");
            let inputs = graph.ports(input_node, "in");
            (outputs.len() >= channels && inputs.len() >= channels).then_some((outputs, inputs))
        });
        for (output, input) in outputs.into_iter().zip(inputs).take(channels) {
            let mut command = self.client_command("pw-link");
            command.arg("--wait");
            if passive {
                command.arg("--passive");
            }
            checked_output(
                command.args([output.to_string(), input.to_string()]),
                "create test link",
            );
        }
    }
}

impl Drop for IsolatedPipeWire {
    fn drop(&mut self) {
        self.stop();
    }
}

struct ApplicationFixture {
    _source: Process,
    _signal: LingeringNode,
    route: Option<Process>,
}

impl ApplicationFixture {
    fn active(server: &IsolatedPipeWire, name: &str) -> Self {
        Self::active_with_layout(server, name, 2, "[ FL, FR ]")
    }

    fn active_with_layout(
        server: &IsolatedPipeWire,
        name: &str,
        channels: usize,
        channel_map: &str,
    ) -> Self {
        let target = format!("{name}.sink");
        let capture_props = format!(
            "{{ node.name = \"{target}\" media.class = \"Stream/Input/Audio\" stream.dont-remix = true node.virtual = true node.always-process = true node.want-driver = true node.passive = false }}"
        );
        let playback_props = format!(
            "{{ node.name = \"{target}.internal\" node.autoconnect = false node.always-process = true node.want-driver = true node.passive = false }}"
        );
        let route = Process::loopback(
            server,
            &format!("{name}.route"),
            channels,
            channel_map,
            &capture_props,
            &playback_props,
        );
        server.wait_for_node(&target);
        let source = application_source(server, name, Some(&target), channels, channel_map);
        let capture = wait_for("application capture node", || {
            server.dump().node_id(&format!("{name}.capture"))
        });
        let signal_channels = channels.min(2);
        let signal_map = if signal_channels == 1 {
            "[ MONO ]"
        } else {
            "[ FL, FR ]"
        };
        let signal = server.test_source(
            &format!("{name}.signal"),
            "Audio/Source",
            signal_channels,
            signal_map,
        );
        server.link_nodes(signal.id, capture, signal_channels, false);
        Self {
            _source: source,
            _signal: signal,
            route: Some(route),
        }
    }
}

fn application_source(
    server: &IsolatedPipeWire,
    name: &str,
    target: Option<&str>,
    channels: usize,
    channel_map: &str,
) -> Process {
    let capture_props = format!(
        "{{ node.name = \"{name}.capture\" node.autoconnect = false node.passive = true }}"
    );
    let route = target.map_or_else(
        || "node.autoconnect = false".to_owned(),
        |target| {
            format!(
                "node.autoconnect = true target.object = \"{target}\" node.dont-fallback = true node.dont-move = true"
            )
        },
    );
    let playback_props = format!(
        "{{ node.name = \"{name}.playback\" {route} node.passive = out application.name = \"OpenMeters Live Test\" application.id = \"org.openmeters.LiveTest.{name}\" }}"
    );
    Process::loopback(
        server,
        name,
        channels,
        channel_map,
        &capture_props,
        &playback_props,
    )
}

const WIDE_POSITIONS: [ChannelPosition; MAX_CAPTURE_CHANNELS] = [
    ChannelPosition::FrontRight,
    ChannelPosition::FrontLeft,
    ChannelPosition::FrontCenter,
    ChannelPosition::LowFrequency,
    ChannelPosition::RearRight,
    ChannelPosition::RearLeft,
    ChannelPosition::SideRight,
    ChannelPosition::SideLeft,
];

fn wide_device(server: &IsolatedPipeWire, name: &str) -> (Process, Process, LingeringNode) {
    let input = format!("{name}.input");
    let channel_map = "[ FR, FL, FC, LFE, RR, RL, SR, SL, TFR, TFL ]";
    let capture_props = format!(
        "{{ node.name = \"{input}\" media.class = \"Stream/Input/Audio\" node.virtual = true node.always-process = true node.want-driver = true node.passive = false }}"
    );
    let playback_props = format!(
        "{{ node.name = \"{name}.playback\" node.autoconnect = false node.virtual = false media.class = \"Audio/Source\" }}"
    );
    let loopback = Process::loopback(
        server,
        name,
        10,
        channel_map,
        &capture_props,
        &playback_props,
    );
    server.wait_for_node(&input);
    let source_name = format!("{name}.signal");
    let source = application_source(server, &source_name, Some(&input), 10, channel_map);
    let capture = wait_for("device signal capture node", || {
        server.dump().node_id(&format!("{source_name}.capture"))
    });
    let signal = server.test_source(&format!("{name}.tone"), "Audio/Source", 2, "[ FL, FR ]");
    server.link_nodes(signal.id, capture, 2, false);
    (loopback, source, signal)
}

fn property_is(node: &Value, property: &str, expected: &str) -> bool {
    node.pointer(&format!("/info/props/{property}"))
        .is_some_and(|value| {
            value.as_str() == Some(expected)
                || value
                    .as_bool()
                    .is_some_and(|value| expected.parse() == Ok(value))
                || value
                    .as_u64()
                    .is_some_and(|value| expected.parse() == Ok(value))
        })
}

fn wait_for<T>(description: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {description}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_mapped_signal(
    description: &str,
    audio: &mut AudioReader,
    positions: [ChannelPosition; MAX_CAPTURE_CHANNELS],
) {
    wait_for(description, || {
        let mut captured = false;
        audio.drain(Instant::now(), |span| {
            let CapturedSpan::Pcm { samples, format } = span else {
                return;
            };
            if format.channels != MAX_CAPTURE_CHANNELS || format.positions != positions {
                return;
            }
            let mut peaks = [0.0_f32; MAX_CAPTURE_CHANNELS];
            for frame in samples.chunks_exact(MAX_CAPTURE_CHANNELS) {
                for (peak, sample) in peaks.iter_mut().zip(frame) {
                    *peak = peak.max(sample.abs());
                }
            }
            captured |= peaks[..2].iter().all(|peak| *peak > 0.01)
                && peaks[2..].iter().all(|peak| *peak < 0.001);
        });
        captured.then_some(())
    });
}

#[test]
#[ignore = "requires pipewire, pw-cli, pw-link, pw-loopback, pw-dump, and wireplumber"]
fn live_backend_recovers_after_server_restart() {
    let mut server = IsolatedPipeWire::new();
    let mut backend =
        AudioBackend::start_with_socket(CaptureConfig::default(), Some(server.remote.clone()))
            .expect("start backend");
    let control = backend.control();
    let mut audio = backend.take_audio();
    let tap_name = format!("openmeters.tap.{}", std::process::id());
    let initial_tap = wait_for("initial backend session", || {
        server.dump().node_id(&tap_name)
    });

    let fixture_name = format!("openmeters-live-recovery-{}", std::process::id());
    let playback_name = format!("{fixture_name}.playback");
    let target_name = format!("{fixture_name}.sink");
    let fixture = ApplicationFixture::active(&server, &fixture_name);
    wait_for("initial recovery capture links", || {
        let graph = server.dump();
        let source = graph.node_id(&playback_name)?;
        let target = graph.node_id(&target_name)?;
        (graph.link_count(source, target) == 2 && graph.link_count(source, initial_tap) == 2)
            .then_some(())
    });
    audio.discard(Instant::now());
    wait_for_mapped_signal(
        "initial recovery PCM",
        &mut audio,
        ChannelPosition::SURROUND,
    );

    server.stop();
    wait_for("backend outage", || (!control.is_alive()).then_some(()));
    drop(fixture);

    server.restart();
    let recovered_tap = wait_for("recovered backend session", || {
        server.dump().node_id(&tap_name)
    });

    let recovered = ApplicationFixture::active(&server, &fixture_name);
    let (source, target) = wait_for("recovered capture links", || {
        let graph = server.dump();
        let source = graph.node_id(&playback_name)?;
        let target = graph.node_id(&target_name)?;
        (graph.link_count(source, target) == 2 && graph.link_count(source, recovered_tap) == 2)
            .then_some((source, target))
    });
    audio.discard(Instant::now());
    wait_for_mapped_signal(
        "recovered application PCM",
        &mut audio,
        ChannelPosition::SURROUND,
    );

    backend.shutdown();
    wait_for("owned link cleanup after shutdown", || {
        let graph = server.dump();
        (graph.node(&tap_name).is_none() && graph.link_count(source, target) == 2).then_some(())
    });
    drop(recovered);
}

#[test]
#[ignore = "requires pipewire, pw-cli, pw-link, pw-loopback, pw-dump, and wireplumber"]
fn live_capture_preserves_graph_invariants() {
    let server = IsolatedPipeWire::new();
    let mut backend =
        AudioBackend::start_with_socket(CaptureConfig::default(), Some(server.remote.clone()))
            .expect("start backend");
    let control = backend.control();
    let mut audio = backend.take_audio();
    let tap_name = format!("openmeters.tap.{}", std::process::id());
    let tap_id = wait_for("capture tap", || {
        audio.discard(Instant::now());
        server.dump().node_id(&tap_name)
    });
    let graph = server.dump();
    assert!(
        graph
            .node(&tap_name)
            .and_then(|node| node.pointer("/info/params/Format/0"))
            .is_none()
    );
    let reconnects = audio.reconnects();
    std::thread::sleep(Duration::from_millis(6_250));
    assert_eq!(server.dump().node_id(&tap_name), Some(tap_id));
    assert_eq!(audio.reconnects(), reconnects);

    let active_name = format!("openmeters-live-active-{}", std::process::id());
    let playback_name = format!("{active_name}.playback");
    let target_name = format!("{active_name}.sink");
    let mut active = ApplicationFixture::active(&server, &active_name);
    let (source_id, target_id, identity) = wait_for("active application fan-out", || {
        audio.discard(Instant::now());
        let graph = server.dump();
        let source = graph.node_id(&playback_name)?;
        let target = graph.node_id(&target_name)?;
        let identity = control.view().applications.first()?.identity.clone();
        (graph.link_count(source, target) == 2 && graph.link_count(source, tap_id) == 2)
            .then_some((source, target, identity))
    });
    let graph = server.dump();
    let tap = graph.node(&tap_name).expect("tap node");
    assert!(property_is(tap, "node.passive", "in"));
    assert!(property_is(tap, "node.always-process", "false"));
    assert!(property_is(tap, "node.latency", "256/48000"));
    assert_eq!(graph.link_count(source_id, target_id), 2);
    wait_for_mapped_signal(
        "captured application PCM",
        &mut audio,
        ChannelPosition::SURROUND,
    );

    let surround_name = format!("openmeters-live-surround-{}", std::process::id());
    let surround_playback = format!("{surround_name}.playback");
    let surround = ApplicationFixture::active_with_layout(
        &server,
        &surround_name,
        6,
        "[ FL, FR, FC, LFE, RL, RR ]",
    );
    wait_for("surround application mix", || {
        audio.discard(Instant::now());
        let graph = server.dump();
        let tap = graph.node(&tap_name)?;
        let source = graph.node_id(&surround_playback)?;
        (tap.get("id")?.as_u64() == Some(tap_id as u64)
            && graph.link_count(source, tap_id) == 6
            && graph.link_count(source_id, tap_id) == 2
            && tap.pointer("/info/params/Format/0/channels")?.as_u64()
                == Some(MAX_CAPTURE_CHANNELS as u64))
        .then_some(())
    });
    wait_for_mapped_signal(
        "captured surround application mix",
        &mut audio,
        ChannelPosition::SURROUND,
    );
    drop(surround);
    wait_for("stable application mix", || {
        audio.discard(Instant::now());
        let graph = server.dump();
        let tap = graph.node(&tap_name)?;
        (tap.get("id")?.as_u64() == Some(tap_id as u64)
            && graph.node(&surround_playback).is_none()
            && graph.link_count(source_id, tap_id) == 2
            && tap.pointer("/info/params/Format/0/channels")?.as_u64()
                == Some(MAX_CAPTURE_CHANNELS as u64))
        .then_some(())
    });

    let mut disabled = HashSet::new();
    disabled.insert(identity);
    assert!(control.configure(CaptureConfig {
        disabled_streams: disabled,
        ..Default::default()
    }));
    wait_for("application disable", || {
        audio.discard(Instant::now());
        (server.dump().link_count(source_id, tap_id) == 0).then_some(())
    });
    assert!(control.configure(CaptureConfig::default()));
    wait_for("application re-enable", || {
        audio.discard(Instant::now());
        (server.dump().link_count(source_id, tap_id) == 2).then_some(())
    });

    active.route.take();
    wait_for("route removal", || {
        audio.discard(Instant::now());
        let graph = server.dump();
        (graph.link_count(source_id, tap_id) == 0 && graph.inactive(&playback_name)).then_some(())
    });
    drop(active);

    let idle_name = format!("openmeters-live-idle-{}", std::process::id());
    let idle_playback = format!("{idle_name}.playback");
    let idle = application_source(&server, &idle_name, None, 2, "[ FL, FR ]");
    let idle_id = wait_for("unrouted application", || {
        server.dump().node_id(&idle_playback)
    });
    std::thread::sleep(Duration::from_millis(250));
    let graph = server.dump();
    assert_eq!(graph.node_id(&tap_name), Some(tap_id));
    assert_eq!(graph.link_count(idle_id, tap_id), 0);
    assert!(graph.inactive(&idle_playback));
    drop(idle);

    let paused_name = format!("openmeters-live-paused-{}", std::process::id());
    let paused_target = format!("{paused_name}.sink");
    let paused_sink = server.passive_sink(&paused_target);
    let target_id = paused_sink.id;
    let paused_playback = format!("{paused_name}.playback");
    let paused_source = application_source(&server, &paused_name, None, 2, "[ FL, FR ]");
    let paused_source_id = wait_for("paused application", || {
        server.dump().node_id(&paused_playback)
    });
    server.link_nodes(paused_source_id, target_id, 2, true);
    wait_for("passive tap of paused route", || {
        audio.discard(Instant::now());
        let graph = server.dump();
        (graph.link_count(paused_source_id, target_id) == 2
            && graph.link_count(paused_source_id, tap_id) == 2
            && graph.inactive(&paused_playback)
            && graph.inactive(&paused_target))
        .then_some(())
    });
    std::thread::sleep(Duration::from_millis(20));
    let mut idle_frames = 0;
    audio.drain(Instant::now(), |span| {
        if let CapturedSpan::Silence { frames, .. } = span {
            idle_frames += frames;
        }
    });
    assert!(idle_frames > 0, "paused capture did not advance silence");

    drop(paused_source);
    drop(paused_sink);

    let device_name = format!("openmeters-live-device-{}", std::process::id());
    let device_node = format!("{device_name}.playback");
    let device = wide_device(&server, &device_name);
    let (token, target) = wait_for("wide device discovery", || {
        let view = control.view();
        let token = view
            .devices
            .iter()
            .find(|token| token.as_ref() == device_node)?
            .to_string();
        let target = server
            .dump()
            .node(&device_node)?
            .pointer("/info/props/object.serial")?
            .as_u64()?
            .to_string();
        Some((token, target))
    });
    assert!(control.configure(CaptureConfig {
        mode: CaptureMode::Device,
        device: DeviceSelection::Device(token),
        ..Default::default()
    }));
    wait_for("wide device target", || {
        audio.discard(Instant::now());
        let graph = server.dump();
        let tap = graph.node(&tap_name)?;
        (property_is(tap, "target.object", &target)
            && tap.pointer("/info/params/Format/0/channels")?.as_u64()
                == Some(MAX_CAPTURE_CHANNELS as u64))
        .then_some(())
    });
    wait_for_mapped_signal("captured device PCM", &mut audio, WIDE_POSITIONS);

    assert!(control.configure(CaptureConfig {
        mode: CaptureMode::Device,
        device: DeviceSelection::Device("openmeters-definitely-missing".into()),
        ..Default::default()
    }));
    wait_for("missing device idle fallback", || {
        audio.discard(Instant::now());
        let graph = server.dump();
        let tap = graph.node(&tap_name)?;
        (tap.pointer("/info/props/target.object").is_none()
            && property_is(tap, "node.autoconnect", "false")
            && property_is(tap, "node.passive", "in"))
        .then_some(())
    });
    drop(device);

    backend.shutdown();
    wait_for("tap cleanup", || {
        server.dump().node(&tap_name).is_none().then_some(())
    });
}
