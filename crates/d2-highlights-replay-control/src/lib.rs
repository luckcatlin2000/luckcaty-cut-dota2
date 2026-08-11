use d2_highlights_core::{CameraPlan, DirectorDocument, TimelineDocument};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub const CONTROL_SCHEMA_VERSION: &str = "1.0";
pub const VCONSOLE_PORT: u16 = 29_000;
const VCONSOLE_HEADER_SIZE: usize = 12;
const VCONSOLE_COMMAND_VERSION: u32 = 0x00D4_0000;

const ALLOWED_COMMANDS: &[&str] = &[
    "echo",
    "find",
    "cvarlist",
    "help",
    "playdemo",
    "sv_cheats",
    "demo_info",
    "demo_goto",
    "demo_gototick",
    "demo_pause",
    "demo_resume",
    "demo_togglepause",
    "demo_pauseatservertick",
    "demo_timescale",
    "startmovie",
    "endmovie",
    "cl_drawhud",
    "dota_hide_cursor",
    "dota_hud_hide_mainhud",
    "dota_hud_hide_topbar",
    "dota_hud_hide_minimap",
    "dota_hud_hide_overlaymap",
    "dota_show_itempickups",
    "r_draw_selected_ring",
    "r_drawpanorama",
    "dota_spectator_hudhide",
    "dota_spectator_hudshow",
    "dota_spectator_options_enabled",
    "dota_spectator_mode",
    "dota_spectator_hero_index",
    "dota_camera_center",
    "dota_camera_center_on_entity",
    "dota_camera_center_on_hero",
    "dota_camera_focus_player",
    "dota_camera_allow_freecam",
    "dota_camera_distance",
    "dota_camera_get_lookatpos",
    "dota_camera_get_pos",
    "dota_camera_lerp_position",
    "dota_camera_set_lookatpos",
];

#[derive(Debug, Error)]
pub enum ReplayControlError {
    #[error("director and timeline source hashes do not match")]
    SourceMismatch,
    #[error("replay duration or playback tick count is invalid")]
    InvalidReplayClock,
    #[error("VConsole command is empty")]
    EmptyCommand,
    #[error("VConsole command contains a newline or NUL byte")]
    UnsafeCommand,
    #[error("VConsole command is not allowlisted: {0}")]
    CommandNotAllowed(String),
    #[error("VConsole command is too long: {0} bytes")]
    CommandTooLong(usize),
    #[error("VConsole probe timed out before the echo marker was observed")]
    MarkerTimeout,
    #[error("system clock is before the Unix epoch")]
    Clock,
    #[error("VConsole I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ReplayControlTarget {
    pub host: String,
    pub port: u16,
    pub protocol: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ScheduledCommand {
    pub source_time_seconds: f32,
    pub source_tick: u32,
    pub command: String,
    pub purpose: String,
    pub expected_confirmation: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ReplayBeatControl {
    pub kind: String,
    pub source_start_seconds: f32,
    pub source_start_tick: u32,
    pub source_end_tick: u32,
    pub playback_speed: f32,
    pub camera_intent: CameraPlan,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera_command: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ReplaySegmentControl {
    pub candidate_id: String,
    pub source_start_tick: u32,
    pub source_peak_tick: u32,
    pub source_end_tick: u32,
    pub commands: Vec<ScheduledCommand>,
    pub beats: Vec<ReplayBeatControl>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ReplayControlDocument {
    pub schema_version: String,
    pub source_sha256: String,
    pub ticks_per_second: f32,
    pub tick_mapping: String,
    pub target: ReplayControlTarget,
    pub launch_arguments: Vec<String>,
    pub safety_rules: Vec<String>,
    pub segments: Vec<ReplaySegmentControl>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct VConsoleProbeReport {
    pub host: String,
    pub port: u16,
    pub connected: bool,
    pub marker_seen: bool,
    pub read_chunks: usize,
    pub bytes_received: usize,
    pub elapsed_milliseconds: u128,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct VConsoleExecutionReport {
    pub host: String,
    pub port: u16,
    pub commands: Vec<String>,
    pub acknowledged: bool,
    pub initial_bytes_drained: usize,
    pub response_bytes_received: usize,
    pub console_excerpt: Vec<String>,
    pub elapsed_milliseconds: u128,
}

pub fn build_replay_control_plan(
    director: &DirectorDocument,
    timeline: &TimelineDocument,
) -> Result<ReplayControlDocument, ReplayControlError> {
    if director.source_sha256 != timeline.source_sha256 {
        return Err(ReplayControlError::SourceMismatch);
    }
    if timeline.replay.playback_ticks <= 0 || timeline.replay.playback_time_seconds <= 0.0 {
        return Err(ReplayControlError::InvalidReplayClock);
    }

    let ticks_per_second =
        timeline.replay.playback_ticks as f32 / timeline.replay.playback_time_seconds;
    let last_tick = timeline.replay.playback_ticks as u32;
    let to_tick = |seconds: f32, peak_seconds: f32, peak_tick: u32| {
        (peak_tick as f32 + ((seconds - peak_seconds) * ticks_per_second))
            .round()
            .clamp(0.0, last_tick as f32) as u32
    };

    let segments = director
        .segments
        .iter()
        .map(|segment| {
            let source_start_tick = to_tick(
                segment.source_start_seconds,
                segment.source_peak_seconds,
                segment.source_peak_tick,
            );
            let source_peak_tick = segment.source_peak_tick.min(last_tick);
            let source_end_tick = to_tick(
                segment.source_end_seconds,
                segment.source_peak_seconds,
                segment.source_peak_tick,
            );
            let commands = vec![
                scheduled_command(
                    segment.source_start_seconds,
                    source_start_tick,
                    "demo_resume".to_string(),
                    "Ensure the demo simulation can advance before seeking.",
                    "The demo tick begins advancing.",
                ),
                scheduled_command(
                    segment.source_start_seconds,
                    source_start_tick,
                    format!("demo_goto {source_start_tick} absolute pause"),
                    "Seek to the absolute pre-roll tick and pause natively on arrival.",
                    "Console emits Demo Skipping and the replay enters paused state at the target.",
                ),
                scheduled_command(
                    segment.source_start_seconds,
                    source_start_tick,
                    "demo_timescale 1.000".to_string(),
                    "Reset replay speed before applying beat-level changes.",
                    "Console reports timescale 1.000.",
                ),
                scheduled_command(
                    segment.source_start_seconds,
                    source_start_tick,
                    "demo_resume".to_string(),
                    "Resume only after capture and frame checks are ready.",
                    "Demo tick advances and captured frames are non-blank.",
                ),
            ];
            let beats = segment
                .beats
                .iter()
                .map(|beat| {
                    let source_start_tick = to_tick(
                        beat.source_start_seconds,
                        segment.source_peak_seconds,
                        segment.source_peak_tick,
                    );
                    let source_end_tick = to_tick(
                        beat.source_end_seconds,
                        segment.source_peak_seconds,
                        segment.source_peak_tick,
                    );
                    let camera_command = if beat.camera.mode == "auto_directed" {
                        None
                    } else {
                        beat.camera
                            .target_hero
                            .as_deref()
                            .and_then(|hero| {
                                nearest_hero_location(
                                    timeline,
                                    hero,
                                    source_start_tick.saturating_add(source_end_tick) / 2,
                                )
                            })
                            .map(|(x, y)| format!("dota_camera_set_lookatpos {x:.3} {y:.3}"))
                    };
                    ReplayBeatControl {
                        kind: beat.kind.clone(),
                        source_start_seconds: beat.source_start_seconds,
                        source_start_tick,
                        source_end_tick,
                        playback_speed: beat.playback_speed,
                        camera_intent: beat.camera.clone(),
                        camera_command,
                    }
                })
                .collect();

            ReplaySegmentControl {
                candidate_id: segment.candidate_id.clone(),
                source_start_tick,
                source_peak_tick,
                source_end_tick,
                commands,
                beats,
            }
        })
        .collect();

    Ok(ReplayControlDocument {
        schema_version: CONTROL_SCHEMA_VERSION.to_string(),
        source_sha256: director.source_sha256.clone(),
        ticks_per_second,
        tick_mapping: "candidate_anchor_tick_relative".to_string(),
        target: ReplayControlTarget {
            host: Ipv4Addr::LOCALHOST.to_string(),
            port: VCONSOLE_PORT,
            protocol: "source2_vconsole2_cmnd_d4".to_string(),
        },
        launch_arguments: vec![
            "-vconsole".to_string(),
            "-console".to_string(),
            "-novid".to_string(),
        ],
        safety_rules: vec![
            "localhost_only".to_string(),
            "offline_replay_only".to_string(),
            "allowlisted_commands_only".to_string(),
            "no_game_binary_modification".to_string(),
            "do_not_reuse_an_online_game_process".to_string(),
        ],
        segments,
    })
}

fn nearest_hero_location(
    timeline: &TimelineDocument,
    hero: &str,
    target_tick: u32,
) -> Option<(f32, f32)> {
    const MAX_TICK_DISTANCE: u32 = 300;
    timeline
        .events
        .iter()
        .filter_map(|event| {
            let x = event.location_x?;
            let y = event.location_y?;
            let belongs_to_hero =
                event.attacker.as_deref() == Some(hero) || event.target.as_deref() == Some(hero);
            if !belongs_to_hero {
                return None;
            }
            let distance = event.tick.abs_diff(target_tick);
            (distance <= MAX_TICK_DISTANCE).then_some((distance, x, y))
        })
        .min_by_key(|(distance, _, _)| *distance)
        .map(|(_, x, y)| (x, y))
}

fn scheduled_command(
    source_time_seconds: f32,
    source_tick: u32,
    command: String,
    purpose: &str,
    expected_confirmation: &str,
) -> ScheduledCommand {
    ScheduledCommand {
        source_time_seconds,
        source_tick,
        command,
        purpose: purpose.to_string(),
        expected_confirmation: expected_confirmation.to_string(),
    }
}

pub fn build_cmnd_packet(command: &str) -> Result<Vec<u8>, ReplayControlError> {
    validate_command(command)?;
    let command_bytes = command.as_bytes();
    let total_length = VCONSOLE_HEADER_SIZE + command_bytes.len() + 1;
    let total_length = u16::try_from(total_length)
        .map_err(|_| ReplayControlError::CommandTooLong(command_bytes.len()))?;

    let mut packet = Vec::with_capacity(total_length as usize);
    packet.extend_from_slice(b"CMND");
    packet.extend_from_slice(&VCONSOLE_COMMAND_VERSION.to_be_bytes());
    packet.extend_from_slice(&total_length.to_be_bytes());
    packet.extend_from_slice(&0_u16.to_be_bytes());
    packet.extend_from_slice(command_bytes);
    packet.push(0);
    Ok(packet)
}

pub fn validate_command(command: &str) -> Result<(), ReplayControlError> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Err(ReplayControlError::EmptyCommand);
    }
    if command.contains(['\r', '\n', '\0']) {
        return Err(ReplayControlError::UnsafeCommand);
    }
    let name = trimmed
        .split_ascii_whitespace()
        .next()
        .ok_or(ReplayControlError::EmptyCommand)?;
    if !ALLOWED_COMMANDS.contains(&name) {
        return Err(ReplayControlError::CommandNotAllowed(name.to_string()));
    }
    Ok(())
}

pub fn probe_vconsole(timeout: Duration) -> Result<VConsoleProbeReport, ReplayControlError> {
    let started = Instant::now();
    let mut stream = connect_vconsole(timeout)?;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ReplayControlError::Clock)?
        .as_nanos();
    let marker = format!("D2H_VCONSOLE_PROBE_{nonce}");
    stream.write_all(&build_cmnd_packet(&format!("echo {marker}"))?)?;

    let marker_result = wait_for_marker(&mut stream, &marker, timeout, started)?;
    Ok(VConsoleProbeReport {
        host: Ipv4Addr::LOCALHOST.to_string(),
        port: VCONSOLE_PORT,
        connected: true,
        marker_seen: true,
        read_chunks: marker_result.read_chunks,
        bytes_received: marker_result.bytes_received,
        elapsed_milliseconds: started.elapsed().as_millis(),
    })
}

pub fn execute_vconsole_commands(
    commands: &[String],
    timeout: Duration,
) -> Result<VConsoleExecutionReport, ReplayControlError> {
    for command in commands {
        validate_command(command)?;
    }

    let started = Instant::now();
    let mut stream = connect_vconsole(timeout)?;
    let initial_bytes_drained = drain_initial_stream(&mut stream, Duration::from_secs(2))?;
    for command in commands {
        stream.write_all(&build_cmnd_packet(command)?)?;
    }

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ReplayControlError::Clock)?
        .as_nanos();
    let marker = format!("D2H_VCONSOLE_ACK_{nonce}");
    stream.write_all(&build_cmnd_packet(&format!("echo {marker}"))?)?;
    let marker_result = wait_for_marker(&mut stream, &marker, timeout, started)?;

    Ok(VConsoleExecutionReport {
        host: Ipv4Addr::LOCALHOST.to_string(),
        port: VCONSOLE_PORT,
        commands: commands.to_vec(),
        acknowledged: true,
        initial_bytes_drained,
        response_bytes_received: marker_result.bytes_received,
        console_excerpt: extract_console_excerpt(&marker_result.bytes, &marker),
        elapsed_milliseconds: started.elapsed().as_millis(),
    })
}

fn connect_vconsole(timeout: Duration) -> Result<TcpStream, ReplayControlError> {
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, VCONSOLE_PORT);
    let stream = TcpStream::connect_timeout(&address.into(), timeout)?;
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    stream.set_write_timeout(Some(timeout))?;
    Ok(stream)
}

fn drain_initial_stream(
    stream: &mut TcpStream,
    max_duration: Duration,
) -> Result<usize, ReplayControlError> {
    let started = Instant::now();
    let mut drained = 0;
    let mut chunk = vec![0_u8; 16 * 1024];
    while started.elapsed() < max_duration {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => drained += count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(drained)
}

struct MarkerRead {
    read_chunks: usize,
    bytes_received: usize,
    bytes: Vec<u8>,
}

fn wait_for_marker(
    stream: &mut TcpStream,
    marker: &str,
    timeout: Duration,
    started: Instant,
) -> Result<MarkerRead, ReplayControlError> {
    let mut read_chunks = 0;
    let mut bytes_received = 0;
    let mut response = Vec::new();
    let mut overlap = Vec::new();
    let mut chunk = vec![0_u8; 16 * 1024];
    while started.elapsed() < timeout {
        let count = match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error.into()),
        };

        read_chunks += 1;
        bytes_received += count;
        if response.len() < 256 * 1024 {
            let remaining = 256 * 1024 - response.len();
            response.extend_from_slice(&chunk[..count.min(remaining)]);
        }
        overlap.extend_from_slice(&chunk[..count]);
        if overlap
            .windows(marker.len())
            .any(|window| window == marker.as_bytes())
        {
            return Ok(MarkerRead {
                read_chunks,
                bytes_received,
                bytes: response,
            });
        }
        let retained = marker.len().saturating_sub(1);
        if overlap.len() > retained {
            overlap.drain(..overlap.len() - retained);
        }
    }

    Err(ReplayControlError::MarkerTimeout)
}

fn extract_console_excerpt(bytes: &[u8], marker: &str) -> Vec<String> {
    let normalized: String = bytes
        .iter()
        .map(|byte| {
            if byte.is_ascii_graphic() || matches!(byte, b' ' | b'\t') {
                *byte as char
            } else {
                '\n'
            }
        })
        .collect();
    let mut lines = Vec::new();
    for line in normalized.lines() {
        let line = line.trim();
        if line.len() < 4 || line.contains(marker) || lines.iter().any(|seen| seen == line) {
            continue;
        }
        lines.push(line.to_string());
        if lines.len() >= 80 {
            break;
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use d2_highlights_core::{
        AudioPlan, CombatEvent, DirectorSegment, ParserIdentity, ReplayMetadata, StoryBeat,
    };

    #[test]
    fn builds_current_cmnd_packet_fixture() {
        let packet = build_cmnd_packet("echo hi").unwrap();
        assert_eq!(
            packet,
            [
                b'C', b'M', b'N', b'D', 0x00, 0xD4, 0x00, 0x00, 0x00, 0x14, 0x00, 0x00, b'e', b'c',
                b'h', b'o', b' ', b'h', b'i', 0x00,
            ]
        );
    }

    #[test]
    fn rejects_non_allowlisted_or_multiline_commands() {
        assert!(matches!(
            build_cmnd_packet("quit"),
            Err(ReplayControlError::CommandNotAllowed(_))
        ));
        assert!(matches!(
            build_cmnd_packet("echo ok\nquit"),
            Err(ReplayControlError::UnsafeCommand)
        ));
    }

    #[test]
    fn accepts_native_movie_export_commands() {
        assert!(
            build_cmnd_packet("startmovie d2h_native_probe jpg wav jpeg_quality 95 framerate 30")
                .is_ok()
        );
        assert!(build_cmnd_packet("endmovie").is_ok());
        assert!(build_cmnd_packet("dota_spectator_hero_index 9").is_ok());
        assert!(build_cmnd_packet("dota_camera_focus_player 4").is_ok());
        assert!(build_cmnd_packet("dota_camera_allow_freecam 1").is_ok());
        assert!(build_cmnd_packet("dota_camera_get_pos").is_ok());
        assert!(build_cmnd_packet("dota_camera_get_lookatpos").is_ok());
        assert!(build_cmnd_packet("dota_camera_lerp_position 100 200 2").is_ok());
        assert!(build_cmnd_packet("find dota_camera").is_ok());
        assert!(build_cmnd_packet("cvarlist dota_camera").is_ok());
        for command in [
            "sv_cheats 1",
            "dota_spectator_hudhide",
            "dota_spectator_options_enabled 0",
            "dota_hud_hide_mainhud 1",
            "dota_hud_hide_topbar 1",
            "dota_hud_hide_minimap 1",
            "dota_hud_hide_overlaymap 1",
            "dota_show_itempickups 0",
            "r_draw_selected_ring 0",
            "r_drawpanorama 0",
        ] {
            assert!(build_cmnd_packet(command).is_ok(), "{command}");
        }
    }

    #[test]
    fn converts_director_seconds_to_replay_ticks() {
        let director = DirectorDocument {
            schema_version: "1.0".to_string(),
            source_sha256: "abc".to_string(),
            template: "test".to_string(),
            total_duration_seconds: 10.0,
            transition_seconds: 0.35,
            segments: vec![DirectorSegment {
                candidate_id: "hl-001".to_string(),
                source_start_seconds: 10.0,
                source_peak_seconds: 15.0,
                source_peak_tick: 480,
                source_end_seconds: 20.0,
                output_start_seconds: 0.0,
                output_end_seconds: 10.0,
                primary_hero: None,
                score: 100.0,
                narration_hint: "test".to_string(),
                beats: vec![StoryBeat {
                    kind: "impact".to_string(),
                    source_start_seconds: 14.0,
                    source_end_seconds: 16.0,
                    playback_speed: 0.85,
                    camera: CameraPlan {
                        mode: "follow_hero".to_string(),
                        target_hero: Some("npc_dota_hero_axe".to_string()),
                        framing: "combat_medium".to_string(),
                    },
                }],
            }],
            audio: AudioPlan {
                music_role: "test".to_string(),
                bpm_min: 120,
                bpm_max: 140,
                game_audio_duck_db: -2.0,
                music_duck_under_voice_db: -8.0,
                cues: Vec::new(),
            },
        };
        let timeline = TimelineDocument {
            schema_version: "1.0".to_string(),
            source_sha256: "abc".to_string(),
            parser: ParserIdentity {
                name: "test".to_string(),
                version: "1".to_string(),
            },
            replay: ReplayMetadata {
                playback_ticks: 3_000,
                playback_time_seconds: 100.0,
                game_build: 1,
                match_id: None,
                game_mode: None,
                game_winner: None,
                players: Vec::new(),
            },
            events: vec![CombatEvent {
                tick: 480,
                time_seconds: Some(15.0),
                event_type: "fixture".to_string(),
                attacker: Some("npc_dota_hero_axe".to_string()),
                target: None,
                inflictor: None,
                damage_source: None,
                value: None,
                health: None,
                attacker_team: Some(2),
                target_team: None,
                location_x: Some(123.5),
                location_y: Some(-456.25),
                attacker_is_hero: Some(true),
                target_is_hero: None,
                long_range_kill: None,
                will_reincarnate: None,
                assist_players: Vec::new(),
            }],
            tree_orders: Vec::new(),
            temporary_trees: Vec::new(),
            salutes: Vec::new(),
        };

        let plan = build_replay_control_plan(&director, &timeline).unwrap();

        assert_eq!(plan.segments[0].source_start_tick, 330);
        assert_eq!(plan.segments[0].source_peak_tick, 480);
        assert_eq!(plan.segments[0].source_end_tick, 630);
        assert_eq!(plan.segments[0].beats[0].source_start_tick, 450);
        assert_eq!(plan.tick_mapping, "candidate_anchor_tick_relative");
        assert_eq!(plan.target.host, "127.0.0.1");
        assert_eq!(
            plan.segments[0].beats[0].camera_command.as_deref(),
            Some("dota_camera_set_lookatpos 123.500 -456.250")
        );
    }

    #[test]
    fn rejects_mismatched_source_documents() {
        let director = DirectorDocument {
            schema_version: "1.0".to_string(),
            source_sha256: "left".to_string(),
            template: "test".to_string(),
            total_duration_seconds: 0.0,
            transition_seconds: 0.0,
            segments: Vec::new(),
            audio: AudioPlan {
                music_role: "test".to_string(),
                bpm_min: 120,
                bpm_max: 140,
                game_audio_duck_db: -2.0,
                music_duck_under_voice_db: -8.0,
                cues: Vec::new(),
            },
        };
        let timeline = TimelineDocument {
            schema_version: "1.0".to_string(),
            source_sha256: "right".to_string(),
            parser: ParserIdentity {
                name: "test".to_string(),
                version: "1".to_string(),
            },
            replay: ReplayMetadata {
                playback_ticks: 30,
                playback_time_seconds: 1.0,
                game_build: 1,
                match_id: None,
                game_mode: None,
                game_winner: None,
                players: Vec::new(),
            },
            events: Vec::new(),
            tree_orders: Vec::new(),
            temporary_trees: Vec::new(),
            salutes: Vec::new(),
        };

        assert!(matches!(
            build_replay_control_plan(&director, &timeline),
            Err(ReplayControlError::SourceMismatch)
        ));
    }
}
