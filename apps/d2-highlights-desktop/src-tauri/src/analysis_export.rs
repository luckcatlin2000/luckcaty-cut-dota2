use d2_highlights_core::{
    CombatEvent, HighlightCandidate, HighlightDocument, JobManifest, ReplayPlayer, TimelineDocument,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const STORY_EXPORT_SCHEMA_VERSION: &str = "dota2_story_export/1.0.0";
const STORY_EXPORT_CONTRACT: &str = "dota2_highlight_json_v1_20260812";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnalysisPackageExportRequest {
    pub job_id: String,
    pub protagonist_hero: String,
    pub protagonist_label: String,
    pub destination_directory: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnalysisPackageExportResult {
    pub package_path: String,
    pub evidence_path: String,
    pub report_path: String,
    pub event_count: usize,
    pub incident_count: usize,
}

#[derive(Default)]
struct PlayerStats {
    kills: usize,
    deaths: usize,
    assists: usize,
}

struct StoryEventRecord {
    value: Value,
    demo_time_seconds: f64,
    tick: u32,
    participants: BTreeSet<String>,
}

struct AnalysisPackage {
    evidence: Value,
    report: String,
    match_id: String,
    event_count: usize,
    incident_count: usize,
}

pub(crate) fn export_analysis_package(
    jobs_root: &Path,
    request: AnalysisPackageExportRequest,
) -> Result<AnalysisPackageExportResult, String> {
    let job_dir = jobs_root.join(&request.job_id);
    let manifest: JobManifest = read_json(&job_dir.join("manifest.json"))?;
    let timeline: TimelineDocument =
        read_json(&job_dir.join("timeline").join("combat-events.json"))?;
    let highlights: HighlightDocument =
        read_json(&job_dir.join("timeline").join("highlights.json"))?;

    if manifest.source.sha256 != timeline.source_sha256
        || manifest.source.sha256 != highlights.source_sha256
    {
        return Err("任务中的录像哈希与分析结果不一致，请重新分析后再导出。".to_string());
    }

    let destination = PathBuf::from(request.destination_directory.trim());
    if !destination.is_absolute() || !destination.is_dir() {
        return Err("请选择一个已经存在的完整保存目录。".to_string());
    }

    let protagonist_hero = request.protagonist_hero.trim();
    let protagonist_label = request.protagonist_label.trim();
    let package = build_analysis_package(
        &request.job_id,
        &manifest,
        &timeline,
        &highlights,
        protagonist_hero,
        protagonist_label,
    )?;

    let safe_label = sanitize_file_component(if protagonist_label.is_empty() {
        protagonist_hero
            .strip_prefix("npc_dota_hero_")
            .unwrap_or(protagonist_hero)
    } else {
        protagonist_label
    });
    let base_name = format!("{}_{}_分析包", package.match_id, safe_label);
    let final_dir = unique_directory(&destination, &base_name);
    let temp_dir = destination.join(format!(
        ".{base_name}.tmp-{}-{}",
        std::process::id(),
        unix_seconds()?
    ));

    fs::create_dir(&temp_dir).map_err(|error| format!("无法建立临时分析包：{error}"))?;
    let write_result = (|| {
        write_json(&temp_dir.join("evidence.json"), &package.evidence)?;
        fs::write(temp_dir.join("report.md"), package.report.as_bytes())
            .map_err(|error| format!("无法写入 report.md：{error}"))?;
        fs::rename(&temp_dir, &final_dir)
            .map_err(|error| format!("无法完成分析包导出：{error}"))?;
        Ok::<(), String>(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(error);
    }

    Ok(AnalysisPackageExportResult {
        package_path: final_dir.display().to_string(),
        evidence_path: final_dir.join("evidence.json").display().to_string(),
        report_path: final_dir.join("report.md").display().to_string(),
        event_count: package.event_count,
        incident_count: package.incident_count,
    })
}

fn build_analysis_package(
    job_id: &str,
    manifest: &JobManifest,
    timeline: &TimelineDocument,
    highlights: &HighlightDocument,
    protagonist_hero: &str,
    protagonist_label: &str,
) -> Result<AnalysisPackage, String> {
    if timeline.replay.players.len() != 10 {
        return Err(format!(
            "标准分析包需要完整 10 人阵容，当前只识别到 {} 人。",
            timeline.replay.players.len()
        ));
    }

    let mut players = timeline.replay.players.clone();
    players.sort_by_key(|player| player.slot);
    if players.windows(2).any(|pair| pair[0].slot == pair[1].slot) {
        return Err("录像阵容包含重复玩家位，无法生成稳定引用。".to_string());
    }

    let protagonist_index = players
        .iter()
        .position(|player| player.hero_name == protagonist_hero)
        .ok_or_else(|| "所选主角不在当前录像的 10 人阵容中。".to_string())?;
    let protagonist_id = player_id(protagonist_index);
    let hero_to_player: HashMap<String, String> = players
        .iter()
        .enumerate()
        .map(|(index, player)| (player.hero_name.clone(), player_id(index)))
        .collect();
    let slot_to_player: HashMap<u8, String> = players
        .iter()
        .enumerate()
        .map(|(index, player)| (player.slot, player_id(index)))
        .collect();

    let game_start_seconds = game_state_time(timeline, 5).unwrap_or(0.0);
    let game_end_seconds = game_state_time(timeline, 6)
        .or_else(|| game_state_time(timeline, 7))
        .unwrap_or(timeline.replay.playback_time_seconds as f64);
    let duration_seconds = (game_end_seconds - game_start_seconds).max(0.0).round() as u64;

    let mut stats = (0..players.len())
        .map(|_| PlayerStats::default())
        .collect::<Vec<_>>();
    accumulate_player_stats(timeline, &hero_to_player, &slot_to_player, &mut stats);

    let (mut story_events, first_blood_seconds) = build_story_events(
        timeline,
        protagonist_hero,
        &protagonist_id,
        &hero_to_player,
        &slot_to_player,
        game_start_seconds,
    );
    story_events.sort_by(|left, right| {
        left.demo_time_seconds
            .total_cmp(&right.demo_time_seconds)
            .then(left.tick.cmp(&right.tick))
    });
    for (index, event) in story_events.iter_mut().enumerate() {
        event
            .value
            .as_object_mut()
            .expect("story event object")
            .insert("event_id".to_string(), json!(format!("e{:06}", index + 1)));
    }

    let incidents = build_incidents(
        timeline,
        highlights,
        protagonist_hero,
        &protagonist_id,
        &hero_to_player,
        &story_events,
        game_start_seconds,
    );
    let radiant_score = stats
        .iter()
        .zip(players.iter())
        .filter(|(_, player)| player_team(player) == "radiant")
        .map(|(stats, _)| stats.kills)
        .sum::<usize>();
    let dire_score = stats
        .iter()
        .zip(players.iter())
        .filter(|(_, player)| player_team(player) == "dire")
        .map(|(stats, _)| stats.kills)
        .sum::<usize>();
    let warnings = vec![
        "当前 DEM 适配器未可靠提供英雄等级、净资产和最终装备；level/net_worth 的 0 仅为 v1 架构占位，禁止作为比赛事实使用。",
        "当前版本未导出经济快照、关键技能持续窗口、完整团战对象和视野事件；请以 capabilities 为准。",
        "无法从现有 Buyback 事件稳定归属玩家时，buyback 不填写 actor_player_id。",
    ];
    let match_id = timeline
        .replay
        .match_id
        .map(|value| value.to_string())
        .unwrap_or_else(|| job_id.to_string());
    let source_name = Path::new(&manifest.source.path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("replay.dem")
        .to_string();
    let event_values = story_events
        .iter()
        .map(|record| record.value.clone())
        .collect::<Vec<_>>();
    let included_event_types = event_values
        .iter()
        .filter_map(|event| event.get("event_type").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let player_values = players
        .iter()
        .enumerate()
        .map(|(index, player)| player_value(index, player, &stats[index]))
        .collect::<Vec<_>>();
    let source_events_scanned = timeline.events.len()
        + timeline.tree_orders.len()
        + timeline.temporary_trees.len()
        + timeline.salutes.len();

    let mut match_value = json!({
        "match_id": match_id,
        "duration_sec": duration_seconds,
        "winning_team": winner_name(timeline.replay.game_winner),
        "radiant_score": radiant_score,
        "dire_score": dire_score,
        "game_version": timeline.replay.game_build.to_string()
    });
    if let Some(game_mode) = timeline.replay.game_mode {
        match_value
            .as_object_mut()
            .expect("match object")
            .insert("game_mode".to_string(), json!(game_mode.to_string()));
    }
    if let Some(first_blood_seconds) = first_blood_seconds {
        match_value.as_object_mut().expect("match object").insert(
            "first_blood_time_sec".to_string(),
            json!(first_blood_seconds),
        );
    }

    let evidence = json!({
        "schema_version": STORY_EXPORT_SCHEMA_VERSION,
        "export_info": {
            "exporter_name": "luckcaty-cut-dota2",
            "exporter_version": env!("CARGO_PKG_VERSION"),
            "exported_at": format_unix_seconds_utc(unix_seconds()?),
            "source_file_name": source_name,
            "source_file_sha256": manifest.source.sha256,
            "detail_level": "story",
            "privacy_mode": "original",
            "capabilities": {
                "replay_ticks": true,
                "positions": false,
                "teamfight_detection": false,
                "ability_windows": false,
                "objective_events": true,
                "economy_snapshots": false,
                "item_timings": false,
                "vision_events": false
            },
            "event_filter": {
                "included_event_types": included_event_types,
                "excluded_event_types": [
                    "continuous_positions",
                    "economy_snapshots",
                    "item_timings",
                    "vision_events"
                ],
                "notes": [
                    "完整保留可归属的英雄击杀；技能事件仅保留主角施法；建筑、肉山、折磨者和买活按当前解析能力导出。"
                ]
            }
        },
        "match": match_value,
        "story_focus": {
            "protagonist_player_id": protagonist_id,
            "perspective": "hero",
            "declared_role": "unknown",
            "supporting_player_ids": [],
            "creator_notes": []
        },
        "players": player_values,
        "events": event_values,
        "key_ability_windows": [],
        "teamfights": [],
        "state_checkpoints": [],
        "incidents": incidents,
        "data_quality": {
            "parse_status": "partial",
            "warnings": warnings,
            "counts": {
                "source_events_scanned": source_events_scanned,
                "story_events_exported": story_events.len(),
                "ability_windows_exported": 0,
                "teamfights_exported": 0,
                "state_checkpoints_exported": 0,
                "incidents_exported": incidents.len()
            }
        },
        "extensions": {
            "job_id": job_id,
            "protagonist_hero": protagonist_hero,
            "protagonist_label": protagonist_label,
            "source_contract": STORY_EXPORT_CONTRACT,
            "placeholder_fields": [
                "players[].stats.level",
                "players[].stats.net_worth"
            ],
            "unsupported_fields": [
                "players[].final_items",
                "players[].role",
                "state_checkpoints",
                "key_ability_windows",
                "teamfights"
            ]
        }
    });
    validate_evidence(&evidence)?;
    let report = build_report(
        &evidence,
        highlights,
        protagonist_hero,
        protagonist_label,
        game_start_seconds,
        warnings.as_slice(),
    );

    Ok(AnalysisPackage {
        evidence,
        report,
        match_id,
        event_count: story_events.len(),
        incident_count: incidents.len(),
    })
}

fn validate_evidence(evidence: &Value) -> Result<(), String> {
    if evidence["schema_version"] != STORY_EXPORT_SCHEMA_VERSION {
        return Err("分析包 Schema 版本无效。".to_string());
    }
    let source_name = evidence["export_info"]["source_file_name"]
        .as_str()
        .ok_or_else(|| "分析包缺少源录像文件名。".to_string())?;
    if Path::new(source_name).is_absolute()
        || Path::new(source_name)
            .file_name()
            .and_then(|name| name.to_str())
            != Some(source_name)
    {
        return Err("分析包只能保存源录像文件名，不能包含本机路径。".to_string());
    }

    let players = evidence["players"]
        .as_array()
        .ok_or_else(|| "分析包缺少 10 人阵容。".to_string())?;
    if players.len() != 10 {
        return Err(format!(
            "分析包阵容必须正好 10 人，当前为 {} 人。",
            players.len()
        ));
    }
    let player_ids = players
        .iter()
        .filter_map(|player| player["player_id"].as_str())
        .collect::<BTreeSet<_>>();
    if player_ids.len() != 10 {
        return Err("分析包玩家引用缺失或重复。".to_string());
    }
    let protagonist_id = evidence["story_focus"]["protagonist_player_id"]
        .as_str()
        .ok_or_else(|| "分析包缺少高光主角。".to_string())?;
    if !player_ids.contains(protagonist_id) {
        return Err("分析包主角不在 10 人阵容中。".to_string());
    }
    let radiant_score = evidence["match"]["radiant_score"]
        .as_u64()
        .ok_or_else(|| "分析包缺少天辉总击杀数。".to_string())?;
    let dire_score = evidence["match"]["dire_score"]
        .as_u64()
        .ok_or_else(|| "分析包缺少夜魇总击杀数。".to_string())?;
    for player in players {
        let player_id = player["player_id"].as_str().unwrap_or("unknown");
        let team_kills = match player["team"].as_str() {
            Some("radiant") => radiant_score,
            Some("dire") => dire_score,
            _ => return Err(format!("分析包玩家 {player_id} 的阵营无效。")),
        };
        let kills = player["stats"]["kills"]
            .as_u64()
            .ok_or_else(|| format!("分析包玩家 {player_id} 缺少击杀统计。"))?;
        let assists = player["stats"]["assists"]
            .as_u64()
            .ok_or_else(|| format!("分析包玩家 {player_id} 缺少助攻统计。"))?;
        if kills + assists > team_kills {
            return Err(format!(
                "分析包玩家 {player_id} 的击杀与助攻之和超过己方总击杀数。"
            ));
        }
    }

    let events = evidence["events"]
        .as_array()
        .ok_or_else(|| "分析包事件列表无效。".to_string())?;
    let event_ids = events
        .iter()
        .filter_map(|event| event["event_id"].as_str())
        .collect::<BTreeSet<_>>();
    if event_ids.len() != events.len() {
        return Err("分析包事件编号缺失或重复。".to_string());
    }
    let mut previous_time = f64::NEG_INFINITY;
    for event in events {
        let game_time = event["time"]["game_time_sec"]
            .as_f64()
            .ok_or_else(|| "分析包事件缺少比赛时间。".to_string())?;
        if game_time < previous_time {
            return Err("分析包事件没有按比赛时间排序。".to_string());
        }
        previous_time = game_time;
        for field in ["actor_player_id", "target_player_ids", "assist_player_ids"] {
            let references = if field == "actor_player_id" {
                event[field].as_str().into_iter().collect::<Vec<_>>()
            } else {
                event[field]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
            };
            if references
                .iter()
                .any(|reference| !player_ids.contains(reference))
            {
                return Err(format!("分析包事件包含未知玩家引用：{field}"));
            }
        }
        if event["event_type"].as_str() == Some("hero_kill")
            && let Some(actor_player_id) = event["actor_player_id"].as_str()
            && event["assist_player_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .any(|assist_player_id| assist_player_id == actor_player_id)
        {
            return Err(format!(
                "分析包事件 {} 的击杀者不能同时计为助攻。",
                event["event_id"].as_str().unwrap_or("unknown")
            ));
        }
    }

    let incidents = evidence["incidents"]
        .as_array()
        .ok_or_else(|| "分析包候选列表无效。".to_string())?;
    let incident_ids = incidents
        .iter()
        .filter_map(|incident| incident["incident_id"].as_str())
        .collect::<BTreeSet<_>>();
    if incident_ids.len() != incidents.len() {
        return Err("分析包候选编号缺失或重复。".to_string());
    }
    for incident in incidents {
        let referenced_events = incident["evidence_event_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str);
        if referenced_events
            .clone()
            .any(|event_id| !event_ids.contains(event_id))
        {
            return Err("分析包候选引用了不存在的事实事件。".to_string());
        }
        if incident["participant_player_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|player_id| !player_ids.contains(player_id))
        {
            return Err("分析包候选引用了不存在的玩家。".to_string());
        }
    }

    let counts = &evidence["data_quality"]["counts"];
    if counts["story_events_exported"].as_u64() != Some(events.len() as u64)
        || counts["incidents_exported"].as_u64() != Some(incidents.len() as u64)
    {
        return Err("分析包数量统计与实际内容不一致。".to_string());
    }
    Ok(())
}

fn accumulate_player_stats(
    timeline: &TimelineDocument,
    hero_to_player: &HashMap<String, String>,
    slot_to_player: &HashMap<u8, String>,
    stats: &mut [PlayerStats],
) {
    for event in timeline.events.iter().filter(|event| {
        event.event_type == "DotaCombatlogDeath"
            && event.target_is_hero == Some(true)
            && event.will_reincarnate != Some(true)
    }) {
        if let Some(target_id) = event
            .target
            .as_ref()
            .and_then(|hero| hero_to_player.get(hero))
            && let Some(index) = player_index(target_id)
        {
            stats[index].deaths += 1;
        }
        let actor_id = actor_player_id(event, hero_to_player);
        if let Some(actor_id) = actor_id.as_deref()
            && let Some(index) = player_index(actor_id)
        {
            stats[index].kills += 1;
        }
        for assist in assist_player_ids(event, slot_to_player, actor_id.as_deref()) {
            if let Some(index) = player_index(&assist) {
                stats[index].assists += 1;
            }
        }
    }
}

fn build_story_events(
    timeline: &TimelineDocument,
    protagonist_hero: &str,
    protagonist_id: &str,
    hero_to_player: &HashMap<String, String>,
    slot_to_player: &HashMap<u8, String>,
    game_start_seconds: f64,
) -> (Vec<StoryEventRecord>, Option<f64>) {
    let mut records = Vec::new();
    let mut first_blood_seconds: Option<f64> = None;

    for event in &timeline.events {
        let demo_time = match event.time_seconds {
            Some(value) => value as f64,
            None => continue,
        };
        let time = time_point(event.tick, demo_time, game_start_seconds);
        if event.event_type == "DotaCombatlogDeath" && event.target_is_hero == Some(true) {
            let targets = event
                .target
                .as_ref()
                .and_then(|hero| hero_to_player.get(hero))
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            if targets.is_empty() {
                continue;
            }
            let actor = actor_player_id(event, hero_to_player);
            let event_type = if event.will_reincarnate == Some(true) {
                "custom:hero_reincarnation"
            } else if actor.is_some() {
                "hero_kill"
            } else {
                "custom:hero_death_uncredited"
            };
            let assist_actor = (event_type == "hero_kill")
                .then_some(actor.as_deref())
                .flatten();
            let assists = assist_player_ids(event, slot_to_player, assist_actor);
            if event.will_reincarnate != Some(true) {
                let game_time = demo_time - game_start_seconds;
                first_blood_seconds =
                    Some(first_blood_seconds.map_or(game_time, |current| current.min(game_time)));
            }
            let mut details = Map::new();
            details.insert("source_event_type".to_string(), json!(event.event_type));
            details.insert("killer_entity".to_string(), json!(event.attacker));
            details.insert("damage_source".to_string(), json!(event.damage_source));
            details.insert("long_range_kill".to_string(), json!(event.long_range_kill));
            details.insert(
                "will_reincarnate".to_string(),
                json!(event.will_reincarnate),
            );
            let mut value = base_event(
                event_type,
                time,
                2,
                targets,
                assists,
                vec!["complete_match", "hero_death"],
                details,
            );
            if let Some(actor) = actor {
                value.insert("actor_player_id".to_string(), json!(actor));
            }
            if let Some(team) = event.attacker_team.and_then(team_from_u32) {
                value.insert("actor_team".to_string(), json!(team));
            }
            records.push(record(value, demo_time, event.tick));
            continue;
        }

        if event.event_type == "DotaCombatlogAbility"
            && event.attacker.as_deref() == Some(protagonist_hero)
        {
            let targets = event
                .target
                .as_ref()
                .and_then(|hero| hero_to_player.get(hero))
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            let mut details = Map::new();
            details.insert("source_event_type".to_string(), json!(event.event_type));
            details.insert("target_entity".to_string(), json!(event.target));
            let mut value = base_event(
                "ability_cast",
                time,
                1,
                targets,
                Vec::new(),
                vec!["protagonist_ability"],
                details,
            );
            value.insert("actor_player_id".to_string(), json!(protagonist_id));
            if let Some(inflictor) = &event.inflictor {
                value.insert("ability".to_string(), json!({ "internal_name": inflictor }));
            }
            records.push(record(value, demo_time, event.tick));
            continue;
        }

        if event.event_type == "DotaCombatlogBuyback" {
            let mut details = Map::new();
            details.insert("source_event_type".to_string(), json!(event.event_type));
            records.push(record(
                base_event(
                    "buyback",
                    time,
                    1,
                    Vec::new(),
                    Vec::new(),
                    vec!["buyback"],
                    details,
                ),
                demo_time,
                event.tick,
            ));
            continue;
        }

        if event.event_type == "DotaCombatlogTeamBuildingKill" {
            if let Some(target) = event.target.as_deref() {
                let (event_type, objective_type) = building_event_type(target);
                let mut details = Map::new();
                details.insert("source_event_type".to_string(), json!(event.event_type));
                let mut value = base_event(
                    event_type,
                    time,
                    2,
                    Vec::new(),
                    Vec::new(),
                    vec!["map_objective"],
                    details,
                );
                if let Some(team) = event.attacker_team.and_then(team_from_u32) {
                    value.insert("actor_team".to_string(), json!(team));
                }
                value.insert(
                    "objective".to_string(),
                    objective_value(
                        objective_type,
                        target,
                        event
                            .target_team
                            .and_then(team_from_u32)
                            .unwrap_or("unknown"),
                    ),
                );
                records.push(record(value, demo_time, event.tick));
            }
            continue;
        }

        if event.event_type == "DotaCombatlogDeath" {
            let Some(target) = event.target.as_deref() else {
                continue;
            };
            let event_type = if target == "npc_dota_roshan" {
                Some(("roshan_killed", "roshan"))
            } else if target.contains("tormentor") {
                Some(("tormentor_killed", "tormentor"))
            } else {
                None
            };
            if let Some((event_type, objective_type)) = event_type {
                let mut details = Map::new();
                details.insert("source_event_type".to_string(), json!(event.event_type));
                let mut value = base_event(
                    event_type,
                    time,
                    2,
                    Vec::new(),
                    Vec::new(),
                    vec!["map_objective"],
                    details,
                );
                if let Some(actor) = actor_player_id(event, hero_to_player) {
                    value.insert("actor_player_id".to_string(), json!(actor));
                }
                if let Some(team) = event.attacker_team.and_then(team_from_u32) {
                    value.insert("actor_team".to_string(), json!(team));
                }
                value.insert(
                    "objective".to_string(),
                    objective_value(objective_type, target, "neutral"),
                );
                records.push(record(value, demo_time, event.tick));
            }
        }
    }

    for salute in &timeline.salutes {
        let demo_time = demo_time_for_tick(timeline, salute.tick);
        let targets = salute
            .target_player_id
            .and_then(|slot| u8::try_from(slot).ok())
            .and_then(|slot| slot_to_player.get(&slot))
            .cloned()
            .into_iter()
            .collect::<Vec<_>>();
        let mut details = Map::new();
        details.insert("tip_amount".to_string(), json!(salute.tip_amount));
        details.insert("event_id".to_string(), json!(salute.event_id));
        details.insert(
            "recent_tip_count".to_string(),
            json!(salute.num_recent_tips),
        );
        let mut value = base_event(
            "custom:player_tip",
            time_point(salute.tick, demo_time, game_start_seconds),
            1,
            targets,
            Vec::new(),
            vec!["player_reaction", "tip"],
            details,
        );
        if let Some(actor) = salute
            .source_player_id
            .and_then(|slot| u8::try_from(slot).ok())
            .and_then(|slot| slot_to_player.get(&slot))
        {
            value.insert("actor_player_id".to_string(), json!(actor));
        }
        records.push(record(value, demo_time, salute.tick));
    }

    (records, first_blood_seconds.map(round3))
}

fn build_incidents(
    timeline: &TimelineDocument,
    highlights: &HighlightDocument,
    protagonist_hero: &str,
    protagonist_id: &str,
    hero_to_player: &HashMap<String, String>,
    events: &[StoryEventRecord],
    game_start_seconds: f64,
) -> Vec<Value> {
    highlights
        .candidates
        .iter()
        .filter(|candidate| {
            candidate.primary_hero.as_deref() == Some(protagonist_hero)
                || candidate
                    .participants
                    .iter()
                    .any(|hero| hero == protagonist_hero)
        })
        .enumerate()
        .map(|(index, candidate)| {
            incident_value(
                timeline,
                candidate,
                index,
                protagonist_id,
                hero_to_player,
                events,
                game_start_seconds,
            )
        })
        .collect()
}

fn incident_value(
    timeline: &TimelineDocument,
    candidate: &HighlightCandidate,
    index: usize,
    protagonist_id: &str,
    hero_to_player: &HashMap<String, String>,
    events: &[StoryEventRecord],
    game_start_seconds: f64,
) -> Value {
    let related = events
        .iter()
        .filter(|event| {
            event.demo_time_seconds >= candidate.start_seconds as f64
                && event.demo_time_seconds <= candidate.end_seconds as f64
        })
        .collect::<Vec<_>>();
    let evidence_ids = related
        .iter()
        .filter_map(|event| event.value.get("event_id").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let mut participants = related
        .iter()
        .flat_map(|event| event.participants.iter().cloned())
        .collect::<BTreeSet<_>>();
    participants.insert(protagonist_id.to_string());
    for hero in &candidate.participants {
        if let Some(player_id) = hero_to_player.get(hero) {
            participants.insert(player_id.clone());
        }
    }
    let protagonist_kills = related
        .iter()
        .filter(|event| {
            event.value.get("event_type").and_then(Value::as_str) == Some("hero_kill")
                && event.value.get("actor_player_id").and_then(Value::as_str)
                    == Some(protagonist_id)
        })
        .count();
    let confidence = if candidate.score >= 100.0 {
        0.9
    } else if candidate.score >= 50.0 {
        0.8
    } else {
        0.7
    };
    let start_tick = nearest_tick(timeline, candidate.start_seconds as f64);
    let end_tick = nearest_tick(timeline, candidate.end_seconds as f64);
    let incident_type = match candidate.kind.as_str() {
        "team_fight" | "multikill" | "trade" => "teamfight",
        "buyback_dieback" => "comeback",
        "mechanical_counterplay" => "counterplay",
        "hero_kill_sequence" | "pickoff" => "gank",
        _ => "custom:highlight_candidate",
    };
    let start = time_point(
        start_tick,
        candidate.start_seconds as f64,
        game_start_seconds,
    );
    let end = time_point(end_tick, candidate.end_seconds as f64, game_start_seconds);
    json!({
        "incident_id": format!("inc{:04}", index + 1),
        "incident_type": incident_type,
        "range": { "start": start, "end": end },
        "decision_time": time_point(
            candidate.anchor_tick,
            candidate.peak_seconds as f64,
            game_start_seconds
        ),
        "result_time": end,
        "participant_player_ids": participants,
        "evidence_event_ids": evidence_ids,
        "facts": [{
            "fact_id": format!("fact{:04}", index + 1),
            "statement": format!(
                "该候选时段包含 {} 条可导出事实事件，其中主角击杀 {} 次。",
                related.len(),
                protagonist_kills
            ),
            "derivation": "calculated",
            "source_event_ids": evidence_ids,
            "metrics": {
                "candidate_id": candidate.id,
                "candidate_kind": candidate.kind,
                "candidate_score": candidate.score,
                "protagonist_kills": protagonist_kills
            }
        }],
        "inference": {
            "label": candidate.title,
            "explanation": candidate.reasons.join("；"),
            "source": "rule_engine",
            "confidence": confidence,
            "rule_id": candidate.kind,
            "alternative_causes": []
        },
        "recommended_replay_range": { "start": start, "end": end },
        "tags": [candidate.kind.clone(), "rule_engine_candidate"]
    })
}

fn build_report(
    evidence: &Value,
    highlights: &HighlightDocument,
    protagonist_hero: &str,
    protagonist_label: &str,
    game_start_seconds: f64,
    warnings: &[&str],
) -> String {
    let match_id = evidence["match"]["match_id"].as_str().unwrap_or("unknown");
    let event_count = evidence["events"].as_array().map_or(0, Vec::len);
    let incident_count = evidence["incidents"].as_array().map_or(0, Vec::len);
    let label = if protagonist_label.is_empty() {
        protagonist_hero
    } else {
        protagonist_label
    };
    let mut report = format!(
        "# Dota 2 录像分析报告\n\n- 比赛编号：`{}`\n- 高光主角：{}（`{}`）\n- 事实事件：{}\n- 主角相关候选：{}\n- 数据状态：部分支持\n\n## 十人阵容\n\n| ID | 阵营 | 英雄 | 玩家昵称 | K/D/A |\n|---|---|---|---|---|\n",
        markdown_cell(match_id),
        markdown_cell(label),
        markdown_cell(protagonist_hero),
        event_count,
        incident_count
    );
    if let Some(players) = evidence["players"].as_array() {
        for player in players {
            let stats = &player["stats"];
            report.push_str(&format!(
                "| {} | {} | `{}` | {} | {}/{}/{} |\n",
                markdown_cell(player["player_id"].as_str().unwrap_or("")),
                markdown_cell(player["team"].as_str().unwrap_or("")),
                markdown_cell(player["hero"]["internal_name"].as_str().unwrap_or("")),
                markdown_cell(player["display_name"].as_str().unwrap_or("匿名玩家")),
                stats["kills"].as_u64().unwrap_or(0),
                stats["deaths"].as_u64().unwrap_or(0),
                stats["assists"].as_u64().unwrap_or(0)
            ));
        }
    }
    report.push_str("\n## 主角相关候选\n\n");
    for candidate in highlights.candidates.iter().filter(|candidate| {
        candidate.primary_hero.as_deref() == Some(protagonist_hero)
            || candidate
                .participants
                .iter()
                .any(|hero| hero == protagonist_hero)
    }) {
        report.push_str(&format!(
            "- `{}` {}-{}：{}\n",
            markdown_cell(&candidate.id),
            format_game_time(candidate.start_seconds as f64 - game_start_seconds),
            format_game_time(candidate.end_seconds as f64 - game_start_seconds),
            markdown_cell(&candidate.title)
        ));
    }
    report.push_str("\n## 数据边界\n\n");
    for warning in warnings {
        report.push_str(&format!("- {warning}\n"));
    }
    report.push_str(
        "\n本报告由 `evidence.json` 派生，不包含导演镜头、ComfyUI 或 MiniMax H3 参数。后续项目应读取证据后另行生成 `director_plan.json`。\n",
    );
    report
}

fn player_value(index: usize, player: &ReplayPlayer, stats: &PlayerStats) -> Value {
    let mut value = json!({
        "player_id": player_id(index),
        "roster_index": index,
        "dota_player_slot": player.slot,
        "team": player_team(player),
        "hero": { "internal_name": player.hero_name },
        "role": "unknown",
        "stats": {
            "kills": stats.kills,
            "deaths": stats.deaths,
            "assists": stats.assists,
            "level": 0,
            "net_worth": 0
        },
        "final_items": []
    });
    if let Some(name) = player.player_name.as_deref() {
        value
            .as_object_mut()
            .expect("player object")
            .insert("display_name".to_string(), json!(name));
    }
    value
}

fn base_event(
    event_type: &str,
    time: Value,
    importance: usize,
    targets: Vec<String>,
    assists: Vec<String>,
    tags: Vec<&str>,
    details: Map<String, Value>,
) -> Map<String, Value> {
    let mut value = Map::new();
    value.insert("event_id".to_string(), json!("pending"));
    value.insert("event_type".to_string(), json!(event_type));
    value.insert("time".to_string(), time);
    value.insert("importance".to_string(), json!(importance));
    value.insert("target_player_ids".to_string(), json!(targets));
    value.insert("assist_player_ids".to_string(), json!(assists));
    value.insert("related_event_ids".to_string(), json!([]));
    value.insert("tags".to_string(), json!(tags));
    value.insert("details".to_string(), Value::Object(details));
    value
}

fn record(value: Map<String, Value>, demo_time_seconds: f64, tick: u32) -> StoryEventRecord {
    let participants = value
        .get("actor_player_id")
        .and_then(Value::as_str)
        .into_iter()
        .chain(
            value
                .get("target_player_ids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        )
        .chain(
            value
                .get("assist_player_ids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        )
        .map(ToOwned::to_owned)
        .collect();
    StoryEventRecord {
        value: Value::Object(value),
        demo_time_seconds,
        tick,
        participants,
    }
}

fn actor_player_id(
    event: &CombatEvent,
    hero_to_player: &HashMap<String, String>,
) -> Option<String> {
    event
        .attacker
        .as_ref()
        .and_then(|hero| hero_to_player.get(hero))
        .or_else(|| {
            event
                .damage_source
                .as_ref()
                .and_then(|hero| hero_to_player.get(hero))
        })
        .cloned()
}

fn assist_player_ids(
    event: &CombatEvent,
    slot_to_player: &HashMap<u8, String>,
    actor_player_id: Option<&str>,
) -> Vec<String> {
    event
        .assist_players
        .iter()
        .filter_map(|slot| u8::try_from(*slot).ok())
        .filter_map(|slot| slot_to_player.get(&slot).cloned())
        .filter(|player_id| Some(player_id.as_str()) != actor_player_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn game_state_time(timeline: &TimelineDocument, state: u32) -> Option<f64> {
    timeline
        .events
        .iter()
        .find(|event| event.event_type == "DotaCombatlogGameState" && event.value == Some(state))
        .and_then(|event| event.time_seconds)
        .map(f64::from)
}

fn demo_time_for_tick(timeline: &TimelineDocument, tick: u32) -> f64 {
    timeline
        .events
        .iter()
        .filter_map(|event| {
            event.time_seconds.map(|time| {
                let distance = event.tick.abs_diff(tick);
                (distance, f64::from(time))
            })
        })
        .min_by_key(|(distance, _)| *distance)
        .map_or(f64::from(tick) / 30.0, |(_, time)| time)
}

fn nearest_tick(timeline: &TimelineDocument, demo_time: f64) -> u32 {
    timeline
        .events
        .iter()
        .filter_map(|event| {
            event
                .time_seconds
                .map(|time| ((f64::from(time) - demo_time).abs(), event.tick))
        })
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map_or(0, |(_, tick)| tick)
}

fn time_point(tick: u32, demo_time: f64, game_start_seconds: f64) -> Value {
    json!({
        "game_time_sec": round3(demo_time - game_start_seconds),
        "replay_tick": tick,
        "demo_time_sec": round3(demo_time)
    })
}

fn building_event_type(target: &str) -> (&'static str, &'static str) {
    if target.contains("tower") {
        ("tower_destroyed", "tower")
    } else if target.contains("rax") || target.contains("barracks") {
        ("barracks_destroyed", "barracks")
    } else {
        ("custom:building_destroyed", "other")
    }
}

fn objective_value(objective_type: &str, target: &str, team: &str) -> Value {
    let lane = if target.contains("_top") {
        "top"
    } else if target.contains("_mid") {
        "mid"
    } else if target.contains("_bot") {
        "bottom"
    } else {
        "none"
    };
    let tier = (1_u64..=5).find(|tier| target.contains(&format!("tower{tier}")));
    let mut value = json!({
        "objective_type": objective_type,
        "objective_team": team,
        "lane": lane,
        "description": target
    });
    if let Some(tier) = tier {
        value
            .as_object_mut()
            .expect("objective object")
            .insert("tier".to_string(), json!(tier));
    }
    value
}

fn team_from_u32(team: u32) -> Option<&'static str> {
    match team {
        2 => Some("radiant"),
        3 => Some("dire"),
        _ => None,
    }
}

fn player_team(player: &ReplayPlayer) -> &'static str {
    match player.game_team {
        Some(3) => "dire",
        _ => "radiant",
    }
}

fn winner_name(winner: Option<i32>) -> &'static str {
    match winner {
        Some(2) => "radiant",
        Some(3) => "dire",
        _ => "unknown",
    }
}

fn player_id(index: usize) -> String {
    format!("p{index}")
}

fn player_index(player_id: &str) -> Option<usize> {
    player_id.strip_prefix('p')?.parse().ok()
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("无法解析 {}：{error}", path.display()))
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    fs::write(path, bytes).map_err(|error| format!("无法写入 {}：{error}", path.display()))
}

fn unique_directory(parent: &Path, base_name: &str) -> PathBuf {
    let first = parent.join(base_name);
    if !first.exists() {
        return first;
    }
    for suffix in 2_u32.. {
        let candidate = parent.join(format!("{base_name}_{suffix}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("directory suffix search is unbounded")
}

fn sanitize_file_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_control() || "<>:\"/\\|?*".contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect::<String>()
        .trim_matches([' ', '.'])
        .to_string();
    if sanitized.is_empty() {
        "主角".to_string()
    } else {
        sanitized
    }
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn format_game_time(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

fn markdown_cell(value: &str) -> String {
    value
        .replace('|', "\\|")
        .replace(['\r', '\n'], " ")
        .trim()
        .to_string()
}

fn unix_seconds() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| error.to_string())
}

fn format_unix_seconds_utc(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let shifted = days_since_epoch + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::{
        AnalysisPackageExportRequest, export_analysis_package, format_unix_seconds_utc,
        sanitize_file_component, validate_evidence,
    };
    use d2_highlights_core::{
        CombatEvent, DemSource, DetectorIdentity, HighlightCandidate, HighlightDocument,
        JobManifest, ParserIdentity, ReplayMetadata, ReplayPlayer, StageRecord, StageStatus,
        TimelineDocument,
    };
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn utc_timestamp_is_rfc3339() {
        assert_eq!(format_unix_seconds_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(
            format_unix_seconds_utc(1_786_544_442),
            "2026-08-12T14:20:42Z"
        );
    }

    #[test]
    fn file_component_removes_windows_path_characters() {
        assert_eq!(sanitize_file_component("暗夜:魔王/?"), "暗夜_魔王__");
        assert_eq!(sanitize_file_component("..."), "主角");
    }

    #[test]
    fn export_is_non_overwriting_and_does_not_copy_or_modify_dem() {
        let root = tempdir().expect("create root");
        let jobs_root = root.path().join("jobs");
        let output_root = root.path().join("exports");
        let job_id = "d2h-1111111111111111";
        let job_dir = jobs_root.join(job_id);
        fs::create_dir_all(job_dir.join("timeline")).expect("create job timeline");
        fs::create_dir_all(&output_root).expect("create output root");
        let dem_path = root.path().join("1234567890.dem");
        fs::write(&dem_path, b"read-only replay fixture").expect("write replay fixture");
        let source_hash = "1".repeat(64);
        let players = (0_u8..10)
            .map(|slot| ReplayPlayer {
                slot,
                hero_name: format!("npc_dota_hero_fixture_{slot}"),
                player_name: Some(format!("player {slot}")),
                game_team: Some(if slot < 5 { 2 } else { 3 }),
                is_fake_client: false,
            })
            .collect::<Vec<_>>();
        let mut kill_event = event(
            900,
            40.0,
            "DotaCombatlogDeath",
            Some("npc_dota_hero_fixture_9"),
            Some("npc_dota_hero_fixture_0"),
            None,
        );
        kill_event.assist_players = vec![8, 9];
        let timeline = TimelineDocument {
            schema_version: "1.3".to_string(),
            source_sha256: source_hash.clone(),
            parser: ParserIdentity {
                name: "fixture".to_string(),
                version: "1".to_string(),
            },
            replay: ReplayMetadata {
                playback_ticks: 3_000,
                playback_time_seconds: 100.0,
                game_build: 6_896,
                match_id: Some(1_234_567_890),
                game_mode: Some(22),
                game_winner: Some(3),
                players,
            },
            events: vec![
                event(10, 10.0, "DotaCombatlogGameState", None, None, Some(5)),
                event(
                    600,
                    30.0,
                    "DotaCombatlogAbility",
                    Some("npc_dota_hero_fixture_9"),
                    Some("npc_dota_hero_fixture_0"),
                    None,
                ),
                kill_event,
                event(2_700, 90.0, "DotaCombatlogGameState", None, None, Some(6)),
            ],
            tree_orders: Vec::new(),
            temporary_trees: Vec::new(),
            salutes: Vec::new(),
        };
        let highlights = HighlightDocument {
            schema_version: "1.4".to_string(),
            source_sha256: source_hash.clone(),
            detector: DetectorIdentity {
                name: "fixture".to_string(),
                version: "1".to_string(),
            },
            candidates: vec![HighlightCandidate {
                id: "hk-001".to_string(),
                rank: 1,
                kind: "hero_kill_sequence".to_string(),
                title: "fixture kill".to_string(),
                score: 80.0,
                start_seconds: 30.0,
                peak_seconds: 40.0,
                end_seconds: 45.0,
                hero_deaths: 1,
                anchor_tick: 900,
                primary_hero: Some("npc_dota_hero_fixture_9".to_string()),
                participants: vec![
                    "npc_dota_hero_fixture_0".to_string(),
                    "npc_dota_hero_fixture_9".to_string(),
                ],
                reasons: vec!["fixture evidence".to_string()],
                interaction: None,
                kill_sequence: None,
            }],
        };
        let manifest = JobManifest {
            schema_version: "1.0".to_string(),
            job_id: job_id.to_string(),
            created_unix_seconds: 1,
            source: DemSource {
                path: dem_path.display().to_string(),
                byte_length: 24,
                sha256: source_hash,
                magic: "PBDEMS2".to_string(),
            },
            stages: BTreeMap::<String, StageRecord>::from([(
                "parse".to_string(),
                StageRecord {
                    status: StageStatus::Complete,
                    output_path: None,
                    error: None,
                },
            )]),
        };
        write_json(job_dir.join("manifest.json"), &manifest);
        write_json(job_dir.join("timeline/combat-events.json"), &timeline);
        write_json(job_dir.join("timeline/highlights.json"), &highlights);

        let first = export_analysis_package(
            &jobs_root,
            AnalysisPackageExportRequest {
                job_id: job_id.to_string(),
                protagonist_hero: "npc_dota_hero_fixture_9".to_string(),
                protagonist_label: "测试主角".to_string(),
                destination_directory: output_root.display().to_string(),
            },
        )
        .expect("export first package");
        let second = export_analysis_package(
            &jobs_root,
            AnalysisPackageExportRequest {
                job_id: job_id.to_string(),
                protagonist_hero: "npc_dota_hero_fixture_9".to_string(),
                protagonist_label: "测试主角".to_string(),
                destination_directory: output_root.display().to_string(),
            },
        )
        .expect("export second package");

        assert!(first.package_path.ends_with("1234567890_测试主角_分析包"));
        assert!(
            second
                .package_path
                .ends_with("1234567890_测试主角_分析包_2")
        );
        assert_eq!(fs::read(&dem_path).unwrap(), b"read-only replay fixture");
        assert!(
            !std::path::Path::new(&first.package_path)
                .join("1234567890.dem")
                .exists()
        );
        let evidence: serde_json::Value =
            serde_json::from_slice(&fs::read(&first.evidence_path).unwrap()).unwrap();
        assert_eq!(evidence["schema_version"], "dota2_story_export/1.0.0");
        assert_eq!(evidence["players"].as_array().unwrap().len(), 10);
        assert_eq!(evidence["events"].as_array().unwrap().len(), 2);
        assert_eq!(evidence["incidents"].as_array().unwrap().len(), 1);
        let kill = evidence["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["event_type"] == "hero_kill")
            .expect("hero kill event");
        assert_eq!(kill["actor_player_id"], "p9");
        assert_eq!(kill["assist_player_ids"], serde_json::json!(["p8"]));
        assert_eq!(evidence["players"][8]["stats"]["assists"], 1);
        assert_eq!(evidence["players"][9]["stats"]["kills"], 1);
        assert_eq!(evidence["players"][9]["stats"]["assists"], 0);
        assert_eq!(
            evidence["export_info"]["source_file_name"],
            "1234567890.dem"
        );
        assert!(!evidence.to_string().contains(root.path().to_str().unwrap()));
    }

    #[test]
    fn validation_rejects_self_assist_and_impossible_participation() {
        let mut evidence = valid_evidence_fixture();
        evidence["events"][0]["assist_player_ids"] = serde_json::json!(["p0"]);
        assert!(
            validate_evidence(&evidence)
                .unwrap_err()
                .contains("击杀者不能同时计为助攻")
        );

        let mut evidence = valid_evidence_fixture();
        evidence["players"][0]["stats"]["assists"] = serde_json::json!(1);
        assert!(
            validate_evidence(&evidence)
                .unwrap_err()
                .contains("击杀与助攻之和超过己方总击杀数")
        );
    }

    #[test]
    #[ignore = "requires D2H_REAL_JOBS_ROOT, D2H_REAL_EXPORT_ROOT, and fixture metadata"]
    fn export_real_job_from_environment() {
        let jobs_root = std::env::var_os("D2H_REAL_JOBS_ROOT")
            .map(std::path::PathBuf::from)
            .expect("D2H_REAL_JOBS_ROOT");
        let output_root = std::env::var_os("D2H_REAL_EXPORT_ROOT")
            .map(std::path::PathBuf::from)
            .expect("D2H_REAL_EXPORT_ROOT");
        let job_id = std::env::var("D2H_REAL_JOB_ID").expect("D2H_REAL_JOB_ID");
        let protagonist_hero =
            std::env::var("D2H_REAL_PROTAGONIST_HERO").expect("D2H_REAL_PROTAGONIST_HERO");
        let protagonist_label =
            std::env::var("D2H_REAL_PROTAGONIST_LABEL").expect("D2H_REAL_PROTAGONIST_LABEL");
        let result = export_analysis_package(
            &jobs_root,
            AnalysisPackageExportRequest {
                job_id,
                protagonist_hero,
                protagonist_label,
                destination_directory: output_root.display().to_string(),
            },
        )
        .expect("export real job");
        println!("{}", result.package_path);
    }

    fn event(
        tick: u32,
        time_seconds: f32,
        event_type: &str,
        attacker: Option<&str>,
        target: Option<&str>,
        value: Option<u32>,
    ) -> CombatEvent {
        CombatEvent {
            tick,
            time_seconds: Some(time_seconds),
            event_type: event_type.to_string(),
            attacker: attacker.map(ToOwned::to_owned),
            target: target.map(ToOwned::to_owned),
            inflictor: (event_type == "DotaCombatlogAbility")
                .then(|| "fixture_ability".to_string()),
            damage_source: attacker.map(ToOwned::to_owned),
            value,
            health: Some(0),
            attacker_team: Some(3),
            target_team: Some(2),
            location_x: None,
            location_y: None,
            attacker_is_hero: attacker.map(|_| true),
            target_is_hero: target.map(|_| true),
            long_range_kill: None,
            will_reincarnate: Some(false),
            assist_players: Vec::new(),
        }
    }

    fn valid_evidence_fixture() -> serde_json::Value {
        let players = (0..10)
            .map(|index| {
                serde_json::json!({
                    "player_id": format!("p{index}"),
                    "team": if index < 5 { "radiant" } else { "dire" },
                    "stats": {
                        "kills": if index == 0 { 1 } else { 0 },
                        "deaths": 0,
                        "assists": 0
                    }
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "schema_version": "dota2_story_export/1.0.0",
            "export_info": { "source_file_name": "fixture.dem" },
            "match": { "radiant_score": 1, "dire_score": 0 },
            "story_focus": { "protagonist_player_id": "p0" },
            "players": players,
            "events": [{
                "event_id": "e000001",
                "event_type": "hero_kill",
                "time": { "game_time_sec": 1.0 },
                "actor_player_id": "p0",
                "target_player_ids": ["p5"],
                "assist_player_ids": []
            }],
            "incidents": [],
            "data_quality": {
                "counts": { "story_events_exported": 1, "incidents_exported": 0 }
            }
        })
    }

    fn write_json(path: std::path::PathBuf, value: &impl serde::Serialize) {
        fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }
}
