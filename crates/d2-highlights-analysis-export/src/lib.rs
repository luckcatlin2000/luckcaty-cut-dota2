use d2_highlights_core::{
    CombatEvent, HighlightCandidate, HighlightDocument, JobManifest, ReplayPlayer,
    ReplayPlayerState, TIMELINE_SCHEMA_VERSION, TimelineDocument,
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
pub struct AnalysisPackageExportRequest {
    pub job_id: String,
    pub protagonist_hero: String,
    pub protagonist_label: String,
    pub destination_directory: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisPackageExportResult {
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

struct DirectorData {
    key_ability_windows: Vec<Value>,
    teamfights: Vec<Value>,
    state_checkpoints: Vec<Value>,
    ability_windows_supported: bool,
    final_player_state_supported: bool,
    economy_snapshots_supported: bool,
}

pub fn export_analysis_package(
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
    let director_data = build_director_data(
        timeline,
        protagonist_hero,
        &protagonist_id,
        &players,
        &hero_to_player,
        &slot_to_player,
        &story_events,
        game_start_seconds,
        game_end_seconds,
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
    let mut warnings = vec![
        "一至五号位不是 DEM 直接字段；role=unknown 是有意保留的事实边界，不代表解析失败。"
            .to_string(),
        "无法从现有 Buyback 事件稳定归属玩家时，buyback 不填写 actor_player_id。".to_string(),
        "本分析包不声明逐帧位置、完整物品时间线或战争迷雾能力。".to_string(),
        "extensions.time_mappings 按 JSON Pointer 标明派生时间换算依据；暂停、稀疏事件或范围外时间不盲目外推，改用已观察时间点并保留原请求时间。".to_string(),
    ];
    if !director_data.ability_windows_supported {
        warnings.push("当前任务来自旧时间轴；重新分析录像后才能生成关键技能持续窗口。".to_string());
    }
    if !director_data.final_player_state_supported {
        warnings.push(
            "当前任务没有覆盖完整十人的可靠轻量状态；等级、净资产或最终装备中的 0/空数组仅为兼容占位。"
                .to_string(),
        );
    }
    if director_data.state_checkpoints.is_empty() {
        warnings.push("缺少可验证的主角状态快照，未生成局势检查点。".to_string());
    }
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
        .map(|(index, player)| {
            player_value(
                index,
                player,
                &stats[index],
                latest_player_state(timeline, index, &player.hero_name),
            )
        })
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

    let parse_status = if director_data.ability_windows_supported
        && director_data.final_player_state_supported
        && !director_data.state_checkpoints.is_empty()
    {
        "complete"
    } else {
        "partial"
    };
    let mut unsupported_fields = vec![
        "players[].role_inference",
        "continuous_positions",
        "item_timings",
        "vision_events",
    ];
    let mut placeholder_fields = Vec::new();
    if !director_data.final_player_state_supported {
        unsupported_fields.push("players[].final_items");
        placeholder_fields.extend(["players[].stats.level", "players[].stats.net_worth"]);
    }
    if !director_data.ability_windows_supported {
        unsupported_fields.push("key_ability_windows");
    }
    if director_data.state_checkpoints.is_empty() {
        unsupported_fields.push("state_checkpoints");
    }

    let mut evidence = json!({
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
                "teamfight_detection": true,
                "ability_windows": director_data.ability_windows_supported,
                "objective_events": true,
                "economy_snapshots": director_data.economy_snapshots_supported,
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
        "key_ability_windows": director_data.key_ability_windows,
        "teamfights": director_data.teamfights,
        "state_checkpoints": director_data.state_checkpoints,
        "incidents": incidents,
        "data_quality": {
            "parse_status": parse_status,
            "warnings": warnings,
            "counts": {
                "source_events_scanned": source_events_scanned,
                "story_events_exported": story_events.len(),
                "ability_windows_exported": director_data.key_ability_windows.len(),
                "teamfights_exported": director_data.teamfights.len(),
                "state_checkpoints_exported": director_data.state_checkpoints.len(),
                "incidents_exported": incidents.len()
            }
        },
        "extensions": {
            "job_id": job_id,
            "protagonist_hero": protagonist_hero,
            "protagonist_label": protagonist_label,
            "source_contract": STORY_EXPORT_CONTRACT,
            "director_context": {
                "version": "lightweight/1.0.0",
                "snapshot_interval_seconds": 30,
                "facts_only": true,
                "role_policy": "not_inferred"
            },
            "placeholder_fields": placeholder_fields,
            "unsupported_fields": unsupported_fields
        }
    });
    // The external v1 schema forbids extra fields inside time_point, but its
    // extensions object explicitly allows additional properties.
    let mut time_mappings = Vec::new();
    extract_time_mappings(&mut evidence, "", &mut time_mappings);
    if time_mappings
        .iter()
        .any(|mapping| mapping["method"] == "unavailable")
    {
        return Err(
            "部分候选缺少可验证的回放时间，无法导出分析包。请重新分析录像后再试。".to_string(),
        );
    }
    evidence["extensions"]["time_mappings"] = json!({"version": "1", "points": time_mappings});
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

    let ability_windows = evidence["key_ability_windows"]
        .as_array()
        .ok_or_else(|| "分析包关键技能窗口列表无效。".to_string())?;
    for window in ability_windows {
        if !player_ids.contains(
            window["owner_player_id"]
                .as_str()
                .ok_or_else(|| "关键技能窗口缺少拥有者。".to_string())?,
        ) {
            return Err("关键技能窗口引用了不存在的拥有者。".to_string());
        }
        validate_range_order(&window["active_range"], "关键技能窗口")?;
        validate_range_order(&window["clip_range"], "关键技能录像范围")?;
        validate_reference_array(
            &window["participant_player_ids"],
            &player_ids,
            "关键技能窗口玩家",
        )?;
        validate_reference_array(&window["linked_event_ids"], &event_ids, "关键技能窗口事件")?;
        validate_reference_array(
            &window["result"]["objective_event_ids"],
            &event_ids,
            "关键技能窗口目标事件",
        )?;
    }

    let teamfights = evidence["teamfights"]
        .as_array()
        .ok_or_else(|| "分析包团战列表无效。".to_string())?;
    for fight in teamfights {
        validate_range_order(&fight["range"], "团战")?;
        validate_reference_array(&fight["participant_player_ids"], &player_ids, "团战玩家")?;
        for field in [
            "kill_event_ids",
            "key_event_ids",
            "follow_up_objective_event_ids",
        ] {
            validate_reference_array(&fight[field], &event_ids, "团战事件")?;
        }
    }

    let checkpoints = evidence["state_checkpoints"]
        .as_array()
        .ok_or_else(|| "分析包局势检查点列表无效。".to_string())?;
    let mut previous_checkpoint_time = f64::NEG_INFINITY;
    for checkpoint in checkpoints {
        let game_time = checkpoint["time"]["game_time_sec"]
            .as_f64()
            .ok_or_else(|| "局势检查点缺少比赛时间。".to_string())?;
        if game_time < previous_checkpoint_time {
            return Err("局势检查点没有按比赛时间排序。".to_string());
        }
        previous_checkpoint_time = game_time;
        for field in ["level", "net_worth", "kills", "deaths", "assists"] {
            if checkpoint["protagonist"][field].as_u64().is_none() {
                return Err(format!("局势检查点缺少主角 {field}。"));
            }
        }
    }

    let counts = &evidence["data_quality"]["counts"];
    if counts["story_events_exported"].as_u64() != Some(events.len() as u64)
        || counts["ability_windows_exported"].as_u64() != Some(ability_windows.len() as u64)
        || counts["teamfights_exported"].as_u64() != Some(teamfights.len() as u64)
        || counts["state_checkpoints_exported"].as_u64() != Some(checkpoints.len() as u64)
        || counts["incidents_exported"].as_u64() != Some(incidents.len() as u64)
    {
        return Err("分析包数量统计与实际内容不一致。".to_string());
    }
    Ok(())
}

fn validate_range_order(range: &Value, label: &str) -> Result<(), String> {
    let start = range["start"]["game_time_sec"]
        .as_f64()
        .ok_or_else(|| format!("{label}缺少开始时间。"))?;
    let end = range["end"]["game_time_sec"]
        .as_f64()
        .ok_or_else(|| format!("{label}缺少结束时间。"))?;
    if end < start {
        return Err(format!("{label}结束时间早于开始时间。"));
    }
    Ok(())
}

fn validate_reference_array(
    value: &Value,
    known: &BTreeSet<&str>,
    label: &str,
) -> Result<(), String> {
    let references = value
        .as_array()
        .ok_or_else(|| format!("{label}引用列表无效。"))?;
    if references
        .iter()
        .filter_map(Value::as_str)
        .any(|reference| !known.contains(reference))
    {
        return Err(format!("{label}引用了不存在的编号。"));
    }
    Ok(())
}

fn accumulate_player_stats(
    timeline: &TimelineDocument,
    hero_to_player: &HashMap<String, String>,
    slot_to_player: &HashMap<u8, String>,
    stats: &mut [PlayerStats],
) {
    for event in timeline
        .events
        .iter()
        .filter(|event| event.is_final_hero_death())
    {
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
        let Some(mapped) = clock_point_for_tick(timeline, salute.tick) else {
            continue;
        };
        let demo_time = mapped.seconds;
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
            mapped.as_value(game_start_seconds, None, Some(salute.tick)),
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
        records.push(record(value, demo_time, mapped.tick));
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
    let incident_type = match candidate.kind.as_str() {
        "team_fight" | "multikill" | "trade" => "teamfight",
        "buyback_dieback" => "comeback",
        "mechanical_counterplay" => "counterplay",
        "hero_kill_sequence" | "pickoff" => "gank",
        _ => "custom:highlight_candidate",
    };
    let start = mapped_time_point(timeline, candidate.start_seconds as f64, game_start_seconds);
    let end = mapped_time_point(timeline, candidate.end_seconds as f64, game_start_seconds);
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

#[allow(clippy::too_many_arguments)]
fn build_director_data(
    timeline: &TimelineDocument,
    protagonist_hero: &str,
    protagonist_id: &str,
    players: &[ReplayPlayer],
    hero_to_player: &HashMap<String, String>,
    slot_to_player: &HashMap<u8, String>,
    events: &[StoryEventRecord],
    game_start_seconds: f64,
    game_end_seconds: f64,
) -> DirectorData {
    let ability_windows_supported = timeline.schema_version == TIMELINE_SCHEMA_VERSION;
    let key_ability_windows = if ability_windows_supported {
        build_key_ability_windows(
            timeline,
            protagonist_hero,
            protagonist_id,
            players,
            events,
            game_start_seconds,
            game_end_seconds,
        )
    } else {
        Vec::new()
    };
    let teamfights = build_teamfights(
        timeline,
        protagonist_id,
        players,
        hero_to_player,
        slot_to_player,
        events,
        &key_ability_windows,
        game_start_seconds,
        game_end_seconds,
    );
    let final_player_state_supported = players.iter().enumerate().all(|(index, player)| {
        latest_player_state(timeline, index, &player.hero_name)
            .is_some_and(|state| state.level.is_some() && state.net_worth.is_some())
    });
    let state_checkpoints = build_state_checkpoints(
        timeline,
        protagonist_id,
        players,
        events,
        &key_ability_windows,
        &teamfights,
        game_start_seconds,
        game_end_seconds,
    );
    let economy_snapshots_supported = !state_checkpoints.is_empty()
        && state_checkpoints.iter().all(|checkpoint| {
            checkpoint.get("radiant_net_worth").is_some()
                && checkpoint.get("dire_net_worth").is_some()
        });
    DirectorData {
        key_ability_windows,
        teamfights,
        state_checkpoints,
        ability_windows_supported,
        final_player_state_supported,
        economy_snapshots_supported,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_key_ability_windows(
    timeline: &TimelineDocument,
    protagonist_hero: &str,
    protagonist_id: &str,
    players: &[ReplayPlayer],
    events: &[StoryEventRecord],
    game_start_seconds: f64,
    game_end_seconds: f64,
) -> Vec<Value> {
    let casts = timeline
        .events
        .iter()
        .filter(|event| {
            event.event_type == "DotaCombatlogAbility"
                && event.attacker.as_deref() == Some(protagonist_hero)
                && event.inflictor.is_some()
                && event.time_seconds.is_some()
        })
        .collect::<Vec<_>>();
    let owner_team = player_index(protagonist_id)
        .and_then(|index| players.get(index))
        .map(player_team)
        .unwrap_or("radiant");
    let mut windows = Vec::new();

    for (cast_index, cast) in casts.iter().enumerate() {
        let ability = cast.inflictor.as_deref().unwrap_or("unknown_ability");
        let start = f64::from(cast.time_seconds.unwrap_or_default());
        let next_same_cast = casts
            .iter()
            .skip(cast_index + 1)
            .find(|event| event.inflictor.as_deref() == Some(ability))
            .and_then(|event| event.time_seconds)
            .map(f64::from);
        let modifier_add = timeline.events.iter().find(|event| {
            event.event_type == "DotaCombatlogModifierAdd"
                && event
                    .time_seconds
                    .is_some_and(|time| f64::from(time) >= start && f64::from(time) <= start + 0.25)
                && event.target.as_deref() == Some(protagonist_hero)
                && event
                    .inflictor
                    .as_deref()
                    .is_some_and(|modifier| modifier_matches_ability(modifier, ability))
                && event
                    .context
                    .modifier_duration_seconds
                    .is_some_and(|duration| duration >= 10.0)
        });
        if cast.context.is_ultimate_ability != Some(true) && modifier_add.is_none() {
            continue;
        }
        let modifier_end = timeline.events.iter().find(|event| {
            event.event_type == "DotaCombatlogModifierRemove"
                && event.time_seconds.is_some_and(|time| {
                    let time = f64::from(time);
                    time >= start && next_same_cast.is_none_or(|next| time < next)
                })
                && event.target.as_deref() == Some(protagonist_hero)
                && event
                    .inflictor
                    .as_deref()
                    .is_some_and(|modifier| modifier_matches_ability(modifier, ability))
        });
        let fallback_duration = modifier_add
            .and_then(|event| event.context.modifier_duration_seconds)
            .map(f64::from)
            .unwrap_or(8.0);
        let replay_end = game_end_seconds.max(start);
        let end = modifier_end
            .and_then(|event| event.time_seconds)
            .map(f64::from)
            .unwrap_or_else(|| {
                (start + fallback_duration).min(next_same_cast.unwrap_or(start + fallback_duration))
            })
            .max(start)
            .min(replay_end);
        let active_events = events_in_demo_range(events, start, end);
        let mut participants = BTreeSet::from([protagonist_id.to_string()]);
        let mut linked_event_ids = Vec::new();
        let mut owner_team_kills = 0_usize;
        let mut enemy_team_kills = 0_usize;
        let mut owner_kills = 0_usize;
        let mut owner_assists = 0_usize;
        let mut owner_died = false;
        for event in active_events {
            participants.extend(event.participants.iter().cloned());
            if let Some(event_id) = story_event_id(event) {
                linked_event_ids.push(event_id.to_string());
            }
            if event.value["event_type"] == "hero_kill" {
                match event.value["actor_team"].as_str() {
                    Some(team) if team == owner_team => owner_team_kills += 1,
                    Some(_) => enemy_team_kills += 1,
                    None => {}
                }
                if event.value["actor_player_id"].as_str() == Some(protagonist_id) {
                    owner_kills += 1;
                }
                if value_array_contains(&event.value["assist_player_ids"], protagonist_id) {
                    owner_assists += 1;
                }
                if value_array_contains(&event.value["target_player_ids"], protagonist_id) {
                    owner_died = true;
                }
            }
        }
        let objective_event_ids = events
            .iter()
            .filter(|event| {
                event.demo_time_seconds >= start
                    && event.demo_time_seconds <= end + 120.0
                    && is_story_objective_event(&event.value)
            })
            .filter_map(story_event_id)
            .map(ToOwned::to_owned)
            .collect::<BTreeSet<_>>();
        let mut tags = BTreeSet::from(["key_ability".to_string()]);
        if cast.context.is_ultimate_ability == Some(true) {
            tags.insert("ultimate_ability".to_string());
        }
        if modifier_add.is_some() {
            tags.insert("sustained_effect".to_string());
        }
        if owner_kills > 0 || owner_assists > 0 {
            tags.insert("combat_contribution".to_string());
        }
        if !objective_event_ids.is_empty() {
            tags.insert("objective_conversion".to_string());
        }
        let clip_start = (start - 12.0).max(0.0);
        let clip_end = (end + 10.0).min(replay_end);
        windows.push(json!({
            "window_id": format!("ability-window-{:04}", windows.len() + 1),
            "owner_player_id": protagonist_id,
            "ability": { "internal_name": ability },
            "active_range": time_range(timeline, start, end, game_start_seconds),
            "clip_range": time_range(timeline, clip_start, clip_end, game_start_seconds),
            "participant_player_ids": participants,
            "linked_event_ids": linked_event_ids.into_iter().collect::<BTreeSet<_>>(),
            "result": {
                "owner_team_kills": owner_team_kills,
                "enemy_team_kills": enemy_team_kills,
                "owner_kills": owner_kills,
                "owner_assists": owner_assists,
                "owner_died": owner_died,
                "objective_event_ids": objective_event_ids
            },
            "tags": tags
        }));
    }
    windows
}

#[allow(clippy::too_many_arguments)]
fn build_teamfights(
    timeline: &TimelineDocument,
    protagonist_id: &str,
    players: &[ReplayPlayer],
    hero_to_player: &HashMap<String, String>,
    slot_to_player: &HashMap<u8, String>,
    events: &[StoryEventRecord],
    key_ability_windows: &[Value],
    game_start_seconds: f64,
    game_end_seconds: f64,
) -> Vec<Value> {
    let deaths = timeline
        .events
        .iter()
        .filter(|event| {
            event.is_final_hero_death()
                && event.time_seconds.is_some()
                && event
                    .target
                    .as_ref()
                    .is_some_and(|hero| hero_to_player.contains_key(hero))
        })
        .collect::<Vec<_>>();
    let mut clusters: Vec<Vec<&CombatEvent>> = Vec::new();
    for death in deaths {
        if let Some(cluster) = clusters.last_mut()
            && f64::from(death.time_seconds.unwrap_or_default())
                - f64::from(
                    cluster
                        .last()
                        .and_then(|event| event.time_seconds)
                        .unwrap_or_default(),
                )
                <= 15.0
        {
            cluster.push(death);
        } else {
            clusters.push(vec![death]);
        }
    }

    let mut teamfights = Vec::new();
    for cluster in clusters.into_iter().filter(|cluster| cluster.len() >= 2) {
        let start = (f64::from(
            cluster
                .first()
                .and_then(|event| event.time_seconds)
                .unwrap_or_default(),
        ) - 8.0)
            .max(0.0);
        let end = (f64::from(
            cluster
                .last()
                .and_then(|event| event.time_seconds)
                .unwrap_or_default(),
        ) + 10.0)
            .min(game_end_seconds.max(0.0));
        let relevant = timeline
            .events
            .iter()
            .filter(|event| {
                event.time_seconds.is_some_and(|time| {
                    let time = f64::from(time);
                    time >= start && time <= end
                }) && is_teamfight_context_event(event)
            })
            .collect::<Vec<_>>();
        let mut participants = BTreeSet::new();
        for event in &relevant {
            if let Some(actor) = actor_player_id(event, hero_to_player) {
                participants.insert(actor);
            }
            if let Some(target) = event
                .target
                .as_ref()
                .and_then(|hero| hero_to_player.get(hero))
            {
                participants.insert(target.clone());
            }
            participants.extend(assist_player_ids(event, slot_to_player, None));
        }
        let participant_teams = participants
            .iter()
            .filter_map(|player_id| player_index(player_id))
            .filter_map(|index| players.get(index))
            .map(player_team)
            .collect::<BTreeSet<_>>();
        if participants.len() < 4 || participant_teams.len() < 2 {
            continue;
        }

        let linked = events_in_demo_range(events, start, end);
        let kill_event_ids = linked
            .iter()
            .filter(|event| event.value["event_type"] == "hero_kill")
            .filter_map(|event| story_event_id(event))
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let key_event_ids = linked
            .iter()
            .filter(|event| is_key_story_event(&event.value))
            .filter_map(|event| story_event_id(event))
            .map(ToOwned::to_owned)
            .collect::<BTreeSet<_>>();
        let follow_up_objective_event_ids = events
            .iter()
            .filter(|event| {
                event.demo_time_seconds > end
                    && event.demo_time_seconds <= end + 120.0
                    && is_story_objective_event(&event.value)
            })
            .filter_map(story_event_id)
            .map(ToOwned::to_owned)
            .collect::<BTreeSet<_>>();
        let radiant_deaths = cluster
            .iter()
            .filter(|event| {
                event
                    .target
                    .as_ref()
                    .and_then(|hero| hero_to_player.get(hero))
                    .and_then(|player_id| player_index(player_id))
                    .and_then(|index| players.get(index))
                    .is_some_and(|player| player_team(player) == "radiant")
            })
            .count();
        let dire_deaths = cluster.len() - radiant_deaths;
        let winner = match radiant_deaths.cmp(&dire_deaths) {
            std::cmp::Ordering::Less => "radiant",
            std::cmp::Ordering::Greater => "dire",
            std::cmp::Ordering::Equal => "draw",
        };
        let protagonist_involved = participants.contains(protagonist_id);
        let protagonist_kills = linked
            .iter()
            .filter(|event| {
                event.value["event_type"] == "hero_kill"
                    && event.value["actor_player_id"].as_str() == Some(protagonist_id)
            })
            .count();
        let protagonist_deaths = linked
            .iter()
            .filter(|event| {
                event.value["event_type"] == "hero_kill"
                    && value_array_contains(&event.value["target_player_ids"], protagonist_id)
            })
            .count();
        let protagonist_assists = linked
            .iter()
            .filter(|event| {
                event.value["event_type"] == "hero_kill"
                    && value_array_contains(&event.value["assist_player_ids"], protagonist_id)
            })
            .count();
        let protagonist_damage = relevant
            .iter()
            .filter(|event| {
                event.event_type == "DotaCombatlogDamage"
                    && actor_player_id(event, hero_to_player).as_deref() == Some(protagonist_id)
                    && event.target_is_hero == Some(true)
            })
            .filter_map(|event| event.value)
            .map(u64::from)
            .sum::<u64>();
        let used_key_ability = key_ability_windows
            .iter()
            .any(|window| ranges_overlap(&window["active_range"], start, end, game_start_seconds));
        let mut fight = json!({
            "fight_id": format!("teamfight-{:04}", teamfights.len() + 1),
            "range": time_range(timeline, start, end, game_start_seconds),
            "participant_player_ids": participants,
            "kill_event_ids": kill_event_ids,
            "key_event_ids": key_event_ids,
            "winner": winner,
            "protagonist_involved": protagonist_involved,
            "protagonist_summary": {
                "kills": protagonist_kills,
                "deaths": protagonist_deaths,
                "assists": protagonist_assists,
                "damage": protagonist_damage,
                "used_key_ability": used_key_ability
            },
            "follow_up_objective_event_ids": follow_up_objective_event_ids,
            "tags": ["clustered_hero_deaths", "calculated_teamfight"]
        });
        if let Some(location) = average_event_location(&cluster) {
            fight
                .as_object_mut()
                .expect("teamfight object")
                .insert("location".to_string(), location);
        }
        if let (Some((radiant_before, dire_before)), Some((radiant_after, dire_after))) = (
            team_net_worth_at(timeline, players, start),
            team_net_worth_at(timeline, players, end),
        ) {
            fight.as_object_mut().expect("teamfight object").insert(
                "economy".to_string(),
                json!({
                    "radiant_net_worth_before": radiant_before,
                    "dire_net_worth_before": dire_before,
                    "radiant_net_worth_after": radiant_after,
                    "dire_net_worth_after": dire_after
                }),
            );
        }
        teamfights.push(fight);
    }
    teamfights
}

#[allow(clippy::too_many_arguments)]
fn build_state_checkpoints(
    timeline: &TimelineDocument,
    protagonist_id: &str,
    players: &[ReplayPlayer],
    events: &[StoryEventRecord],
    key_ability_windows: &[Value],
    teamfights: &[Value],
    game_start_seconds: f64,
    game_end_seconds: f64,
) -> Vec<Value> {
    let mut targets = BTreeSet::new();
    let mut interval = game_start_seconds;
    while interval < game_end_seconds {
        targets.insert((interval * 1_000.0).round() as i64);
        interval += 300.0;
    }
    targets.insert((game_end_seconds * 1_000.0).round() as i64);
    for fight in teamfights.iter().filter(|fight| {
        fight["protagonist_involved"] == true
            && ((fight["protagonist_summary"]["used_key_ability"] == true
                && fight["kill_event_ids"].as_array().map_or(0, Vec::len) >= 3)
                || fight["kill_event_ids"].as_array().map_or(0, Vec::len) >= 5)
    }) {
        for point in ["start", "end"] {
            if let Some(time) = fight["range"][point]["demo_time_sec"].as_f64() {
                targets.insert((time * 1_000.0).round() as i64);
            }
        }
    }

    let protagonist_index = player_index(protagonist_id).unwrap_or_default();
    let protagonist_hero = players
        .get(protagonist_index)
        .map(|player| player.hero_name.as_str())
        .unwrap_or_default();
    if let Some(snapshot) = timeline.state_snapshots.iter().find(|snapshot| {
        snapshot.time_seconds.is_some()
            && snapshot.players.iter().any(|state| {
                usize::from(state.roster_index) == protagonist_index
                    && state.hero_name == protagonist_hero
                    && state.level.is_some()
                    && state.net_worth.is_some()
            })
    }) && let Some(time) = snapshot.time_seconds
    {
        targets.insert((f64::from(time) * 1_000.0).round() as i64);
    }
    let mut checkpoints = Vec::new();
    let mut used_ticks = BTreeSet::new();
    for target_millis in targets {
        let target_time = target_millis as f64 / 1_000.0;
        let Some(snapshot) = snapshot_at_or_before(timeline, target_time) else {
            continue;
        };
        if !used_ticks.insert(snapshot.tick) {
            continue;
        }
        let Some(state) = snapshot.players.iter().find(|state| {
            usize::from(state.roster_index) == protagonist_index
                && state.hero_name == protagonist_hero
                && state.level.is_some()
                && state.net_worth.is_some()
        }) else {
            continue;
        };
        let demo_time = snapshot.time_seconds.map(f64::from).unwrap_or(target_time);
        let (radiant_score, dire_score) = score_before(events, demo_time, players);
        let (kills, deaths, assists) = player_kda_before(events, protagonist_id, demo_time);
        let items = state
            .items
            .iter()
            .take(10)
            .map(|item| json!({ "internal_name": item }))
            .collect::<Vec<_>>();
        let reason = checkpoint_reason(
            target_time,
            game_start_seconds,
            game_end_seconds,
            teamfights,
        );
        let mut world_state = Map::new();
        if let Some(at_night_time) = snapshot.at_night_time {
            let forced_night = at_night_time
                && key_ability_windows.iter().any(|window| {
                    window["ability"]["internal_name"]
                        .as_str()
                        .is_some_and(|ability| ability.contains("darkness"))
                        && ranges_overlap(
                            &window["active_range"],
                            demo_time,
                            demo_time,
                            game_start_seconds,
                        )
                });
            world_state.insert(
                "day_night".to_string(),
                json!(if forced_night {
                    "forced_night"
                } else if at_night_time {
                    "night"
                } else {
                    "day"
                }),
            );
        }
        let roshan_kills = events
            .iter()
            .filter(|event| {
                event.demo_time_seconds <= demo_time && event.value["event_type"] == "roshan_killed"
            })
            .count();
        world_state.insert("roshan_kill_count".to_string(), json!(roshan_kills));
        let mut checkpoint = json!({
            "checkpoint_id": format!("checkpoint-{:04}", checkpoints.len() + 1),
            "time": time_point(snapshot.tick, demo_time, game_start_seconds),
            "reason": reason,
            "radiant_score": radiant_score,
            "dire_score": dire_score,
            "protagonist": {
                "level": state.level,
                "net_worth": state.net_worth,
                "kills": kills,
                "deaths": deaths,
                "assists": assists,
                "items": items
            },
            "world_state": world_state
        });
        if let Some((radiant_net_worth, dire_net_worth)) =
            team_net_worth_in_snapshot(snapshot, players)
        {
            let object = checkpoint.as_object_mut().expect("checkpoint object");
            object.insert("radiant_net_worth".to_string(), json!(radiant_net_worth));
            object.insert("dire_net_worth".to_string(), json!(dire_net_worth));
        }
        checkpoints.push(checkpoint);
    }
    checkpoints
}

fn modifier_matches_ability(modifier: &str, ability: &str) -> bool {
    let modifier = modifier.strip_prefix("modifier_").unwrap_or(modifier);
    modifier == ability || modifier.ends_with(ability) || ability.ends_with(modifier)
}

fn events_in_demo_range(
    events: &[StoryEventRecord],
    start: f64,
    end: f64,
) -> Vec<&StoryEventRecord> {
    events
        .iter()
        .filter(|event| event.demo_time_seconds >= start && event.demo_time_seconds <= end)
        .collect()
}

fn story_event_id(event: &StoryEventRecord) -> Option<&str> {
    event.value["event_id"].as_str()
}

fn value_array_contains(value: &Value, expected: &str) -> bool {
    value
        .as_array()
        .into_iter()
        .flatten()
        .any(|value| value.as_str() == Some(expected))
}

fn is_story_objective_event(event: &Value) -> bool {
    matches!(
        event["event_type"].as_str(),
        Some(
            "tower_destroyed"
                | "barracks_destroyed"
                | "roshan_killed"
                | "tormentor_killed"
                | "aegis_picked_up"
        )
    )
}

fn is_key_story_event(event: &Value) -> bool {
    event["event_type"] == "hero_kill"
        || event["event_type"] == "ability_cast"
        || event["event_type"] == "buyback"
        || is_story_objective_event(event)
}

fn is_teamfight_context_event(event: &CombatEvent) -> bool {
    matches!(
        event.event_type.as_str(),
        "DotaCombatlogDamage"
            | "DotaCombatlogHeal"
            | "DotaCombatlogDeath"
            | "DotaCombatlogAbility"
            | "DotaCombatlogBuyback"
            | "DotaCombatlogModifierAdd"
            | "DotaCombatlogModifierRemove"
            | "DotaCombatlogModifierStack"
    )
}

fn time_range(timeline: &TimelineDocument, start: f64, end: f64, game_start_seconds: f64) -> Value {
    json!({
        "start": mapped_time_point(timeline, start, game_start_seconds),
        "end": mapped_time_point(timeline, end, game_start_seconds)
    })
}

fn ranges_overlap(range: &Value, start: f64, end: f64, game_start_seconds: f64) -> bool {
    let Some(range_start) = range["start"]["game_time_sec"].as_f64() else {
        return false;
    };
    let Some(range_end) = range["end"]["game_time_sec"].as_f64() else {
        return false;
    };
    range_start <= end - game_start_seconds && range_end >= start - game_start_seconds
}

fn average_event_location(events: &[&CombatEvent]) -> Option<Value> {
    let coordinates = events
        .iter()
        .filter_map(|event| Some((f64::from(event.location_x?), f64::from(event.location_y?))))
        .collect::<Vec<_>>();
    if coordinates.is_empty() {
        return None;
    }
    let count = coordinates.len() as f64;
    let x = coordinates.iter().map(|(x, _)| x).sum::<f64>() / count;
    let y = coordinates.iter().map(|(_, y)| y).sum::<f64>() / count;
    Some(json!({ "x": round3(x), "y": round3(y) }))
}

fn snapshot_at_or_before(
    timeline: &TimelineDocument,
    demo_time: f64,
) -> Option<&d2_highlights_core::ReplayStateSnapshot> {
    timeline.state_snapshots.iter().rev().find(|snapshot| {
        snapshot
            .time_seconds
            .is_some_and(|time| f64::from(time) <= demo_time)
    })
}

fn team_net_worth_at(
    timeline: &TimelineDocument,
    players: &[ReplayPlayer],
    demo_time: f64,
) -> Option<(u64, u64)> {
    team_net_worth_in_snapshot(snapshot_at_or_before(timeline, demo_time)?, players)
}

fn team_net_worth_in_snapshot(
    snapshot: &d2_highlights_core::ReplayStateSnapshot,
    players: &[ReplayPlayer],
) -> Option<(u64, u64)> {
    let mut radiant = Vec::new();
    let mut dire = Vec::new();
    for (index, player) in players.iter().enumerate() {
        let state = snapshot.players.iter().find(|state| {
            usize::from(state.roster_index) == index && state.hero_name == player.hero_name
        })?;
        let net_worth = u64::from(state.net_worth?);
        if player_team(player) == "radiant" {
            radiant.push(net_worth);
        } else {
            dire.push(net_worth);
        }
    }
    (radiant.len() == 5 && dire.len() == 5)
        .then(|| (radiant.into_iter().sum(), dire.into_iter().sum()))
}

fn score_before(
    events: &[StoryEventRecord],
    demo_time: f64,
    players: &[ReplayPlayer],
) -> (usize, usize) {
    let mut radiant = 0;
    let mut dire = 0;
    for event in events.iter().filter(|event| {
        event.demo_time_seconds <= demo_time && event.value["event_type"] == "hero_kill"
    }) {
        let Some(actor) = event.value["actor_player_id"].as_str() else {
            continue;
        };
        let Some(player) = player_index(actor).and_then(|index| players.get(index)) else {
            continue;
        };
        if player_team(player) == "radiant" {
            radiant += 1;
        } else {
            dire += 1;
        }
    }
    (radiant, dire)
}

fn player_kda_before(
    events: &[StoryEventRecord],
    player_id: &str,
    demo_time: f64,
) -> (usize, usize, usize) {
    let mut kills = 0;
    let mut deaths = 0;
    let mut assists = 0;
    for event in events.iter().filter(|event| {
        event.demo_time_seconds <= demo_time && event.value["event_type"] == "hero_kill"
    }) {
        if event.value["actor_player_id"].as_str() == Some(player_id) {
            kills += 1;
        }
        if value_array_contains(&event.value["target_player_ids"], player_id) {
            deaths += 1;
        }
        if value_array_contains(&event.value["assist_player_ids"], player_id) {
            assists += 1;
        }
    }
    (kills, deaths, assists)
}

fn checkpoint_reason(
    target_time: f64,
    game_start_seconds: f64,
    game_end_seconds: f64,
    teamfights: &[Value],
) -> &'static str {
    if (target_time - game_end_seconds).abs() <= 0.5 {
        return "match_end";
    }
    for fight in teamfights {
        if fight["range"]["start"]["demo_time_sec"]
            .as_f64()
            .is_some_and(|time| (time - target_time).abs() <= 0.5)
        {
            return "before_teamfight";
        }
        if fight["range"]["end"]["demo_time_sec"]
            .as_f64()
            .is_some_and(|time| (time - target_time).abs() <= 0.5)
        {
            return "after_teamfight";
        }
    }
    let interval_index = (target_time - game_start_seconds) / 300.0;
    if (interval_index - interval_index.round()).abs() <= 0.002 {
        "interval"
    } else {
        "custom"
    }
}

fn build_report(
    evidence: &Value,
    highlights: &HighlightDocument,
    protagonist_hero: &str,
    protagonist_label: &str,
    game_start_seconds: f64,
    warnings: &[String],
) -> String {
    let match_id = evidence["match"]["match_id"].as_str().unwrap_or("unknown");
    let event_count = evidence["events"].as_array().map_or(0, Vec::len);
    let incident_count = evidence["incidents"].as_array().map_or(0, Vec::len);
    let label = if protagonist_label.is_empty() {
        protagonist_hero
    } else {
        protagonist_label
    };
    let data_status = match evidence["data_quality"]["parse_status"].as_str() {
        Some("complete") => "完整支持",
        Some("failed") => "不可用",
        _ => "部分支持",
    };
    let mut report = format!(
        "# Dota 2 录像分析报告\n\n- 比赛编号：`{}`\n- 高光主角：{}（`{}`）\n- 事实事件：{}\n- 主角相关候选：{}\n- 数据状态：{}\n\n## 十人阵容\n\n| ID | 阵营 | 英雄 | 玩家昵称 | K/D/A |\n|---|---|---|---|---|\n",
        markdown_cell(match_id),
        markdown_cell(label),
        markdown_cell(protagonist_hero),
        event_count,
        incident_count,
        data_status
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

fn latest_player_state<'a>(
    timeline: &'a TimelineDocument,
    roster_index: usize,
    hero_name: &str,
) -> Option<&'a ReplayPlayerState> {
    timeline
        .state_snapshots
        .iter()
        .rev()
        .flat_map(|snapshot| snapshot.players.iter())
        .find(|state| {
            usize::from(state.roster_index) == roster_index && state.hero_name == hero_name
        })
}

fn player_value(
    index: usize,
    player: &ReplayPlayer,
    stats: &PlayerStats,
    state: Option<&ReplayPlayerState>,
) -> Value {
    let mut stats_value = json!({
        "kills": stats.kills,
        "deaths": stats.deaths,
        "assists": stats.assists,
        "level": state.and_then(|state| state.level).unwrap_or(0),
        "net_worth": state.and_then(|state| state.net_worth).unwrap_or(0)
    });
    if let Some(state) = state {
        for (key, value) in [
            ("gpm", state.gpm),
            ("xpm", state.xpm),
            ("last_hits", state.last_hits),
            ("observer_wards_placed", state.observer_wards_placed),
        ] {
            if let Some(value) = value {
                stats_value
                    .as_object_mut()
                    .expect("stats object")
                    .insert(key.to_string(), json!(value));
            }
        }
    }
    let final_items = state
        .map(|state| {
            state
                .items
                .iter()
                .take(10)
                .map(|item| json!({ "internal_name": item }))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut value = json!({
        "player_id": player_id(index),
        "roster_index": index,
        "dota_player_slot": player.slot,
        "team": player_team(player),
        "hero": { "internal_name": player.hero_name },
        "role": "unknown",
        "stats": stats_value,
        "final_items": final_items
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
        .attributed_hero(|hero| hero_to_player.contains_key(hero))
        .and_then(|hero| hero_to_player.get(hero))
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

#[derive(Clone, Copy)]
struct ClockPoint {
    tick: u32,
    seconds: f64,
    method: &'static str,
}

impl ClockPoint {
    fn as_value(
        self,
        game_start: f64,
        requested_seconds: Option<f64>,
        requested_tick: Option<u32>,
    ) -> Value {
        let mut value = time_point(self.tick, self.seconds, game_start);
        value["mapping"] = json!({
            "method": self.method,
            "requested_demo_time_sec": requested_seconds.map(round3),
            "requested_replay_tick": requested_tick,
            "time_offset_sec": requested_seconds.map(|requested| round3(self.seconds - requested))
        });
        value
    }
}

fn clock_anchors(timeline: &TimelineDocument) -> impl Iterator<Item = ClockPoint> + '_ {
    timeline.events.iter().filter_map(|event| {
        let seconds = f64::from(event.time_seconds?);
        (seconds.is_finite() && event.tick != u32::MAX).then_some(ClockPoint {
            tick: event.tick,
            seconds,
            method: "observed_event",
        })
    })
}

fn continuous_clock_interval(
    timeline: &TimelineDocument,
    before: ClockPoint,
    after: ClockPoint,
) -> bool {
    // Never assume a global offset or extrapolate over a pause. Only short,
    // observed brackets whose elapsed ticks agree with the replay rate qualify.
    if timeline.replay.playback_ticks <= 0
        || timeline.replay.playback_time_seconds <= 0.0
        || after.tick <= before.tick
    {
        return false;
    }
    let seconds_per_tick = f64::from(timeline.replay.playback_time_seconds)
        / f64::from(timeline.replay.playback_ticks);
    let elapsed = after.seconds - before.seconds;
    let tick_elapsed = f64::from(after.tick - before.tick) * seconds_per_tick;
    elapsed > 0.0
        && elapsed <= 2.0
        && (elapsed - tick_elapsed).abs() <= seconds_per_tick * 0.25 + 0.001
}

fn clock_point_for_seconds(timeline: &TimelineDocument, seconds: f64) -> Option<ClockPoint> {
    let before = clock_anchors(timeline)
        .filter(|point| point.seconds <= seconds)
        .max_by(|a, b| a.seconds.total_cmp(&b.seconds));
    let after = clock_anchors(timeline)
        .filter(|point| point.seconds >= seconds)
        .min_by(|a, b| a.seconds.total_cmp(&b.seconds));
    if let (Some(before), Some(after)) = (before, after) {
        if before.seconds == seconds {
            return Some(before);
        }
        if after.seconds == seconds {
            return Some(after);
        }
        if continuous_clock_interval(timeline, before, after) {
            let fraction = (seconds - before.seconds) / (after.seconds - before.seconds);
            let tick = (f64::from(before.tick) + fraction * f64::from(after.tick - before.tick))
                .round() as u32;
            let quantized_seconds = before.seconds
                + f64::from(tick - before.tick) / f64::from(after.tick - before.tick)
                    * (after.seconds - before.seconds);
            return Some(ClockPoint {
                tick,
                seconds: quantized_seconds,
                method: "validated_local_interpolation",
            });
        }
    }
    [before, after]
        .into_iter()
        .flatten()
        .min_by(|a, b| {
            (a.seconds - seconds)
                .abs()
                .total_cmp(&(b.seconds - seconds).abs())
        })
        .map(|mut point| {
            point.method = "nearest_observed_event";
            point
        })
}

fn clock_point_for_tick(timeline: &TimelineDocument, tick: u32) -> Option<ClockPoint> {
    let before = clock_anchors(timeline)
        .filter(|point| point.tick <= tick)
        .max_by_key(|point| point.tick);
    let after = clock_anchors(timeline)
        .filter(|point| point.tick >= tick)
        .min_by_key(|point| point.tick);
    if let (Some(before), Some(after)) = (before, after) {
        if before.tick == tick {
            return Some(before);
        }
        if after.tick == tick {
            return Some(after);
        }
        if continuous_clock_interval(timeline, before, after) {
            let fraction = f64::from(tick - before.tick) / f64::from(after.tick - before.tick);
            return Some(ClockPoint {
                tick,
                seconds: before.seconds + fraction * (after.seconds - before.seconds),
                method: "validated_local_interpolation",
            });
        }
    }
    [before, after]
        .into_iter()
        .flatten()
        .min_by_key(|point| point.tick.abs_diff(tick))
        .map(|mut point| {
            point.method = "nearest_observed_event";
            point
        })
}

fn mapped_time_point(timeline: &TimelineDocument, seconds: f64, game_start: f64) -> Value {
    if let Some(point) = clock_point_for_seconds(timeline, seconds) {
        return point.as_value(game_start, Some(seconds), None);
    }
    // This internal sentinel is rejected by build_analysis_package before any
    // files are created. It must never reach consumers as a factual time point.
    let mut value = time_point(0, 0.0, game_start);
    value["mapping"] = json!({"method": "unavailable", "requested_demo_time_sec": round3(seconds)});
    value
}

fn extract_time_mappings(value: &mut Value, pointer: &str, records: &mut Vec<Value>) {
    match value {
        Value::Object(object) => {
            if object.contains_key("game_time_sec")
                && let Some(mut mapping) = object.remove("mapping")
            {
                mapping["json_pointer"] = json!(pointer);
                records.push(mapping);
            }
            for (key, value) in object {
                let escaped = key.replace('~', "~0").replace('/', "~1");
                extract_time_mappings(value, &format!("{pointer}/{escaped}"), records);
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter_mut().enumerate() {
                extract_time_mappings(value, &format!("{pointer}/{index}"), records);
            }
        }
        _ => {}
    }
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
        AnalysisPackageExportRequest, build_analysis_package, export_analysis_package,
        format_unix_seconds_utc, sanitize_file_component, validate_evidence,
    };
    use d2_highlights_core::{
        CombatEvent, DemSource, DetectorIdentity, HighlightCandidate, HighlightDocument,
        JobManifest, ParserIdentity, ReplayMetadata, ReplayPlayer, ReplayPlayerState,
        ReplayStateSnapshot, StageRecord, StageStatus, TIMELINE_SCHEMA_VERSION, TimelineDocument,
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

    fn clock_timeline(events: Vec<CombatEvent>) -> TimelineDocument {
        TimelineDocument {
            schema_version: TIMELINE_SCHEMA_VERSION.into(),
            source_sha256: "clock".into(),
            parser: ParserIdentity {
                name: "fixture".into(),
                version: "1".into(),
            },
            replay: ReplayMetadata {
                playback_ticks: 30_000,
                playback_time_seconds: 1000.0,
                game_build: 1,
                match_id: None,
                game_mode: None,
                game_winner: None,
                players: Vec::new(),
            },
            events,
            tree_orders: Vec::new(),
            temporary_trees: Vec::new(),
            salutes: Vec::new(),
            state_snapshots: Vec::new(),
        }
    }

    #[test]
    fn derived_clock_points_use_local_offset_and_round_trip_within_one_tick() {
        let timeline = clock_timeline(vec![
            event(3000, 110.0, "clock", None, None, None),
            event(3030, 111.0, "clock", None, None, None),
        ]);
        let point = super::clock_point_for_seconds(&timeline, 110.31).unwrap();
        assert_eq!(point.tick, 3009); // local offset is +10 seconds, not tick / 30
        assert!((point.seconds - 110.3).abs() < 0.00001);
        assert_eq!(point.method, "validated_local_interpolation");
        let reverse = super::clock_point_for_tick(&timeline, point.tick).unwrap();
        assert!((reverse.seconds - point.seconds).abs() < 0.00001);
        let value = super::mapped_time_point(&timeline, 110.31, 10.0);
        assert_eq!(value["replay_tick"], 3009);
        assert_eq!(value["demo_time_sec"], 110.3);
        assert_eq!(value["game_time_sec"], 100.3);
    }

    #[test]
    fn pause_or_rate_discontinuity_never_produces_a_guessed_intermediate_tick() {
        let timeline = clock_timeline(vec![
            event(3000, 110.0, "clock", None, None, None),
            event(3090, 111.0, "clock", None, None, None),
        ]);
        let value = super::mapped_time_point(&timeline, 110.7, 10.0);
        assert_eq!(value["mapping"]["method"], "nearest_observed_event");
        assert_eq!(value["mapping"]["requested_demo_time_sec"], 110.7);
        assert_eq!(value["replay_tick"], 3090);
        assert_eq!(value["demo_time_sec"], 111.0);
        let tip = super::clock_point_for_tick(&timeline, 3060).unwrap();
        assert_eq!((tip.tick, tip.seconds), (3090, 111.0));
        assert_eq!(
            tip.as_value(10.0, None, Some(3060))["mapping"]["requested_replay_tick"],
            3060
        );
    }

    #[test]
    fn sparse_and_out_of_range_times_snap_to_observed_pairs_with_explicit_metadata() {
        let timeline = clock_timeline(vec![
            event(3000, 110.0, "clock", None, None, None),
            event(3300, 120.0, "clock", None, None, None),
        ]);
        for requested in [100.0, 114.0, 130.0] {
            let point = super::clock_point_for_seconds(&timeline, requested).unwrap();
            assert_eq!(point.method, "nearest_observed_event");
            assert!([(3000, 110.0), (3300, 120.0)].contains(&(point.tick, point.seconds)));
        }
        let empty = clock_timeline(Vec::new());
        assert!(super::clock_point_for_tick(&empty, 4000).is_none());
        assert_eq!(
            super::mapped_time_point(&empty, 100.0, 0.0)["mapping"]["method"],
            "unavailable"
        );
    }

    #[test]
    fn exported_owner_resolution_matches_detector_without_inferring_unknown_units() {
        let mut death = event(
            3000,
            110.0,
            "DotaCombatlogDeath",
            Some("npc_dota_summoned_unit"),
            Some("victim"),
            None,
        );
        death.damage_source = Some("npc_dota_hero_mirana".into());
        let roster = std::collections::HashMap::from([(
            "npc_dota_hero_mirana".to_string(),
            "p0".to_string(),
        )]);
        assert_eq!(
            super::actor_player_id(&death, &roster).as_deref(),
            Some("p0")
        );
        death.damage_source = Some("npc_dota_hero_unknown".into());
        assert!(super::actor_player_id(&death, &roster).is_none());
    }

    #[test]
    fn mapping_metadata_uses_extensions_without_extra_time_point_properties() {
        let timeline = clock_timeline(vec![event(3000, 110.0, "clock", None, None, None)]);
        let mut value = serde_json::json!({"incidents": [{"range": {"end": super::mapped_time_point(&timeline, 111.0, 10.0)}}]});
        let mut mappings = Vec::new();
        super::extract_time_mappings(&mut value, "", &mut mappings);
        let point = &value["incidents"][0]["range"]["end"];
        assert_eq!(point.as_object().unwrap().len(), 3);
        assert_eq!(point["replay_tick"], 3000);
        assert_eq!(point["demo_time_sec"], 110.0);
        assert_eq!(mappings[0]["json_pointer"], "/incidents/0/range/end");
        assert_eq!(mappings[0]["requested_demo_time_sec"], 111.0);
        assert_eq!(mappings[0]["method"], "nearest_observed_event");
    }

    #[test]
    fn unavailable_clock_rejects_export_without_changing_inputs_or_existing_output() {
        let root = tempdir().unwrap();
        let jobs_root = root.path().join("jobs");
        let output_root = root.path().join("exports");
        let job_id = "d2h-1111111111111111";
        let job_dir = jobs_root.join(job_id);
        fs::create_dir_all(job_dir.join("timeline")).unwrap();
        let existing_package = output_root.join("1_测试主角_分析包");
        fs::create_dir_all(&existing_package).unwrap();
        fs::write(existing_package.join("evidence.json"), b"existing evidence").unwrap();
        fs::write(existing_package.join("report.md"), b"existing report").unwrap();
        let dem_path = root.path().join("1.dem");
        fs::write(&dem_path, b"read-only replay fixture").unwrap();

        let source_hash = "1".repeat(64);
        let mut timeline = clock_timeline(Vec::new());
        timeline.source_sha256 = source_hash.clone();
        timeline.replay.match_id = Some(1);
        timeline.replay.players = (0_u8..10)
            .map(|slot| ReplayPlayer {
                slot,
                hero_name: format!("npc_dota_hero_fixture_{slot}"),
                player_name: None,
                game_team: Some(if slot < 5 { 2 } else { 3 }),
                is_fake_client: false,
            })
            .collect();
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
                title: "candidate with missing clock evidence".to_string(),
                score: 80.0,
                start_seconds: 30.0,
                peak_seconds: 40.0,
                end_seconds: 45.0,
                hero_deaths: 1,
                anchor_tick: 900,
                primary_hero: Some("npc_dota_hero_fixture_9".to_string()),
                participants: vec!["npc_dota_hero_fixture_9".to_string()],
                reasons: Vec::new(),
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
            stages: BTreeMap::new(),
        };
        write_json(job_dir.join("manifest.json"), &manifest);
        write_json(job_dir.join("timeline/combat-events.json"), &timeline);
        write_json(job_dir.join("timeline/highlights.json"), &highlights);
        let protected_paths = [
            dem_path,
            job_dir.join("manifest.json"),
            job_dir.join("timeline/combat-events.json"),
            job_dir.join("timeline/highlights.json"),
            existing_package.join("evidence.json"),
            existing_package.join("report.md"),
        ];
        let before = protected_paths
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect::<Vec<_>>();

        let error = export_analysis_package(
            &jobs_root,
            AnalysisPackageExportRequest {
                job_id: job_id.to_string(),
                protagonist_hero: "npc_dota_hero_fixture_9".to_string(),
                protagonist_label: "测试主角".to_string(),
                destination_directory: output_root.display().to_string(),
            },
        )
        .unwrap_err();
        assert!(error.contains("缺少可验证的回放时间"), "{error}");
        for (path, original) in protected_paths.iter().zip(before) {
            assert_eq!(fs::read(path).unwrap(), original, "{}", path.display());
        }
        let output_entries = fs::read_dir(&output_root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(output_entries, vec![existing_package]);
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
            state_snapshots: Vec::new(),
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
    fn lightweight_director_data_uses_decoded_context_and_sparse_snapshots() {
        let source_hash = "2".repeat(64);
        let players = (0_u8..10)
            .map(|slot| ReplayPlayer {
                slot,
                hero_name: format!("npc_dota_hero_fixture_{slot}"),
                player_name: None,
                game_team: Some(if slot < 5 { 2 } else { 3 }),
                is_fake_client: false,
            })
            .collect::<Vec<_>>();
        let mut ability = event(
            600,
            20.0,
            "DotaCombatlogAbility",
            Some("npc_dota_hero_fixture_9"),
            None,
            None,
        );
        ability.inflictor = Some("fixture_ultimate".to_string());
        let mut modifier_add = event(
            600,
            20.0,
            "DotaCombatlogModifierAdd",
            Some("npc_dota_hero_fixture_9"),
            Some("npc_dota_hero_fixture_9"),
            None,
        );
        modifier_add.inflictor = Some("modifier_fixture_ultimate".to_string());
        modifier_add.context.modifier_duration_seconds = Some(30.0);
        let mut first_kill = event(
            750,
            25.0,
            "DotaCombatlogDeath",
            Some("npc_dota_hero_fixture_9"),
            Some("npc_dota_hero_fixture_0"),
            None,
        );
        first_kill.assist_players = vec![7, 8];
        let mut second_kill = event(
            900,
            30.0,
            "DotaCombatlogDeath",
            Some("npc_dota_hero_fixture_9"),
            Some("npc_dota_hero_fixture_1"),
            None,
        );
        second_kill.assist_players = vec![7, 8];
        let mut modifier_remove = event(
            1_500,
            50.0,
            "DotaCombatlogModifierRemove",
            Some("npc_dota_hero_fixture_9"),
            Some("npc_dota_hero_fixture_9"),
            None,
        );
        modifier_remove.inflictor = Some("modifier_fixture_ultimate".to_string());
        let mut late_ability = event(
            2_640,
            88.0,
            "DotaCombatlogAbility",
            Some("npc_dota_hero_fixture_9"),
            None,
            None,
        );
        late_ability.inflictor = Some("fixture_ultimate".to_string());
        let mut late_modifier_add = event(
            2_640,
            88.0,
            "DotaCombatlogModifierAdd",
            Some("npc_dota_hero_fixture_9"),
            Some("npc_dota_hero_fixture_9"),
            None,
        );
        late_modifier_add.inflictor = Some("modifier_fixture_ultimate".to_string());
        late_modifier_add.context.modifier_duration_seconds = Some(30.0);
        let states = (0_u8..10)
            .map(|slot| ReplayPlayerState {
                roster_index: slot,
                hero_name: format!("npc_dota_hero_fixture_{slot}"),
                level: Some(12 + u32::from(slot)),
                net_worth: Some(5_000 + u32::from(slot) * 100),
                last_hits: Some(40 + u32::from(slot)),
                gpm: None,
                xpm: None,
                observer_wards_placed: None,
                items: vec![format!("item_fixture_{slot}")],
            })
            .collect::<Vec<_>>();
        let timeline = TimelineDocument {
            schema_version: TIMELINE_SCHEMA_VERSION.to_string(),
            source_sha256: source_hash.clone(),
            parser: ParserIdentity {
                name: "fixture".to_string(),
                version: "1".to_string(),
            },
            replay: ReplayMetadata {
                playback_ticks: 3_000,
                playback_time_seconds: 100.0,
                game_build: 1,
                match_id: Some(1_234_567_890),
                game_mode: Some(22),
                game_winner: Some(3),
                players: players.clone(),
            },
            events: vec![
                event(300, 10.0, "DotaCombatlogGameState", None, None, Some(5)),
                ability,
                modifier_add,
                first_kill,
                second_kill,
                modifier_remove,
                late_ability,
                late_modifier_add,
                event(2_700, 90.0, "DotaCombatlogGameState", None, None, Some(6)),
            ],
            tree_orders: Vec::new(),
            temporary_trees: Vec::new(),
            salutes: Vec::new(),
            state_snapshots: vec![
                ReplayStateSnapshot {
                    tick: 300,
                    time_seconds: Some(10.0),
                    at_night_time: Some(false),
                    players: states.clone(),
                },
                ReplayStateSnapshot {
                    tick: 2_700,
                    time_seconds: Some(90.0),
                    at_night_time: Some(true),
                    players: states,
                },
            ],
        };
        let highlights = HighlightDocument {
            schema_version: "1.4".to_string(),
            source_sha256: source_hash.clone(),
            detector: DetectorIdentity {
                name: "fixture".to_string(),
                version: "1".to_string(),
            },
            candidates: Vec::new(),
        };
        let manifest = JobManifest {
            schema_version: "1.0".to_string(),
            job_id: "fixture".to_string(),
            created_unix_seconds: 1,
            source: DemSource {
                path: "1234567890.dem".to_string(),
                byte_length: 1,
                sha256: source_hash,
                magic: "PBDEMS2".to_string(),
            },
            stages: BTreeMap::new(),
        };

        let package = build_analysis_package(
            "fixture",
            &manifest,
            &timeline,
            &highlights,
            "npc_dota_hero_fixture_9",
            "测试主角",
        )
        .expect("build enhanced package");
        let evidence = package.evidence;

        assert_eq!(evidence["data_quality"]["parse_status"], "complete");
        assert_eq!(evidence["key_ability_windows"].as_array().unwrap().len(), 2);
        let late_window = evidence["key_ability_windows"]
            .as_array()
            .unwrap()
            .last()
            .unwrap();
        assert_eq!(late_window["active_range"]["end"]["game_time_sec"], 80.0);
        assert_eq!(late_window["clip_range"]["end"]["game_time_sec"], 80.0);
        assert_eq!(evidence["teamfights"].as_array().unwrap().len(), 1);
        assert!(!evidence["state_checkpoints"].as_array().unwrap().is_empty());
        assert_eq!(evidence["players"][9]["stats"]["level"], 21);
        assert_eq!(evidence["players"][9]["stats"]["net_worth"], 5_900);
        assert_eq!(
            evidence["players"][9]["final_items"][0]["internal_name"],
            "item_fixture_9"
        );
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
    #[ignore = "requires D2H_REAL_JOBS_ROOT and D2H_REAL_EXPORT_ROOT"]
    fn export_real_job_from_environment() {
        let jobs_root = std::env::var_os("D2H_REAL_JOBS_ROOT")
            .map(std::path::PathBuf::from)
            .expect("D2H_REAL_JOBS_ROOT");
        let output_root = std::env::var_os("D2H_REAL_EXPORT_ROOT")
            .map(std::path::PathBuf::from)
            .expect("D2H_REAL_EXPORT_ROOT");
        let result = export_analysis_package(
            &jobs_root,
            AnalysisPackageExportRequest {
                job_id: "d2h-cf51b8745d95f024".to_string(),
                protagonist_hero: "npc_dota_hero_night_stalker".to_string(),
                protagonist_label: "暗夜魔王".to_string(),
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
            context: Default::default(),
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
