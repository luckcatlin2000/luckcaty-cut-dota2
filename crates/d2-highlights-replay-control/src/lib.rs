use d2_highlights_core::{CameraPlan, DirectorDocument, TimelineDocument};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub const CONTROL_SCHEMA_VERSION: &str = "1.0";
pub const VCONSOLE_PORT: u16 = 29_000;
const VCONSOLE_HEADER_SIZE: usize = 12;
const VCONSOLE_COMMAND_VERSION: u16 = 0x00D4;
// The engine's initialization stream includes large opaque CVRB frames.
// Packet lengths are 32-bit; keep a separate defensive allocation bound.
const MAX_VCONSOLE_PACKET_BYTES: usize = 16 * 1024 * 1024;
// With no arguments this command prints usage and the current playback tick,
// then returns without seeking. demo_info prints file metadata instead.
pub const DEMO_STATUS_COMMAND: &str = "demo_goto";
const DEMO_STATUS_USAGE: &str = "syntax: demo_goto <tick> [relative] [pause]";
const VCONSOLE_PRINT_TEXT_OFFSET: usize = VCONSOLE_HEADER_SIZE + 28;
const MAX_RESPONSE_TEXT_BYTES: usize = 256 * 1024;

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
    #[error("VConsole command contains a control character, separator, quote or comment")]
    UnsafeCommand,
    #[error("VConsole command is not allowlisted: {0}")]
    CommandNotAllowed(String),
    #[error("VConsole command is too long: {0} bytes")]
    CommandTooLong(usize),
    #[error("VConsole probe timed out before the echo marker was observed")]
    MarkerTimeout,
    #[error("VConsole session lost synchronization; commands will not be resent")]
    SessionUnavailable,
    #[error("VConsole reported a command failure: {0}")]
    CommandFailed(String),
    #[error("VConsole response format is invalid: {0}")]
    InvalidResponse(&'static str),
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
    packet.extend_from_slice(&u32::from(total_length).to_be_bytes());
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
    // All production commands use typed numeric or ASCII alias arguments. No
    // current caller needs quoting, comments or command chaining. Reject them
    // before encoding; checking only the first word is not a command boundary.
    if command.chars().any(char::is_control)
        || command.contains([';', '"', '\''])
        || command.contains("//")
    {
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
    VConsoleSession::connect(timeout)?.probe(timeout)
}

pub fn execute_vconsole_commands(
    commands: &[String],
    timeout: Duration,
) -> Result<VConsoleExecutionReport, ReplayControlError> {
    for command in commands {
        validate_command(command)?;
    }
    VConsoleSession::connect(timeout)?.execute(commands, timeout)
}

/// One connection owned by one offline client. Keep this alive until that
/// client exits, avoiding repeated engine connection teardown while recording.
/// A failed transaction must never be silently replayed.
pub struct VConsoleSession {
    stream: TcpStream,
    decoder: ConsoleDecoder,
    probe_marker: Option<String>,
    failed: bool,
}

impl VConsoleSession {
    pub fn connect(timeout: Duration) -> Result<Self, ReplayControlError> {
        Ok(Self::from_stream(connect_vconsole(timeout)?))
    }

    fn from_stream(stream: TcpStream) -> Self {
        Self {
            stream,
            decoder: ConsoleDecoder::default(),
            probe_marker: None,
            failed: false,
        }
    }

    fn ensure_usable(&self) -> Result<(), ReplayControlError> {
        if self.failed {
            Err(ReplayControlError::SessionUnavailable)
        } else {
            Ok(())
        }
    }

    /// A short handshake wait may expire while the game initializes. Retry the
    /// wait on this socket, with the same marker and partial frame buffer.
    pub fn probe(&mut self, timeout: Duration) -> Result<VConsoleProbeReport, ReplayControlError> {
        self.ensure_usable()?;
        let started = Instant::now();
        let result = (|| {
            if self.probe_marker.is_none() {
                let nonce = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| ReplayControlError::Clock)?
                    .as_nanos();
                let marker = format!("D2H_VCONSOLE_PROBE_{nonce}");
                self.stream
                    .write_all(&build_cmnd_packet(&format!("echo {marker}"))?)?;
                self.probe_marker = Some(marker);
            }
            let marker = self.probe_marker.as_deref().expect("probe marker was sent");
            let read = wait_for_marker(
                &mut self.stream,
                marker,
                None,
                timeout,
                started,
                &mut self.decoder,
            )?;
            Ok(VConsoleProbeReport {
                host: Ipv4Addr::LOCALHOST.to_string(),
                port: VCONSOLE_PORT,
                connected: true,
                marker_seen: true,
                read_chunks: read.read_chunks,
                bytes_received: read.bytes_received,
                elapsed_milliseconds: started.elapsed().as_millis(),
            })
        })();
        if result
            .as_ref()
            .is_err_and(|e| !matches!(e, ReplayControlError::MarkerTimeout))
        {
            self.failed = true;
        }
        result
    }

    pub fn execute(
        &mut self,
        commands: &[String],
        timeout: Duration,
    ) -> Result<VConsoleExecutionReport, ReplayControlError> {
        self.ensure_usable()?;
        for command in commands {
            validate_command(command)?;
        }
        let result = self.execute_inner(commands, timeout);
        if result
            .as_ref()
            .is_err_and(|e| !matches!(e, ReplayControlError::CommandFailed(_)))
        {
            self.failed = true;
        }
        result
    }

    fn execute_inner(
        &mut self,
        commands: &[String],
        timeout: Duration,
    ) -> Result<VConsoleExecutionReport, ReplayControlError> {
        let started = Instant::now();
        self.stream.set_write_timeout(Some(timeout))?;
        let initial_bytes_drained =
            drain_initial_stream(&mut self.stream, Duration::from_secs(2), &mut self.decoder)?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ReplayControlError::Clock)?
            .as_nanos();
        let marker = format!("D2H_VCONSOLE_ACK_{nonce}");
        let start_marker = format!("D2H_VCONSOLE_BEGIN_{nonce}");
        self.stream
            .write_all(&build_cmnd_packet(&format!("echo {start_marker}"))?)?;
        for command in commands {
            self.stream.write_all(&build_cmnd_packet(command)?)?;
        }
        self.stream
            .write_all(&build_cmnd_packet(&format!("echo {marker}"))?)?;
        let marker_result = wait_for_marker(
            &mut self.stream,
            &marker,
            Some(&start_marker),
            timeout,
            started,
            &mut self.decoder,
        )?;

        let mut console_excerpt = marker_result.console_lines;
        reject_console_errors(commands, &console_excerpt)?;
        if console_excerpt.len() > 80 {
            console_excerpt.drain(..console_excerpt.len() - 80);
        }
        Ok(VConsoleExecutionReport {
            host: Ipv4Addr::LOCALHOST.to_string(),
            port: VCONSOLE_PORT,
            commands: commands.to_vec(),
            acknowledged: true,
            initial_bytes_drained,
            response_bytes_received: marker_result.bytes_received,
            console_excerpt,
            elapsed_milliseconds: started.elapsed().as_millis(),
        })
    }

    /// Drain unsolicited console traffic during long capture waits without
    /// blocking frame collection or retaining unrelated console contents.
    pub fn drain_pending(&mut self) -> Result<(), ReplayControlError> {
        self.ensure_usable()?;
        let result = (|| {
            self.stream.set_nonblocking(true)?;
            let read: Result<(), ReplayControlError> = (|| {
                let started = Instant::now();
                let mut bytes = [0_u8; 16 * 1024];
                while started.elapsed() < Duration::from_millis(5) {
                    match self.stream.read(&mut bytes) {
                        Ok(0) => {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "VConsole peer closed the session",
                            )
                            .into());
                        }
                        Ok(n) => {
                            self.decoder.push(&bytes[..n])?;
                        }
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            break;
                        }
                        Err(e) => return Err(e.into()),
                    }
                }
                Ok(())
            })();
            // Restore blocking reads even if decoding failed.
            let reset = self.stream.set_nonblocking(false);
            read?;
            reset?;
            Ok(())
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

fn reject_console_errors(commands: &[String], lines: &[String]) -> Result<(), ReplayControlError> {
    let status_query = commands.len() == 1 && commands[0].trim() == DEMO_STATUS_COMMAND;
    let error = lines.iter().find(|line| {
        let line = line.trim().to_ascii_lowercase();
        line.starts_with("unknown command")
            || line.starts_with("error:")
            || line.starts_with("error -")
            || (line.starts_with("syntax:") && !(status_query && line == DEMO_STATUS_USAGE))
            || line.contains("couldn't open demo file")
            || line.contains("failed to read demo header")
            || line.contains("can't playdemo")
    });
    if let Some(error) = error {
        Err(ReplayControlError::CommandFailed(error.clone()))
    } else {
        Ok(())
    }
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
    decoder: &mut ConsoleDecoder,
) -> Result<usize, ReplayControlError> {
    let started = Instant::now();
    let mut drained = 0;
    let mut chunk = vec![0_u8; 16 * 1024];
    while started.elapsed() < max_duration {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => {
                drained += count;
                // Keep any partial packet for the subsequent command read.
                decoder.push(&chunk[..count])?;
            }
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
    console_lines: Vec<String>,
}

// PRNT carries 28 bytes of metadata after the common 12-byte header. Treating
// the whole binary packet as text leaks header bytes into tick/camera lines.
// Buffer TCP fragments and only expose complete PRNT message bodies.
#[derive(Default)]
struct ConsoleDecoder {
    pending: Vec<u8>,
}

impl ConsoleDecoder {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, ReplayControlError> {
        self.pending.extend_from_slice(bytes);
        let mut consumed = 0;
        let mut messages = Vec::new();
        while self.pending.len() - consumed >= VCONSOLE_HEADER_SIZE {
            let packet = &self.pending[consumed..];
            // Header: type[4], version:u16, total length:u32, handle:u16.
            // Using only bytes 8..10 works for small PRNT but truncates CVRB.
            let length = u32::from_be_bytes([packet[6], packet[7], packet[8], packet[9]]) as usize;
            if length < VCONSOLE_HEADER_SIZE {
                return Err(ReplayControlError::InvalidResponse("invalid packet length"));
            }
            if length > MAX_VCONSOLE_PACKET_BYTES {
                return Err(ReplayControlError::InvalidResponse(
                    "packet length exceeds 16 MiB limit",
                ));
            }
            if packet.len() < length {
                break;
            }
            if &packet[..4] == b"PRNT" {
                if length <= VCONSOLE_PRINT_TEXT_OFFSET {
                    return Err(ReplayControlError::InvalidResponse("short PRNT packet"));
                }
                let body = &packet[VCONSOLE_PRINT_TEXT_OFFSET..length];
                let end = body.iter().position(|byte| *byte == 0).ok_or(
                    ReplayControlError::InvalidResponse("unterminated PRNT message"),
                )?;
                messages.push(String::from_utf8_lossy(&body[..end]).into_owned());
            }
            consumed += length;
        }
        self.pending.drain(..consumed);
        Ok(messages)
    }
}

fn wait_for_marker(
    stream: &mut TcpStream,
    marker: &str,
    start_marker: Option<&str>,
    timeout: Duration,
    started: Instant,
    decoder: &mut ConsoleDecoder,
) -> Result<MarkerRead, ReplayControlError> {
    let mut read_chunks = 0;
    let mut bytes_received = 0;
    let mut console_lines = Vec::new();
    let mut text_bytes = 0;
    let mut collecting = start_marker.is_none();
    let mut chunk = vec![0_u8; 16 * 1024];
    while started.elapsed() < timeout {
        let count = match stream.read(&mut chunk) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "VConsole peer closed the session before acknowledgement",
                )
                .into());
            }
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
        for message in decoder.push(&chunk[..count])? {
            // Only a complete echoed PRNT message is an acknowledgement.
            // Strings in headers, metadata, or other packet types do not count.
            if Some(message.trim()) == start_marker {
                collecting = true;
                console_lines.clear();
                text_bytes = 0;
                continue;
            }
            if !collecting {
                continue;
            }
            if message.trim() == marker {
                return Ok(MarkerRead {
                    read_chunks,
                    bytes_received,
                    console_lines,
                });
            }
            if start_marker.is_none() {
                // The connection probe needs only its echo, not historical logs.
                continue;
            }
            text_bytes += message.len();
            if text_bytes > MAX_RESPONSE_TEXT_BYTES {
                return Err(ReplayControlError::InvalidResponse(
                    "command response exceeds limit",
                ));
            }
            console_lines.extend(
                message
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned),
            );
        }
    }

    Err(ReplayControlError::MarkerTimeout)
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
        for command in [
            "playdemo replays/a; quit",
            "echo ok;quit",
            "echo //comment",
            "echo \"quoted\"",
            "echo ok\tquit",
        ] {
            assert!(
                matches!(
                    build_cmnd_packet(command),
                    Err(ReplayControlError::UnsafeCommand)
                ),
                "{command}"
            );
        }
    }

    #[test]
    fn acknowledgement_does_not_hide_native_command_errors() {
        assert!(
            reject_console_errors(&[], &["Unknown command: dota_missing".to_string()]).is_err()
        );
        assert!(
            reject_console_errors(
                &[],
                &["CDemoPlayer::StartPlayback: couldn't open demo file 'a'".to_string()]
            )
            .is_err()
        );
        assert!(
            reject_console_errors(
                &[],
                &[
                    "Currently playing 100 of 3000 ticks. Minutes:0.05 File:replays/d2h_safe"
                        .to_string()
                ]
            )
            .is_ok()
        );
    }

    #[test]
    fn status_query_usage_is_allowed_without_ignoring_real_failures() {
        let usage = vec!["Syntax: demo_goto <tick> [relative] [pause]".to_string()];
        assert!(reject_console_errors(&[DEMO_STATUS_COMMAND.to_string()], &usage).is_ok());
        for commands in [
            vec!["demo_goto 500 absolute pause".to_string()],
            vec!["demo_info".to_string()],
            vec!["demo_goto".to_string(), "demo_pause".to_string()],
        ] {
            assert!(reject_console_errors(&commands, &usage).is_err());
        }
        for line in [
            "Error - Not currently playing back a demo.",
            "Unknown command: demo_goto",
            "Syntax: unexpected query",
        ] {
            assert!(
                reject_console_errors(&[DEMO_STATUS_COMMAND.to_string()], &[line.to_string()])
                    .is_err()
            );
        }
    }

    fn console_packet(kind: &[u8; 4], text: &str) -> Vec<u8> {
        let length = VCONSOLE_PRINT_TEXT_OFFSET + text.len() + 1;
        let mut packet = Vec::new();
        packet.extend_from_slice(kind);
        packet.extend_from_slice(&VCONSOLE_COMMAND_VERSION.to_be_bytes());
        packet.extend_from_slice(&(length as u32).to_be_bytes());
        packet.extend_from_slice(&0_u16.to_be_bytes());
        packet.extend_from_slice(&[b'M'; 28]);
        packet.extend_from_slice(text.as_bytes());
        packet.push(0);
        packet
    }

    #[test]
    fn print_decoder_reassembles_fragments_and_excludes_binary_metadata() {
        let text = "  Currently playing 9000 of 30000 ticks. Minutes:5.00 File:replays/fixture";
        let packet = console_packet(b"PRNT", text);
        for split in 0..=packet.len() {
            let mut decoder = ConsoleDecoder::default();
            let mut messages = decoder.push(&packet[..split]).unwrap();
            messages.extend(decoder.push(&packet[split..]).unwrap());
            assert_eq!(messages, vec![text], "split {split}");
            assert!(decoder.pending.is_empty());
        }
        let mut wire = console_packet(b"CVAR", text);
        wire.extend(console_packet(b"PRNT", "中文反馈\n第二行"));
        wire.extend(console_packet(b"PRNT", "Camera look-at position: 1 2 3"));
        assert_eq!(
            ConsoleDecoder::default().push(&wire).unwrap(),
            vec!["中文反馈\n第二行", "Camera look-at position: 1 2 3",]
        );
    }

    fn large_initialization_frame() -> Vec<u8> {
        // Independent wire fixture: version is 16-bit, total length is 32-bit.
        // A 131084-byte CVRB frame has a low length word of just 12. Reading
        // only that word incorrectly treats its body as the next packet.
        let mut frame = vec![b'C', b'V', b'R', b'B', 0, 0xd4, 0, 2, 0, 12, 0, 0];
        frame.resize(131_084, 0);
        let false_ack = console_packet(b"PRNT", "PROBE_FIXTURE");
        frame[12..12 + false_ack.len()].copy_from_slice(&false_ack);
        frame
    }

    #[test]
    fn large_initialization_packet_keeps_following_print_packets_aligned() {
        let mut wire = large_initialization_frame();
        let mut decoder = ConsoleDecoder::default();
        // Embedded PRNT bytes are compressed/opaque blob data, never an ACK.
        assert!(decoder.push(&wire[..65_536]).unwrap().is_empty());
        assert!(decoder.push(&wire[65_536..]).unwrap().is_empty());
        assert!(decoder.pending.is_empty());
        wire.extend(console_packet(b"PRNT", "PROBE_FIXTURE"));
        let mut decoder = ConsoleDecoder::default();
        let mut messages = Vec::new();
        for fragment in wire.chunks(4093) {
            messages.extend(decoder.push(fragment).unwrap());
        }
        assert_eq!(messages, vec!["PROBE_FIXTURE"]);
        assert!(decoder.pending.is_empty());
    }

    #[test]
    fn probe_marker_survives_large_initialization_over_tcp() {
        let mut wire = large_initialization_frame();
        wire.extend(console_packet(b"PRNT", "PROBE_FIXTURE"));
        let (mut socket, server) = console_fixture(wire);
        socket
            .set_read_timeout(Some(Duration::from_millis(250)))
            .unwrap();
        let result = wait_for_marker(
            &mut socket,
            "PROBE_FIXTURE",
            None,
            Duration::from_secs(5),
            Instant::now(),
            &mut ConsoleDecoder::default(),
        );
        // Do not close the client while the fixture server is still writing.
        server.join().unwrap();
        assert!(result.is_ok(), "{:?}", result.err());
    }

    #[test]
    fn malformed_print_packets_never_become_feedback() {
        let mut short = console_packet(b"PRNT", "ignored");
        short[6..10].copy_from_slice(&11_u32.to_be_bytes());
        assert!(ConsoleDecoder::default().push(&short).is_err());
        short[6..10].copy_from_slice(&40_u32.to_be_bytes());
        assert!(ConsoleDecoder::default().push(&short).is_err());
        short[6..10].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(ConsoleDecoder::default().push(&short[..12]).is_err());
        let mut unterminated = console_packet(b"PRNT", "ignored");
        *unterminated.last_mut().unwrap() = b'x';
        assert!(ConsoleDecoder::default().push(&unterminated).is_err());
        let truncated = console_packet(b"PRNT", "Currently playing 9000 of 30000 ticks.");
        assert!(
            ConsoleDecoder::default()
                .push(&truncated[..truncated.len() - 1])
                .unwrap()
                .is_empty()
        );
    }

    fn console_fixture(wire: Vec<u8>) -> (TcpStream, std::thread::JoinHandle<()>) {
        // A private ephemeral test socket: never the game's fixed control port.
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            for fragment in wire.chunks(3) {
                socket.write_all(fragment).unwrap();
            }
        });
        (TcpStream::connect(address).unwrap(), server)
    }

    fn read_fixture_command(socket: &mut TcpStream) -> String {
        let mut header = [0_u8; 12];
        socket.read_exact(&mut header).unwrap();
        assert_eq!(&header[..4], b"CMND");
        let len = u32::from_be_bytes(header[6..10].try_into().unwrap()) as usize;
        let mut body = vec![0; len - 12];
        socket.read_exact(&mut body).unwrap();
        assert_eq!(body.pop(), Some(0));
        String::from_utf8(body).unwrap()
    }

    #[test]
    fn session_reuses_handshake_socket_after_timeout_and_across_commands() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (release, resume) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            drop(listener); // Any attempted reconnect must fail this fixture.
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let probe = read_fixture_command(&mut socket);
            assert!(probe.starts_with("echo D2H_VCONSOLE_PROBE_"));
            let packet = console_packet(b"PRNT", probe.strip_prefix("echo ").unwrap());
            socket.write_all(&packet[..18]).unwrap();
            resume.recv_timeout(Duration::from_secs(3)).unwrap();
            socket.write_all(&packet[18..]).unwrap();
            let mut actions = Vec::new();
            for _ in 0..6 {
                let command = read_fixture_command(&mut socket);
                if let Some(text) = command.strip_prefix("echo ") {
                    socket.write_all(&console_packet(b"PRNT", text)).unwrap();
                } else {
                    actions.push(command);
                    socket
                        .write_all(&console_packet(
                            b"PRNT",
                            "Currently playing 9000 of 30000 ticks.",
                        ))
                        .unwrap();
                }
            }
            actions
        });
        let socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(10)))
            .unwrap();
        let mut session = VConsoleSession::from_stream(socket);
        assert!(matches!(
            session.probe(Duration::from_millis(40)),
            Err(ReplayControlError::MarkerTimeout)
        ));
        release.send(()).unwrap();
        assert!(session.probe(Duration::from_secs(1)).unwrap().marker_seen);
        for command in ["demo_goto", "demo_pause"] {
            let report = session
                .execute(&[command.into()], Duration::from_secs(1))
                .unwrap();
            assert!(report.acknowledged);
            assert_eq!(
                report.console_excerpt,
                ["Currently playing 9000 of 30000 ticks."]
            );
        }
        assert_eq!(server.join().unwrap(), ["demo_goto", "demo_pause"]);
    }

    #[test]
    fn session_probe_stops_when_peer_closes_instead_of_retrying_handshake() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let probe = read_fixture_command(&mut socket);
            assert!(probe.starts_with("echo D2H_VCONSOLE_PROBE_"));
            // Closing without a reply is a broken session, not slow startup.
        });
        let socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(10)))
            .unwrap();
        let mut session = VConsoleSession::from_stream(socket);
        assert!(matches!(
            session.probe(Duration::from_secs(1)),
            Err(ReplayControlError::Io(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof
        ));
        assert!(matches!(
            session.probe(Duration::from_secs(1)),
            Err(ReplayControlError::SessionUnavailable)
        ));
        server.join().unwrap();
    }

    #[test]
    fn session_never_resends_a_command_after_missing_acknowledgement() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            drop(listener);
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let begin = read_fixture_command(&mut socket);
            socket
                .write_all(&console_packet(
                    b"PRNT",
                    begin.strip_prefix("echo ").unwrap(),
                ))
                .unwrap();
            let action = read_fixture_command(&mut socket);
            let _ack = read_fixture_command(&mut socket); // Deliberately never acknowledged.
            let mut extra = [0_u8; 1];
            assert_eq!(
                socket.read(&mut extra).unwrap(),
                0,
                "No retry or cleanup command may be sent on an ambiguous transaction"
            );
            action
        });
        let socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(10)))
            .unwrap();
        let mut session = VConsoleSession::from_stream(socket);
        let commands = ["startmovie fixture jpg wav framerate 60".into()];
        assert!(matches!(
            session.execute(&commands, Duration::from_millis(80)),
            Err(ReplayControlError::MarkerTimeout)
        ));
        assert!(matches!(
            session.execute(&commands, Duration::from_secs(1)),
            Err(ReplayControlError::SessionUnavailable)
        ));
        drop(session);
        assert_eq!(server.join().unwrap(), commands[0]);
    }

    #[test]
    fn session_idle_drain_preserves_partial_frames_and_discards_stale_feedback() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (sent, initial) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let idle = console_packet(b"PRNT", "Currently playing 100 of 30000 ticks.");
            socket.write_all(&idle[..10]).unwrap();
            sent.send(()).unwrap();
            resume.recv_timeout(Duration::from_secs(3)).unwrap();
            socket.write_all(&idle[10..]).unwrap();
            for _ in 0..3 {
                let command = read_fixture_command(&mut socket);
                let text = command
                    .strip_prefix("echo ")
                    .unwrap_or("Currently playing 9000 of 30000 ticks.");
                socket.write_all(&console_packet(b"PRNT", text)).unwrap();
            }
        });
        let socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(10)))
            .unwrap();
        let mut session = VConsoleSession::from_stream(socket);
        initial.recv_timeout(Duration::from_secs(3)).unwrap();
        session.drain_pending().unwrap();
        assert_eq!(session.decoder.pending.len(), 10);
        release.send(()).unwrap();
        let report = session
            .execute(&["demo_goto".into()], Duration::from_secs(1))
            .unwrap();
        assert_eq!(
            report.console_excerpt,
            ["Currently playing 9000 of 30000 ticks."]
        );
        server.join().unwrap();
        assert!(session.drain_pending().is_err());
        assert!(matches!(
            session.execute(&["demo_pause".into()], Duration::from_secs(1)),
            Err(ReplayControlError::SessionUnavailable)
        ));
    }

    #[test]
    fn response_window_excludes_stale_ticks_and_waits_for_print_ack() {
        let mut wire = console_packet(b"PRNT", "Currently playing 100 of 30000 ticks.");
        wire.extend(console_packet(b"CVAR", "ACK_FIXTURE"));
        wire.extend(console_packet(b"PRNT", "BEGIN_FIXTURE"));
        wire.extend(console_packet(
            b"PRNT",
            "Syntax: demo_goto <tick> [relative] [pause]",
        ));
        wire.extend(console_packet(
            b"PRNT",
            "Currently playing 9000 of 30000 ticks.",
        ));
        wire.extend(console_packet(b"PRNT", "ACK_FIXTURE"));
        let (mut socket, server) = console_fixture(wire);
        let result = wait_for_marker(
            &mut socket,
            "ACK_FIXTURE",
            Some("BEGIN_FIXTURE"),
            Duration::from_secs(1),
            Instant::now(),
            &mut ConsoleDecoder::default(),
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(
            result.console_lines,
            vec![
                "Syntax: demo_goto <tick> [relative] [pause]",
                "Currently playing 9000 of 30000 ticks.",
            ]
        );
    }

    #[test]
    fn acknowledgement_in_non_print_data_or_echo_text_is_not_success() {
        let mut wire = console_packet(b"PRNT", "BEGIN_FIXTURE");
        wire.extend(console_packet(b"CVAR", "ACK_FIXTURE"));
        wire.extend(console_packet(b"PRNT", "echo ACK_FIXTURE"));
        let (mut socket, server) = console_fixture(wire);
        let result = wait_for_marker(
            &mut socket,
            "ACK_FIXTURE",
            Some("BEGIN_FIXTURE"),
            Duration::from_secs(1),
            Instant::now(),
            &mut ConsoleDecoder::default(),
        );
        server.join().unwrap();
        // This fixture closes after the fake echoes. Neither is an ACK, so
        // the observed EOF must remain a failure rather than a success.
        assert!(matches!(
            result,
            Err(ReplayControlError::Io(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof
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
                context: Default::default(),
            }],
            tree_orders: Vec::new(),
            temporary_trees: Vec::new(),
            salutes: Vec::new(),
            state_snapshots: Vec::new(),
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
            state_snapshots: Vec::new(),
        };

        assert!(matches!(
            build_replay_control_plan(&director, &timeline),
            Err(ReplayControlError::SourceMismatch)
        ));
    }
}
