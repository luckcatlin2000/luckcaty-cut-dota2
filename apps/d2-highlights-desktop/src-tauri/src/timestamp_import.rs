use d2_highlights_core::{CombatEvent, DemSource, TimelineDocument};
use d2_highlights_renderer::{ClipCameraMode, RenderTakeRole};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::Path,
};

const MAX_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportedClip {
    pub title: String,
    pub source_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_source_id: Option<String>,
    pub anchor_tick: u32,
    pub anchor_seconds: f32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreviewRequest {
    pub job_id: String,
    pub path: Option<String>,
    pub text: Option<String>,
    pub hero: Option<String>,
    pub time_basis: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreviewClip {
    pub id: String,
    pub hero: String,
    pub camera_mode: ClipCameraMode,
    pub take_group_id: Option<String>,
    pub take_role: RenderTakeRole,
    pub include_in_final: bool,
    pub start_seconds: f32,
    pub end_seconds: f32,
    pub display_start_seconds: f32,
    pub display_end_seconds: f32,
    pub imported: ImportedClip,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Preview {
    pub schema_version: String,
    pub match_id: String,
    pub hero: String,
    pub time_basis: String,
    pub source_name: String,
    pub clips: Vec<PreviewClip>,
}

#[derive(Deserialize)]
struct Plan {
    schema_version: String,
    match_id: String,
    hero: String,
    time_basis: String,
    source: Option<PlanSource>,
    clips: Vec<Row>,
}
#[derive(Deserialize)]
struct PlanSource {
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    id: String,
    title: String,
    start: String,
    end: String,
    #[serde(default)]
    hero: Option<String>,
    anchor: Option<Anchor>,
    camera_mode: Option<ClipCameraMode>,
    take_group_id: Option<String>,
    take_role: Option<RenderTakeRole>,
    include_in_final: Option<bool>,
}
#[derive(Deserialize)]
struct Anchor {
    demo_tick: u32,
    event_time_seconds: f32,
}

pub(crate) fn read_text(path: &Path) -> Result<String, String> {
    if !matches!(
        path.extension()
            .and_then(|v| v.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("json" | "txt")
    ) {
        return Err("请选择 JSON 片段方案或 TXT 时间表。".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|e| format!("无法读取方案文件：{e}"))?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err("方案文件超过 256 KB。".into());
    }
    String::from_utf8(bytes).map_err(|_| "方案文件需使用 UTF-8 编码。".into())
}

fn clock(text: &str) -> Result<f32, String> {
    let parts: Vec<_> = text.trim().split(':').collect();
    if !(2..=3).contains(&parts.len()) || parts.iter().any(|s| s.is_empty()) {
        return Err(format!("时间格式无效：{text}"));
    }
    let mut total = 0.0;
    for (i, part) in parts.iter().enumerate() {
        if !part.chars().all(|c| c.is_ascii_digit() || c == '.')
            || (i + 1 < parts.len() && part.contains('.'))
        {
            return Err(format!("时间格式无效：{text}"));
        }
        let value: f32 = part.parse().map_err(|_| format!("时间格式无效：{text}"))?;
        if !value.is_finite() || value < 0.0 || (i > 0 && value >= 60.0) {
            return Err(format!("时间格式无效：{text}"));
        }
        total = total * 60.0 + value;
    }
    Ok(total)
}

pub(crate) fn build_preview(
    text: &str,
    source_name: &str,
    source: &DemSource,
    timeline: &TimelineDocument,
    hero: Option<&str>,
    basis: Option<&str>,
) -> Result<Preview, String> {
    if text.len() > MAX_BYTES || timeline.source_sha256 != source.sha256 {
        return Err("方案或录像数据无效，请重新载入。".into());
    }
    let text = text.trim_start_matches('\u{feff}').trim();
    let match_id = timeline
        .replay
        .match_id
        .ok_or("本局缺少录像编号，不能验证方案。")?
        .to_string();
    let plan = if text.starts_with('{') {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("方案文件格式无效：{e}"))?;
        if value["schema_version"] != "d2h.timestamp-plan/1.2"
            && value["clips"].as_array().is_some_and(|clips| {
                clips.iter().any(|row| {
                    [
                        "camera_mode",
                        "take_group_id",
                        "take_role",
                        "include_in_final",
                    ]
                    .iter()
                    .any(|field| row.get(field).is_some())
                })
            })
        {
            return Err(
                "机位和主备分组字段需要 d2h.timestamp-plan/1.2，旧版本不能携带这些字段。".into(),
            );
        }
        let plan: Plan =
            serde_json::from_value(value).map_err(|e| format!("方案文件格式无效：{e}"))?;
        if !matches!(
            plan.schema_version.as_str(),
            "d2h.timestamp-plan/1.0"
                | "d2h.timestamp-plan/1.1"
                | "d2h.timestamp-plan/1.2"
                | "timestamp-import-proposal/v1"
        ) {
            return Err("片段方案版本不受支持。".into());
        }
        if plan.match_id != match_id {
            return Err(format!(
                "方案属于录像 {}，当前是 {}。",
                plan.match_id, match_id
            ));
        }
        if plan
            .source
            .as_ref()
            .is_some_and(|s| !s.sha256.eq_ignore_ascii_case(&source.sha256))
        {
            return Err("方案与当前录像文件不一致。".into());
        }
        if plan.schema_version == "d2h.timestamp-plan/1.2" && plan.source.is_none() {
            return Err("1.2 录制方案必须提供源录像 SHA-256。".into());
        }
        plan
    } else {
        let mut rows = Vec::new();
        for (index, line) in text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
        {
            let (range, title) = line
                .split_once(['｜', '|'])
                .ok_or_else(|| format!("第 {} 行请使用 开始—结束｜名称。", index + 1))?;
            let (start, end) = range
                .split_once(['—', '–', '-'])
                .ok_or("时间表需要完整起止范围。")?;
            rows.push(Row {
                id: format!("row-{}", index + 1),
                title: title.trim().into(),
                start: start.trim().into(),
                end: end.trim().into(),
                hero: None,
                anchor: None,
                camera_mode: None,
                take_group_id: None,
                take_role: None,
                include_in_final: None,
            });
        }
        Plan {
            schema_version: "d2h.timestamp-plan/1.0".into(),
            match_id: match_id.clone(),
            hero: hero.ok_or("请选择主角英雄。")?.into(),
            time_basis: basis.unwrap_or("game").into(),
            source: None,
            clips: rows,
        }
    };
    if !timeline
        .replay
        .players
        .iter()
        .any(|p| p.hero_name == plan.hero)
    {
        return Err("方案中的主角不在本局阵容中。".into());
    }
    if plan.clips.is_empty() || plan.clips.len() > 100 {
        return Err("请选择包含 1—100 个片段的方案。".into());
    }
    let offset = match plan.time_basis.as_str() {
        "game" => timeline
            .events
            .iter()
            .find(|e| e.event_type == "DotaCombatlogGameState" && e.value == Some(5))
            .and_then(|e| e.time_seconds)
            .ok_or("缺少比赛开始时钟，无法转换游戏时间。")?,
        "demo" => 0.0,
        _ => return Err("方案时间依据需为游戏时钟或录像事件时间。".into()),
    };
    let mut ids = HashSet::new();
    let mut fingerprints = HashSet::new();
    let mut clips = Vec::new();
    let extended = plan.schema_version == "d2h.timestamp-plan/1.2";
    for row in plan.clips {
        if row.id.is_empty()
            || !ids.insert(row.id.clone())
            || row.title.trim().is_empty()
            || row.title.chars().count() > 120
        {
            return Err("片段编号重复或名称无效。".into());
        }
        let (camera_mode, take_group_id, take_role, include_in_final) = if extended {
            let camera = row.camera_mode.ok_or("1.2 片段缺少 camera_mode。")?;
            if !matches!(
                camera,
                ClipCameraMode::PlayerPerspective | ClipCameraMode::HeroChase
            ) {
                return Err("1.2 片段机位只支持 player_perspective 或 hero_chase。".into());
            }
            let group = row
                .take_group_id
                .as_deref()
                .map(|group| {
                    if group.trim().is_empty() || group.chars().count() > 256 {
                        return Err("1.2 场次编号不能为空或超过 256 个字符。".to_string());
                    }
                    if row.anchor.is_none() {
                        return Err("同场主备机位必须提供共同的事件锚点。".to_string());
                    }
                    let key = format!("{}|{group}", source.sha256);
                    Ok(format!(
                        "wtf-group-{}",
                        &format!("{:x}", Sha256::digest(key.as_bytes()))[..32]
                    ))
                })
                .transpose()?;
            (
                camera,
                group,
                row.take_role.ok_or("1.2 片段缺少 take_role。")?,
                row.include_in_final
                    .ok_or("1.2 片段缺少 include_in_final。")?,
            )
        } else {
            (
                ClipCameraMode::PlayerPerspective,
                None,
                RenderTakeRole::Primary,
                true,
            )
        };
        let clip_hero = row.hero.as_deref().unwrap_or(&plan.hero);
        if !timeline
            .replay
            .players
            .iter()
            .any(|p| p.hero_name == clip_hero)
        {
            return Err(format!("“{}”的录制英雄不在本局阵容中。", row.title));
        }
        let display_start = clock(&row.start)?;
        let display_end = clock(&row.end)?;
        let start = ((display_start + offset) * 30.0).round() / 30.0;
        let end = ((display_end + offset) * 30.0).round() / 30.0;
        let frames = (end * 30.0).round() - (start * 30.0).round();
        if start < 0.0
            || end > timeline.replay.playback_time_seconds
            || !(30.0..=3000.0).contains(&frames)
        {
            return Err(format!(
                "“{}”超出录像范围，或时长不在 1—100 秒之间。",
                row.title
            ));
        }
        let event = if let Some(anchor) = &row.anchor {
            timeline
                .events
                .iter()
                .find(|e| {
                    e.tick == anchor.demo_tick
                        && e.time_seconds
                            .is_some_and(|t| (t - anchor.event_time_seconds).abs() < 0.002)
                })
                .ok_or("方案锚点与本局事件不一致。")?
        } else {
            timeline
                .events
                .iter()
                .filter(|e| e.time_seconds.is_some_and(|t| t >= start && t <= end))
                .min_by(|a, b| {
                    let score = |e: &CombatEvent| {
                        (if e.attacker.as_deref() == Some(clip_hero)
                            || e.target.as_deref() == Some(clip_hero)
                        {
                            0.0
                        } else {
                            1000.0
                        }) + (e.time_seconds.unwrap_or_default() - (start + end) / 2.0).abs()
                    };
                    score(a).total_cmp(&score(b))
                })
                .ok_or("片段内没有可验证的事件定位锚点。")?
        };
        let anchor_seconds = event.time_seconds.ok_or("事件缺少时间。")?;
        if anchor_seconds < start || anchor_seconds > end {
            return Err("事件锚点不在片段范围内。".into());
        }
        let mut fingerprint = format!(
            "{}|{}|{}|{}|{}|{}",
            source.sha256,
            clip_hero,
            start.to_bits(),
            end.to_bits(),
            event.tick,
            anchor_seconds.to_bits()
        );
        if extended {
            fingerprint.push_str(&format!(
                "|{camera_mode:?}|{take_group_id:?}|{take_role:?}|{include_in_final}"
            ));
        }
        let content_id = format!("{:x}", Sha256::digest(fingerprint.as_bytes()));
        if !fingerprints.insert(content_id.clone()) {
            return Err("同一方案中存在重复片段，请保留一条。".into());
        }
        let source_id = if extended {
            format!(
                "{:x}",
                Sha256::digest(
                    format!("timestamp/1.2/shot|{}|{}", source.sha256, row.id).as_bytes()
                )
            )
        } else {
            content_id
        };
        let candidate_source_id = take_group_id.as_ref().map(|group| {
            let key = format!(
                "timestamp/1.2/group|{}|{group}|{}|{}|{}|{}",
                source.sha256,
                start.to_bits(),
                end.to_bits(),
                event.tick,
                anchor_seconds.to_bits()
            );
            format!("{:x}", Sha256::digest(key.as_bytes()))
        });
        clips.push(PreviewClip {
            id: format!(
                "import-{}",
                candidate_source_id.as_ref().unwrap_or(&source_id)
            ),
            hero: clip_hero.to_string(),
            camera_mode,
            take_group_id,
            take_role,
            include_in_final,
            start_seconds: start,
            end_seconds: end,
            display_start_seconds: display_start,
            display_end_seconds: display_end,
            imported: ImportedClip {
                title: row.title.trim().into(),
                source_id,
                candidate_source_id,
                anchor_tick: event.tick,
                anchor_seconds,
            },
        });
    }
    validate_preview_groups(&clips)?;
    Ok(Preview {
        schema_version: plan.schema_version,
        match_id,
        hero: plan.hero,
        time_basis: plan.time_basis,
        source_name: source_name.into(),
        clips,
    })
}

fn validate_preview_groups(clips: &[PreviewClip]) -> Result<(), String> {
    let mut groups: HashMap<&str, Vec<&PreviewClip>> = HashMap::new();
    for clip in clips {
        if let Some(group) = clip.take_group_id.as_deref() {
            groups.entry(group).or_default().push(clip);
        } else if clip.take_role != RenderTakeRole::Primary || !clip.include_in_final {
            return Err("独立片段必须是默认入片的主机位；备用机位必须有场次分组。".into());
        }
    }
    for group in groups.values() {
        let primaries: Vec<_> = group
            .iter()
            .filter(|clip| clip.take_role == RenderTakeRole::Primary)
            .collect();
        if primaries.len() != 1 || !primaries[0].include_in_final {
            return Err("每个场次必须且只能有一个默认入片的主机位。".into());
        }
        let primary = primaries[0];
        for clip in group {
            if clip.take_role == RenderTakeRole::Alternate && clip.include_in_final {
                return Err("备用机位不能加入默认成片。".into());
            }
            if clip.id != primary.id
                || clip.start_seconds != primary.start_seconds
                || clip.end_seconds != primary.end_seconds
                || clip.imported.anchor_tick != primary.imported.anchor_tick
                || clip.imported.anchor_seconds != primary.imported.anchor_seconds
            {
                return Err("同场主备机位必须使用相同时间窗、候选身份和事件锚点。".into());
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_anchor(
    imported: &ImportedClip,
    candidate_id: &str,
    timeline: &TimelineDocument,
) -> Result<(), String> {
    if imported.title.trim().is_empty()
        || imported.title.chars().count() > 120
        || imported.source_id.len() != 64
        || !imported.source_id.bytes().all(|b| b.is_ascii_hexdigit())
        || imported
            .candidate_source_id
            .as_ref()
            .is_some_and(|id| id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()))
        || candidate_id
            != format!(
                "import-{}",
                imported
                    .candidate_source_id
                    .as_ref()
                    .unwrap_or(&imported.source_id)
            )
        || !timeline.events.iter().any(|e| {
            e.tick == imported.anchor_tick
                && e.time_seconds
                    .is_some_and(|t| (t - imported.anchor_seconds).abs() < 0.002)
        })
    {
        return Err("导入片段的名称或定位锚点无效。".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn data() -> (DemSource, TimelineDocument) {
        let source = serde_json::from_value(
            json!({"path":"fixture.dem","byte_length":1,"sha256":"abc","magic":"PBDEMS2"}),
        )
        .unwrap();
        let timeline=serde_json::from_value(json!({"schema_version":"1.4","source_sha256":"abc","parser":{"name":"fixture","version":"1"},
            "replay":{"playback_ticks":18000,"playback_time_seconds":600,"game_build":1,"match_id":42,"game_mode":null,"game_winner":null,
            "players":[{"slot":7,"hero_name":"npc_dota_hero_mirana","game_team":3,"is_fake_client":false}]},
            "events":[{"tick":600,"time_seconds":100,"event_type":"DotaCombatlogGameState","value":5,"assist_players":[]},
            {"tick":9404,"time_seconds":187.26667,"event_type":"DotaCombatlogDeath","attacker":"npc_dota_hero_mirana","assist_players":[]}]})).unwrap();
        (source, timeline)
    }

    fn multi_camera_plan() -> (DemSource, TimelineDocument, serde_json::Value) {
        let (source, mut timeline) = data();
        let mut other = timeline.replay.players[0].clone();
        other.slot = 2;
        other.hero_name = "npc_dota_hero_morphling".into();
        timeline.replay.players.push(other);
        let plan = json!({
            "schema_version":"d2h.timestamp-plan/1.2", "match_id":"42",
            "hero":"npc_dota_hero_mirana", "time_basis":"demo", "source":{"sha256":"abc"},
            "clips":[
                {"id":"shot-A", "title":"Primary", "start":"00:02:45.000", "end":"00:03:14.000",
                    "hero":"npc_dota_hero_mirana", "anchor":{"demo_tick":9404,"event_time_seconds":187.26667},
                    "camera_mode":"player_perspective", "take_group_id":"scene-one", "take_role":"primary", "include_in_final":true},
                {"id":"shot-B", "title":"Alternate", "start":"00:02:45.000", "end":"00:03:14.000",
                    "hero":"npc_dota_hero_morphling", "anchor":{"demo_tick":9404,"event_time_seconds":187.26667},
                    "camera_mode":"hero_chase", "take_group_id":"scene-one", "take_role":"alternate", "include_in_final":false}
            ]
        });
        (source, timeline, plan)
    }

    #[test]
    fn version_12_keeps_independent_shots_with_one_shared_candidate_and_anchor() {
        let (source, timeline, mut plan) = multi_camera_plan();
        let long_group = "scene-".repeat(35);
        for row in plan["clips"].as_array_mut().unwrap() {
            row["take_group_id"] = json!(long_group);
        }
        let preview = build_preview(
            &plan.to_string(),
            "wtf.json",
            &source,
            &timeline,
            None,
            None,
        )
        .unwrap();
        let primary = &preview.clips[0];
        let alternate = &preview.clips[1];
        assert_eq!(primary.id, alternate.id);
        assert_ne!(primary.imported.source_id, alternate.imported.source_id);
        assert_eq!(
            primary.imported.candidate_source_id,
            alternate.imported.candidate_source_id
        );
        assert_eq!(primary.take_group_id, alternate.take_group_id);
        assert!(primary.take_group_id.as_ref().unwrap().len() < 96);
        assert_eq!(primary.camera_mode, ClipCameraMode::PlayerPerspective);
        assert_eq!(alternate.camera_mode, ClipCameraMode::HeroChase);
        assert_eq!(alternate.hero, "npc_dota_hero_morphling");
        assert!(primary.include_in_final);
        assert!(!alternate.include_in_final);
        for clip in &preview.clips {
            validate_anchor(&clip.imported, &clip.id, &timeline).unwrap();
        }
    }

    #[test]
    fn old_versions_reject_new_fields_even_when_null_instead_of_silently_dropping_them() {
        let (source, timeline) = data();
        let legacy = json!({"schema_version":"d2h.timestamp-plan/1.1","match_id":"42","hero":"npc_dota_hero_mirana",
            "time_basis":"game","clips":[{"id":"one","title":"Legacy","start":"01:05","end":"01:34"}]});
        let preview = build_preview(
            &legacy.to_string(),
            "legacy.json",
            &source,
            &timeline,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            preview.clips[0].camera_mode,
            ClipCameraMode::PlayerPerspective
        );
        assert_eq!(preview.clips[0].take_role, RenderTakeRole::Primary);
        assert!(preview.clips[0].include_in_final);
        assert!(preview.clips[0].take_group_id.is_none());
        assert!(preview.clips[0].imported.candidate_source_id.is_none());
        for version in [
            "d2h.timestamp-plan/1.0",
            "d2h.timestamp-plan/1.1",
            "timestamp-import-proposal/v1",
        ] {
            for field in [
                "camera_mode",
                "take_group_id",
                "take_role",
                "include_in_final",
            ] {
                let mut invalid = legacy.clone();
                invalid["schema_version"] = json!(version);
                invalid["clips"][0][field] = serde_json::Value::Null;
                let error = build_preview(
                    &invalid.to_string(),
                    "old.json",
                    &source,
                    &timeline,
                    None,
                    None,
                )
                .err()
                .unwrap();
                assert!(error.contains("1.2"));
            }
        }
    }

    #[test]
    fn version_12_rejects_incomplete_or_inconsistent_take_groups_before_import() {
        let (source, mut timeline, plan) = multi_camera_plan();
        timeline.events.push(serde_json::from_value(json!({"tick":9000,"time_seconds":180,"event_type":"DotaCombatlogDamage","assist_players":[]})).unwrap());
        let mut invalid_batches = Vec::new();
        let mut changed = plan.clone();
        changed["clips"][0]["take_role"] = json!("alternate");
        changed["clips"][0]["include_in_final"] = json!(false);
        invalid_batches.push(changed);
        let mut changed = plan.clone();
        changed["clips"][1]["take_role"] = json!("primary");
        changed["clips"][1]["include_in_final"] = json!(true);
        invalid_batches.push(changed);
        for (field, value) in [
            ("include_in_final", json!(true)),
            ("take_group_id", serde_json::Value::Null),
            ("end", json!("00:03:10.000")),
            ("anchor", json!({"demo_tick":9000,"event_time_seconds":180})),
            ("anchor", serde_json::Value::Null),
            ("take_group_id", json!("")),
            ("take_group_id", json!("g".repeat(257))),
        ] {
            let mut changed = plan.clone();
            changed["clips"][1][field] = value;
            invalid_batches.push(changed);
        }
        for invalid in invalid_batches {
            assert!(
                build_preview(
                    &invalid.to_string(),
                    "invalid.json",
                    &source,
                    &timeline,
                    None,
                    None
                )
                .is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn version_12_requires_source_identity_supported_camera_and_explicit_take_metadata() {
        let (source, timeline, plan) = multi_camera_plan();
        for field in ["camera_mode", "take_role", "include_in_final"] {
            let mut invalid = plan.clone();
            invalid["clips"][0].as_object_mut().unwrap().remove(field);
            assert!(
                build_preview(
                    &invalid.to_string(),
                    "missing.json",
                    &source,
                    &timeline,
                    None,
                    None
                )
                .is_err()
            );
        }
        for camera in ["directed", "free_camera", "unknown"] {
            let mut invalid = plan.clone();
            invalid["clips"][0]["camera_mode"] = json!(camera);
            assert!(
                build_preview(
                    &invalid.to_string(),
                    "camera.json",
                    &source,
                    &timeline,
                    None,
                    None
                )
                .is_err()
            );
        }
        for value in [serde_json::Value::Null, json!({"sha256":"wrong"})] {
            let mut invalid = plan.clone();
            invalid["source"] = value;
            assert!(
                build_preview(
                    &invalid.to_string(),
                    "source.json",
                    &source,
                    &timeline,
                    None,
                    None
                )
                .is_err()
            );
        }
        let mut invalid = plan.clone();
        invalid["clips"][1]["hero"] = json!("npc_dota_hero_axe");
        assert!(
            build_preview(
                &invalid.to_string(),
                "hero.json",
                &source,
                &timeline,
                None,
                None
            )
            .is_err()
        );
    }

    #[test]
    fn version_12_roundtrip_preserves_alternate_semantics_and_opaque_old_drafts() {
        let (source, timeline, mut plan) = multi_camera_plan();
        let temp = tempfile::tempdir().unwrap();
        let job_id = "d2h-fixture";
        let job = temp.path().join(job_id);
        d2_highlights_core::write_json_pretty(&job.join("manifest.json"), &json!({
            "schema_version":"1.0","job_id":job_id,"created_unix_seconds":0,"source":source,"stages":{}})).unwrap();
        d2_highlights_core::write_json_pretty(&job.join("timeline/combat-events.json"), &timeline)
            .unwrap();
        d2_highlights_core::write_json_pretty(&job.join("timeline/highlights.json"), &json!({
            "schema_version":"1.4","source_sha256":"abc","detector":{"name":"fixture","version":"1"},"candidates":[]})).unwrap();
        let preview = build_preview(
            &plan.to_string(),
            "wtf.json",
            &source,
            &timeline,
            None,
            None,
        )
        .unwrap();
        let receipt = crate::append_timestamp_preview(temp.path(), job_id, preview).unwrap();
        assert_eq!(receipt["totalClipCount"], 2);
        assert_eq!(receipt["totalDurationSeconds"], 29.0);
        let draft_path = job.join("director/edit-plan.json");
        let mut saved: serde_json::Value =
            serde_json::from_slice(&fs::read(&draft_path).unwrap()).unwrap();
        let old_wtf = json!({"mode":"wtf","old_unknown":{"preserve":true},"clips":[{"legacy":"keep opaque"}]});
        saved["plans"].as_array_mut().unwrap().push(old_wtf.clone());
        d2_highlights_core::write_json_pretty(&draft_path, &saved).unwrap();
        for row in plan["clips"].as_array_mut().unwrap() {
            row["start"] = json!("00:02:50.000");
            row["end"] = json!("00:03:10.000");
        }
        let preview = build_preview(
            &plan.to_string(),
            "updated.json",
            &source,
            &timeline,
            None,
            None,
        )
        .unwrap();
        let receipt = crate::append_timestamp_preview(temp.path(), job_id, preview).unwrap();
        assert_eq!(receipt["totalClipCount"], 2);
        assert!(receipt["addedClipIds"].as_array().unwrap().is_empty());
        assert_eq!(receipt["updatedClipIds"].as_array().unwrap().len(), 2);
        assert_eq!(receipt["totalDurationSeconds"], 20.0);
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(&draft_path).unwrap()).unwrap();
        assert_eq!(saved["active_mode"], "default");
        assert_eq!(saved["plans"][1], old_wtf);
        assert_eq!(
            saved["plans"][0]["clips"][0]["candidate_id"],
            saved["plans"][0]["clips"][1]["candidate_id"]
        );
        assert_eq!(saved["plans"][0]["clips"][1]["camera_mode"], "hero_chase");
        assert_eq!(saved["plans"][0]["clips"][1]["take_role"], "alternate");
        assert_eq!(saved["plans"][0]["clips"][1]["include_in_final"], false);
        let document = crate::decode_highlight_document(saved).unwrap();
        crate::validate_imported_document(&document, &job).unwrap();
        crate::validate_edit_plan_take_groups(&document.plans[0].clips).unwrap();
        let original = fs::read(&draft_path).unwrap();
        plan["clips"].as_array_mut().unwrap().truncate(1);
        plan["clips"][0]["id"] = json!("another-primary");
        let incomplete_addition = build_preview(
            &plan.to_string(),
            "conflict.json",
            &source,
            &timeline,
            None,
            None,
        )
        .unwrap();
        assert!(crate::append_timestamp_preview(temp.path(), job_id, incomplete_addition).is_err());
        assert_eq!(fs::read(draft_path).unwrap(), original);
    }
    #[test]
    fn one_json_plan_selects_and_anchors_each_hero_independently() {
        let (s, mut t) = data();
        for (slot, hero) in [
            (2, "npc_dota_hero_morphling"),
            (0, "npc_dota_hero_ogre_magi"),
        ] {
            let mut player = t.replay.players[0].clone();
            player.slot = slot;
            player.hero_name = hero.into();
            t.replay.players.push(player);
        }
        t.events.push(serde_json::from_value(json!({"tick":5400,"time_seconds":180,"event_type":"DotaCombatlogDamage","attacker":"npc_dota_hero_morphling","assist_players":[]})).unwrap());
        t.events.push(serde_json::from_value(json!({"tick":6900,"time_seconds":230,"event_type":"DotaCombatlogDamage","attacker":"npc_dota_hero_ogre_magi","assist_players":[]})).unwrap());
        let plan = json!({"schema_version":"d2h.timestamp-plan/1.1","match_id":"42","hero":"npc_dota_hero_mirana","time_basis":"game","clips":[
            {"id":"one","title":"mirana","start":"01:05","end":"01:34"},
            {"id":"two","title":"morphling","hero":"npc_dota_hero_morphling","start":"01:05","end":"01:34"},
            {"id":"three","title":"ogre","hero":"npc_dota_hero_ogre_magi","start":"02:00","end":"02:20"}
        ]});
        let p = build_preview(&plan.to_string(), "multi.json", &s, &t, None, None).unwrap();
        assert_eq!(
            p.clips.iter().map(|c| c.hero.as_str()).collect::<Vec<_>>(),
            vec![
                "npc_dota_hero_mirana",
                "npc_dota_hero_morphling",
                "npc_dota_hero_ogre_magi"
            ]
        );
        assert_eq!(
            p.clips
                .iter()
                .map(|c| c.imported.anchor_tick)
                .collect::<Vec<_>>(),
            vec![9404, 5400, 6900]
        );
        let legacy = build_preview(
            "01:05—01:34｜old",
            "old.txt",
            &s,
            &t,
            Some("npc_dota_hero_mirana"),
            Some("game"),
        )
        .unwrap();
        assert_eq!(
            p.clips[0].imported.source_id,
            legacy.clips[0].imported.source_id
        );
        assert_ne!(p.clips[0].imported.source_id, p.clips[1].imported.source_id);
    }

    #[test]
    fn invalid_row_hero_is_rejected_without_falling_back_to_the_global_hero() {
        let (s, t) = data();
        for hero in ["", "npc_dota_hero_morphling", "unknown"] {
            let plan = json!({"schema_version":"d2h.timestamp-plan/1.1","match_id":"42","hero":"npc_dota_hero_mirana","time_basis":"game","clips":[{"id":"one","title":"bad hero","hero":hero,"start":"01:05","end":"01:34"}]});
            let error = build_preview(&plan.to_string(), "x", &s, &t, None, None)
                .err()
                .unwrap();
            assert!(error.contains("录制英雄不在本局阵容中"));
        }
    }

    #[test]
    fn converts_game_clock_using_real_event_and_preserves_range() {
        let (s, t) = data();
        let p = build_preview(
            "01:05—01:34｜单杀",
            "paste",
            &s,
            &t,
            Some("npc_dota_hero_mirana"),
            Some("game"),
        )
        .unwrap();
        assert_eq!(p.clips[0].start_seconds, 165.0);
        assert_eq!(p.clips[0].end_seconds, 194.0);
        assert_eq!(p.clips[0].imported.anchor_tick, 9404);
        let again = build_preview(
            "01:05—01:34｜新名字",
            "other",
            &s,
            &t,
            Some("npc_dota_hero_mirana"),
            Some("game"),
        )
        .unwrap();
        assert_eq!(p.clips[0].id, again.clips[0].id);
    }
    #[test]
    fn rejects_wrong_match_hash_anchor_and_partial_invalid_batch() {
        let (s, t) = data();
        let mut plan = json!({"schema_version":"d2h.timestamp-plan/1.0","match_id":"43","hero":"npc_dota_hero_mirana","time_basis":"game","clips":[{"id":"1","title":"x","start":"01:05","end":"01:34"}]});
        assert!(build_preview(&plan.to_string(), "x", &s, &t, None, None).is_err());
        plan["match_id"] = json!("42");
        plan["source"] = json!({"sha256":"wrong"});
        assert!(build_preview(&plan.to_string(), "x", &s, &t, None, None).is_err());
        plan.as_object_mut().unwrap().remove("source");
        plan["clips"][0]["anchor"] = json!({"demo_tick":30,"event_time_seconds":187.26667});
        assert!(build_preview(&plan.to_string(), "x", &s, &t, None, None).is_err());
        assert!(
            build_preview(
                "01:05—01:34｜ok\n03:00—02:00｜bad",
                "x",
                &s,
                &t,
                Some("npc_dota_hero_mirana"),
                None
            )
            .is_err()
        );
    }
    #[test]
    fn rejects_missing_clock_invalid_minutes_and_fake_restored_anchor() {
        let (s, mut t) = data();
        t.events.remove(0);
        assert!(
            build_preview(
                "01:05—01:34｜x",
                "x",
                &s,
                &t,
                Some("npc_dota_hero_mirana"),
                None
            )
            .is_err()
        );
        assert!(clock("01:60").is_err());
        assert!(clock("NaN:20").is_err());
        let imported = ImportedClip {
            title: "x".into(),
            source_id: "a".repeat(64),
            candidate_source_id: None,
            anchor_tick: 123,
            anchor_seconds: 187.26667,
        };
        assert!(validate_anchor(&imported, &format!("import-{}", imported.source_id), &t).is_err());
    }
    #[test]
    fn rejects_duplicate_range_even_with_different_titles_and_row_ids() {
        let (s, t) = data();
        assert!(
            build_preview(
                "01:05—01:34｜one\n01:05—01:34｜two",
                "x",
                &s,
                &t,
                Some("npc_dota_hero_mirana"),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn restored_import_checks_source_and_anchor_without_modifying_draft() {
        let (s, t) = data();
        let dir = tempfile::tempdir().unwrap();
        let timeline_path = dir.path().join("timeline/combat-events.json");
        d2_highlights_core::write_json_pretty(&timeline_path, &t).unwrap();
        let p = build_preview(
            "01:05—01:34｜单杀",
            "x",
            &s,
            &t,
            Some("npc_dota_hero_mirana"),
            None,
        )
        .unwrap();
        let c = &p.clips[0];
        let clip = json!({"clip_id":"clip-import-1","candidate_id":c.id,"imported":c.imported,"view_hero":"npc_dota_hero_mirana",
            "source_start_seconds":165,"source_end_seconds":194});
        let value = json!({"schema_version":"d2h.edit-plan/1.6","source_sha256":"abc","job_id":"d2h-fixture",
            "updated_unix_seconds":0,"plans":[{"mode":"default","clips":[clip]},{"mode":"wtf","clips":[]}]});
        let mut document: crate::EditPlanDocument = serde_json::from_value(value).unwrap();
        let draft = dir.path().join("director/edit-plan.json");
        d2_highlights_core::write_json_pretty(&draft, &document).unwrap();
        let original = std::fs::read(&draft).unwrap();
        crate::validate_imported_document(&document, dir.path()).unwrap();
        document.plans[0].clips[0]
            .imported
            .as_mut()
            .unwrap()
            .anchor_tick = 30;
        assert!(crate::validate_imported_document(&document, dir.path()).is_err());
        document.source_sha256 = "other".into();
        assert!(crate::validate_imported_document(&document, dir.path()).is_err());
        assert_eq!(std::fs::read(draft).unwrap(), original);
    }
}
