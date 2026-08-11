use d2_highlights_core::{
    CombatEvent, DetectorIdentity, HIGHLIGHT_SCHEMA_VERSION, HeroKillMoment,
    HeroKillSequenceEvidence, HighlightCandidate, HighlightDocument, InteractionEvidence,
    InteractionVerification, TemporaryTreeEvent, TemporaryTreeState, TimelineDocument,
    TreeOrderEvent,
};
use std::collections::{BTreeMap, BTreeSet};

pub const DETECTOR_NAME: &str = "rules-v2";
pub const DETECTOR_VERSION: &str = "0.5.0";
const CLUSTER_GAP_SECONDS: f32 = 18.0;
const SETUP_SECONDS: f32 = 12.0;
const RESULT_SECONDS: f32 = 8.0;
const HERO_KILL_SEQUENCE_GAP_SECONDS: f32 = 18.0;
const HERO_KILL_SEQUENCE_MAX_SECONDS: f32 = 75.0;
const HERO_KILL_SETUP_LOOKBACK_SECONDS: f32 = 20.0;
const HERO_KILL_DAMAGE_LOOKBACK_SECONDS: f32 = 8.0;
const HERO_KILL_ABILITY_LEAD_SECONDS: f32 = 2.5;
const HERO_KILL_VISUAL_LEAD_SECONDS: f32 = 1.0;
const HERO_KILL_RESULT_SECONDS: f32 = 3.0;
const TREE_COUNTER_WINDOW_SECONDS: f32 = 4.25;
const TREE_COUNTER_SETUP_SECONDS: f32 = 4.0;
const TREE_COUNTER_RESULT_SECONDS: f32 = 4.0;
const TREE_CREATION_WINDOW_TICKS: u32 = 16;
const TREE_ACTION_TICK_TOLERANCE: u32 = 2;
const FIRST_FIFTEEN_MINUTES_SECONDS: f32 = 15.0 * 60.0;
const REPEATED_INTERACTION_BONUS: f32 = 140.0;
const BUYBACK_DIEBACK_MAX_SECONDS: f32 = 90.0;
const BUYBACK_DIEBACK_SETUP_SECONDS: f32 = 2.0;
const BUYBACK_DIEBACK_RESULT_SECONDS: f32 = 4.0;

pub fn detect_highlights(
    timeline: &TimelineDocument,
    max_candidates: usize,
    min_score: f32,
) -> HighlightDocument {
    let hero_deaths = timeline
        .events
        .iter()
        .filter(|event| {
            event.event_type == "DotaCombatlogDeath"
                && event.target_is_hero == Some(true)
                && event.time_seconds.is_some()
        })
        .collect::<Vec<_>>();

    let mut clusters: Vec<Vec<&CombatEvent>> = Vec::new();
    for death in hero_deaths {
        let death_time = death.time_seconds.unwrap_or_default();
        let belongs_to_last = clusters
            .last()
            .and_then(|cluster| cluster.last())
            .and_then(|event| event.time_seconds)
            .map(|last_time| death_time - last_time <= CLUSTER_GAP_SECONDS)
            .unwrap_or(false);

        if belongs_to_last {
            clusters.last_mut().expect("cluster exists").push(death);
        } else {
            clusters.push(vec![death]);
        }
    }

    let mut candidates = clusters
        .iter()
        .map(|cluster| score_cluster(timeline, cluster))
        .filter(|candidate| candidate.score >= min_score)
        .collect::<Vec<_>>();
    candidates.extend(
        objective_candidates(timeline, &candidates)
            .into_iter()
            .filter(|candidate| candidate.score >= min_score),
    );
    candidates.extend(
        mechanical_counterplay_candidates(timeline)
            .into_iter()
            .filter(|candidate| candidate.score >= min_score),
    );

    candidates.sort_by(|left, right| {
        repeated_mechanical_story_priority(right)
            .cmp(&repeated_mechanical_story_priority(left))
            .then_with(|| right.score.total_cmp(&left.score))
            .then_with(|| left.start_seconds.total_cmp(&right.start_seconds))
    });
    candidates.truncate(max_candidates);

    for (index, candidate) in candidates.iter_mut().enumerate() {
        candidate.rank = index + 1;
        candidate.id = format!("hl-{:03}", index + 1);
    }
    let generic_candidate_count = candidates.len();
    let mut hero_kill_candidates = hero_kill_sequence_candidates(timeline);
    for (index, candidate) in hero_kill_candidates.iter_mut().enumerate() {
        candidate.rank = generic_candidate_count + index + 1;
        candidate.id = format!("hk-{:03}", index + 1);
    }
    let hero_kill_candidate_count = hero_kill_candidates.len();
    candidates.extend(hero_kill_candidates);
    let mut mistake_candidates = buyback_dieback_candidates(timeline);
    for (index, candidate) in mistake_candidates.iter_mut().enumerate() {
        candidate.rank = generic_candidate_count + hero_kill_candidate_count + index + 1;
        candidate.id = format!("ms-{:03}", index + 1);
    }
    candidates.extend(mistake_candidates);

    HighlightDocument {
        schema_version: HIGHLIGHT_SCHEMA_VERSION.to_string(),
        source_sha256: timeline.source_sha256.clone(),
        detector: DetectorIdentity {
            name: DETECTOR_NAME.to_string(),
            version: DETECTOR_VERSION.to_string(),
        },
        candidates,
    }
}

fn buyback_dieback_candidates(timeline: &TimelineDocument) -> Vec<HighlightCandidate> {
    let mut used_death_ticks = BTreeSet::new();
    timeline
        .events
        .iter()
        .filter(|event| event.event_type == "DotaCombatlogBuyback")
        .filter_map(|buyback| {
            let buyback_time = buyback.time_seconds?;
            let player_slot = u8::try_from(buyback.value?).ok()?;
            let hero = timeline
                .replay
                .players
                .iter()
                .find(|player| player.slot == player_slot)?
                .hero_name
                .as_str();
            let death = timeline
                .events
                .iter()
                .filter(|event| {
                    event.event_type == "DotaCombatlogDeath"
                        && event.target.as_deref() == Some(hero)
                        && event.target_is_hero == Some(true)
                        && event.will_reincarnate != Some(true)
                        && event.time_seconds.is_some_and(|death_time| {
                            death_time > buyback_time
                                && death_time - buyback_time <= BUYBACK_DIEBACK_MAX_SECONDS
                        })
                })
                .min_by(|left, right| {
                    left.time_seconds
                        .unwrap_or_default()
                        .total_cmp(&right.time_seconds.unwrap_or_default())
                })?;
            if !used_death_ticks.insert(death.tick) {
                return None;
            }
            let death_time = death.time_seconds?;
            let delay = death_time - buyback_time;
            let killer = death
                .attacker
                .as_deref()
                .filter(|name| name.starts_with("npc_dota_hero_"));
            let mut participants = BTreeSet::from([hero.to_string()]);
            participants.extend(killer.map(ToOwned::to_owned));
            let immediacy_score = ((BUYBACK_DIEBACK_MAX_SECONDS - delay)
                / BUYBACK_DIEBACK_MAX_SECONDS)
                .clamp(0.0, 1.0)
                * 70.0;

            Some(HighlightCandidate {
                id: String::new(),
                rank: 0,
                kind: "buyback_dieback".to_string(),
                title: format!("Buyback death after {delay:.1}s"),
                score: 90.0 + immediacy_score,
                start_seconds: (buyback_time - BUYBACK_DIEBACK_SETUP_SECONDS).max(0.0),
                peak_seconds: death_time,
                end_seconds: (death_time + BUYBACK_DIEBACK_RESULT_SECONDS)
                    .min(timeline.replay.playback_time_seconds),
                hero_deaths: 1,
                anchor_tick: death.tick,
                primary_hero: Some(hero.to_string()),
                participants: participants.into_iter().collect(),
                reasons: vec![
                    format!("player slot {player_slot} bought back at {buyback_time:.3}s"),
                    format!("the same hero died again {delay:.2}s later"),
                    "death does not consume a reincarnation".to_string(),
                ],
                interaction: None,
                kill_sequence: None,
            })
        })
        .collect()
}

fn score_cluster(timeline: &TimelineDocument, cluster: &[&CombatEvent]) -> HighlightCandidate {
    let first = cluster.first().expect("non-empty death cluster");
    let last = cluster.last().expect("non-empty death cluster");
    let first_time = first.time_seconds.unwrap_or_default();
    let last_time = last.time_seconds.unwrap_or(first_time);
    let context_start = (first_time - 3.0).max(0.0);
    let context_end = (last_time + 3.0).min(timeline.replay.playback_time_seconds);
    let context = timeline.events.iter().filter(|event| {
        event
            .time_seconds
            .map(|time| time >= context_start && time <= context_end)
            .unwrap_or(false)
    });

    let mut first_blood = 0_u32;
    let mut multikills = 0_u32;
    let mut killstreaks = 0_u32;
    let mut buybacks = 0_u32;
    let mut aegis_taken = 0_u32;
    let mut building_kills = 0_u32;
    for event in context {
        match event.event_type.as_str() {
            "DotaCombatlogFirstBlood" => first_blood += 1,
            "DotaCombatlogMultikill" => multikills += 1,
            "DotaCombatlogKillstreak" => killstreaks += 1,
            "DotaCombatlogBuyback" => buybacks += 1,
            "DotaCombatlogAegisTaken" => aegis_taken += 1,
            "DotaCombatlogTeamBuildingKill" => building_kills += 1,
            _ => {}
        }
    }

    let death_count = cluster.len();
    let long_range_kills = cluster
        .iter()
        .filter(|event| event.long_range_kill == Some(true))
        .count();
    let team_losses = cluster
        .iter()
        .filter_map(|event| event.target_team)
        .collect::<BTreeSet<_>>();
    let high_assist_kills = cluster
        .iter()
        .filter(|event| event.assist_players.len() >= 3)
        .count();

    let mut score = death_count as f32 * 18.0;
    let mut reasons = vec![format!(
        "{death_count} hero death(s) in {:.1}s",
        last_time - first_time
    )];

    if death_count >= 3 {
        score += 25.0;
        reasons.push("multi-hero fight cluster".to_string());
    }
    if death_count >= 5 {
        score += 20.0;
        reasons.push("large team-fight casualty count".to_string());
    }
    if team_losses.len() > 1 {
        score += 12.0;
        reasons.push("both teams lost heroes".to_string());
    }
    if first_blood > 0 {
        score += 20.0;
        reasons.push("first blood".to_string());
    }
    if multikills > 0 {
        score += multikills as f32 * 25.0;
        reasons.push(format!("{multikills} multikill event(s)"));
    }
    if killstreaks > 0 {
        score += killstreaks as f32 * 8.0;
        reasons.push(format!("{killstreaks} killstreak event(s)"));
    }
    if long_range_kills > 0 {
        score += long_range_kills as f32 * 10.0;
        reasons.push(format!("{long_range_kills} long-range kill(s)"));
    }
    if high_assist_kills > 0 {
        score += high_assist_kills as f32 * 5.0;
        reasons.push(format!("{high_assist_kills} heavily assisted kill(s)"));
    }
    if buybacks > 0 {
        score += buybacks as f32 * 20.0;
        reasons.push(format!("{buybacks} buyback event(s)"));
    }
    if aegis_taken > 0 {
        score += aegis_taken as f32 * 25.0;
        reasons.push(format!("{aegis_taken} Aegis event(s)"));
    }
    if building_kills > 0 {
        score += building_kills as f32 * 10.0;
        reasons.push(format!("{building_kills} building kill(s)"));
    }

    let kind = if first_blood > 0 {
        "first_blood"
    } else if multikills > 0 {
        "multikill"
    } else if death_count >= 3 {
        "team_fight"
    } else if team_losses.len() > 1 {
        "trade"
    } else {
        "pickoff"
    };

    let participants = cluster
        .iter()
        .flat_map(|event| [event.attacker.as_ref(), event.target.as_ref()])
        .flatten()
        .filter(|name| name.starts_with("npc_dota_hero_"))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    HighlightCandidate {
        id: String::new(),
        rank: 0,
        kind: kind.to_string(),
        title: title_for(kind, death_count),
        score,
        start_seconds: (first_time - SETUP_SECONDS).max(0.0),
        peak_seconds: last_time,
        end_seconds: (last_time + RESULT_SECONDS).min(timeline.replay.playback_time_seconds),
        hero_deaths: death_count,
        anchor_tick: last.tick,
        primary_hero: last
            .attacker
            .as_ref()
            .filter(|name| name.starts_with("npc_dota_hero_"))
            .cloned()
            .or_else(|| {
                last.target
                    .as_ref()
                    .filter(|name| name.starts_with("npc_dota_hero_"))
                    .cloned()
            }),
        participants,
        reasons,
        interaction: None,
        kill_sequence: None,
    }
}

#[derive(Clone, Copy)]
struct KillWithSetup<'a> {
    death: &'a CombatEvent,
    setup: &'a CombatEvent,
}

fn hero_kill_sequence_candidates(timeline: &TimelineDocument) -> Vec<HighlightCandidate> {
    let roster = timeline
        .replay
        .players
        .iter()
        .map(|player| player.hero_name.as_str())
        .collect::<BTreeSet<_>>();
    let mut kills_by_hero: BTreeMap<&str, Vec<KillWithSetup<'_>>> = BTreeMap::new();

    for death in timeline.events.iter().filter(|event| {
        event.event_type == "DotaCombatlogDeath"
            && event.target_is_hero == Some(true)
            && event.time_seconds.is_some()
            && event.will_reincarnate != Some(true)
            && event
                .attacker
                .as_deref()
                .is_some_and(|hero| roster.contains(hero))
            && event
                .target
                .as_deref()
                .is_some_and(|target| target.starts_with("npc_dota_hero_"))
            && match (event.attacker_team, event.target_team) {
                (Some(attacker_team), Some(target_team)) => attacker_team != target_team,
                _ => true,
            }
    }) {
        let killer = death.attacker.as_deref().expect("filtered attacker");
        let setup = find_kill_setup_event(timeline, death, killer);
        kills_by_hero
            .entry(killer)
            .or_default()
            .push(KillWithSetup { death, setup });
    }

    let mut candidates = Vec::new();
    for (hero, mut kills) in kills_by_hero {
        kills.sort_by(|left, right| {
            left.death
                .time_seconds
                .unwrap_or_default()
                .total_cmp(&right.death.time_seconds.unwrap_or_default())
        });
        let total_kills = kills.len();
        let mut sequences: Vec<Vec<KillWithSetup<'_>>> = Vec::new();
        for kill in kills {
            let joins_previous = sequences
                .last()
                .map(|sequence| {
                    let previous = sequence.last().expect("non-empty sequence");
                    let sequence_start = sequence
                        .iter()
                        .filter_map(|existing| existing.setup.time_seconds)
                        .min_by(f32::total_cmp)
                        .unwrap_or_default();
                    let kill_time = kill.death.time_seconds.unwrap_or_default();
                    kill_time - previous.death.time_seconds.unwrap_or_default()
                        <= HERO_KILL_SEQUENCE_GAP_SECONDS
                        && kill_time - sequence_start <= HERO_KILL_SEQUENCE_MAX_SECONDS
                })
                .unwrap_or(false);
            if joins_previous {
                sequences.last_mut().expect("sequence exists").push(kill);
            } else {
                sequences.push(vec![kill]);
            }
        }

        let sequence_count = sequences.len();
        for (sequence_index, sequence) in sequences.into_iter().enumerate() {
            let first_setup_time = sequence
                .iter()
                .filter_map(|kill| kill.setup.time_seconds)
                .min_by(f32::total_cmp)
                .unwrap_or_else(|| sequence[0].death.time_seconds.unwrap_or_default());
            let last_death = sequence.last().expect("non-empty kill sequence").death;
            let last_death_time = last_death.time_seconds.unwrap_or(first_setup_time);
            let kills = sequence
                .iter()
                .map(|kill| HeroKillMoment {
                    death_tick: kill.death.tick,
                    death_time_seconds: kill.death.time_seconds.unwrap_or_default(),
                    target_hero: kill
                        .death
                        .target
                        .clone()
                        .unwrap_or_else(|| "npc_dota_hero_unknown".to_string()),
                    inflictor: non_empty(kill.death.inflictor.as_deref()).map(ToOwned::to_owned),
                    setup_tick: kill.setup.tick,
                    setup_time_seconds: kill.setup.time_seconds.unwrap_or_default(),
                    setup_action: setup_action_name(kill.setup),
                })
                .collect::<Vec<_>>();
            let kill_count = kills.len();
            let participants = std::iter::once(hero.to_string())
                .chain(kills.iter().map(|kill| kill.target_hero.clone()))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let skill_setups = kills
                .iter()
                .filter(|kill| kill.setup_action.as_deref() != Some("basic_attack"))
                .count();
            let mut reasons = vec![format!(
                "{hero} received credit for {kill_count} hero kill(s) in one continuous sequence"
            )];
            reasons.extend(kills.iter().map(|kill| {
                format!(
                    "{} -> {} at {:.3}s",
                    kill.setup_action.as_deref().unwrap_or("basic_attack"),
                    kill.target_hero,
                    kill.death_time_seconds
                )
            }));

            candidates.push(HighlightCandidate {
                id: String::new(),
                rank: 0,
                kind: "hero_kill_sequence".to_string(),
                title: format!("{hero} kill sequence ({kill_count})"),
                score: 72.0 + kill_count as f32 * 28.0 + skill_setups as f32 * 4.0,
                start_seconds: (first_setup_time - HERO_KILL_VISUAL_LEAD_SECONDS).max(0.0),
                peak_seconds: last_death_time,
                end_seconds: (last_death_time + HERO_KILL_RESULT_SECONDS)
                    .min(timeline.replay.playback_time_seconds),
                hero_deaths: kill_count,
                anchor_tick: last_death.tick,
                primary_hero: Some(hero.to_string()),
                participants,
                reasons,
                interaction: None,
                kill_sequence: Some(HeroKillSequenceEvidence {
                    hero: hero.to_string(),
                    sequence_index: sequence_index + 1,
                    sequence_count,
                    total_kills,
                    kills,
                }),
            });
        }
    }
    candidates.sort_by(|left, right| {
        left.primary_hero
            .cmp(&right.primary_hero)
            .then_with(|| left.start_seconds.total_cmp(&right.start_seconds))
    });
    candidates
}

fn find_kill_setup_event<'a>(
    timeline: &'a TimelineDocument,
    death: &'a CombatEvent,
    killer: &str,
) -> &'a CombatEvent {
    let death_time = death.time_seconds.unwrap_or_default();
    if let Some(inflictor) = non_empty(death.inflictor.as_deref())
        && let Some(ability) = timeline.events.iter().rev().find(|event| {
            event.event_type == "DotaCombatlogAbility"
                && event.attacker.as_deref() == Some(killer)
                && non_empty(event.inflictor.as_deref()) == Some(inflictor)
                && event.time_seconds.is_some_and(|time| {
                    time <= death_time && death_time - time <= HERO_KILL_SETUP_LOOKBACK_SECONDS
                })
        })
    {
        return ability;
    }

    let target = death.target.as_deref();
    let first_damage = timeline
        .events
        .iter()
        .filter(|event| {
            event.event_type == "DotaCombatlogDamage"
                && event.attacker.as_deref() == Some(killer)
                && event.target.as_deref() == target
                && event.time_seconds.is_some_and(|time| {
                    time <= death_time && death_time - time <= HERO_KILL_DAMAGE_LOOKBACK_SECONDS
                })
        })
        .min_by(|left, right| {
            left.time_seconds
                .unwrap_or_default()
                .total_cmp(&right.time_seconds.unwrap_or_default())
        });
    let damage_time = first_damage
        .and_then(|event| event.time_seconds)
        .unwrap_or(death_time);
    if let Some(ability) = timeline.events.iter().rev().find(|event| {
        event.event_type == "DotaCombatlogAbility"
            && event.attacker.as_deref() == Some(killer)
            && (event.target.is_none() || event.target.as_deref() == target)
            && event.time_seconds.is_some_and(|time| {
                time <= damage_time && damage_time - time <= HERO_KILL_ABILITY_LEAD_SECONDS
            })
    }) {
        return ability;
    }
    first_damage.unwrap_or(death)
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

fn setup_action_name(event: &CombatEvent) -> Option<String> {
    non_empty(event.inflictor.as_deref())
        .map(ToOwned::to_owned)
        .or_else(|| (event.event_type == "DotaCombatlogDamage").then(|| "basic_attack".to_string()))
}

#[derive(Clone, Copy)]
struct TreeCounterMatch<'a> {
    trigger: &'a CombatEvent,
    response: &'a CombatEvent,
    tree_created: &'a TemporaryTreeEvent,
    tree_deleted: &'a TemporaryTreeEvent,
    tree_order: &'a TreeOrderEvent,
    related_bushwhack: bool,
}

fn mechanical_counterplay_candidates(timeline: &TimelineDocument) -> Vec<HighlightCandidate> {
    let mut ground_acorns = timeline
        .events
        .iter()
        .filter(|event| {
            event.event_type == "DotaCombatlogAbility"
                && event.attacker.as_deref() == Some("npc_dota_hero_hoodwink")
                && event.inflictor.as_deref() == Some("hoodwink_acorn_shot")
                && event
                    .target
                    .as_deref()
                    .is_none_or(|target| target.is_empty())
                && event.time_seconds.is_some()
        })
        .collect::<Vec<_>>();
    ground_acorns.sort_by_key(|event| event.tick);
    let mut quelling_uses = timeline
        .events
        .iter()
        .filter(|event| {
            event.event_type == "DotaCombatlogItem"
                && event.inflictor.as_deref() == Some("item_quelling_blade")
                && event.attacker_is_hero == Some(true)
                && event
                    .attacker
                    .as_deref()
                    .is_some_and(|attacker| attacker.starts_with("npc_dota_hero_"))
                && event.time_seconds.is_some()
        })
        .collect::<Vec<_>>();
    quelling_uses.sort_by_key(|event| event.tick);

    let mut used_tree_handles = BTreeSet::new();
    let mut grouped: BTreeMap<String, Vec<TreeCounterMatch<'_>>> = BTreeMap::new();
    for trigger in ground_acorns {
        let Some(tree_created) = timeline
            .temporary_trees
            .iter()
            .filter(|event| {
                event.state == TemporaryTreeState::Created
                    && event.tick >= trigger.tick
                    && event.tick <= trigger.tick.saturating_add(TREE_CREATION_WINDOW_TICKS)
                    && !used_tree_handles.contains(&event.entity_handle)
            })
            .min_by_key(|event| event.tick)
        else {
            continue;
        };
        let Some(tree_deleted) = timeline.temporary_trees.iter().find(|event| {
            event.state == TemporaryTreeState::Deleted
                && event.entity_handle == tree_created.entity_handle
                && event.tick >= tree_created.tick
        }) else {
            continue;
        };
        let Some(response) = quelling_uses.iter().copied().find(|event| {
            event.tick.abs_diff(tree_deleted.tick) <= TREE_ACTION_TICK_TOLERANCE
                && event.attacker.as_deref().is_some_and(|responder| {
                    heroes_are_opponents(timeline, "npc_dota_hero_hoodwink", responder)
                })
        }) else {
            continue;
        };
        let responder = response
            .attacker
            .as_deref()
            .expect("hero response has attacker");
        let Some(tree_order) = timeline
            .tree_orders
            .iter()
            .filter(|order| {
                order.tick.abs_diff(tree_deleted.tick) <= TREE_ACTION_TICK_TOLERANCE
                    && order
                        .unit_class_names
                        .iter()
                        .any(|class_name| hero_class_matches(class_name, responder))
            })
            .max_by_key(|order| order.tick)
        else {
            continue;
        };
        used_tree_handles.insert(tree_created.entity_handle);

        let trigger_time = trigger.time_seconds.unwrap_or_default();
        let response_time = response.time_seconds.unwrap_or(trigger_time);
        let related_bushwhack = timeline.events.iter().any(|event| {
            event.event_type == "DotaCombatlogAbility"
                && event.attacker.as_deref() == Some("npc_dota_hero_hoodwink")
                && event.inflictor.as_deref() == Some("hoodwink_bushwhack")
                && event
                    .time_seconds
                    .is_some_and(|time| time >= trigger_time - 2.5 && time <= response_time + 1.0)
        });
        grouped
            .entry(responder.to_string())
            .or_default()
            .push(TreeCounterMatch {
                trigger,
                response,
                tree_created,
                tree_deleted,
                tree_order,
                related_bushwhack,
            });
    }

    let mut candidates = Vec::new();
    for (responder, mut matches) in grouped {
        matches.sort_by_key(|matched| matched.trigger.tick);
        let occurrence_count = matches.len();
        let first_fifteen_occurrence_count = matches
            .iter()
            .filter(|matched| {
                matched.response.time_seconds.unwrap_or(f32::MAX) <= FIRST_FIFTEEN_MINUTES_SECONDS
            })
            .count();
        let source_to_responder_salute_count =
            salute_count_between_heroes(timeline, "npc_dota_hero_hoodwink", &responder);
        let representative_index = matches
            .iter()
            .enumerate()
            .max_by(|(left_index, left), (right_index, right)| {
                tree_counter_base_score(left, occurrence_count, source_to_responder_salute_count)
                    .total_cmp(&tree_counter_base_score(
                        right,
                        occurrence_count,
                        source_to_responder_salute_count,
                    ))
                    .then_with(|| right_index.cmp(left_index))
            })
            .map(|(index, _)| index)
            .unwrap_or_default();

        for (index, matched) in matches.iter().enumerate() {
            let trigger_time = matched.trigger.time_seconds.unwrap_or_default();
            let response_time = matched.response.time_seconds.unwrap_or(trigger_time);
            let response_delay_seconds = response_time - trigger_time;
            let is_representative = occurrence_count > 1 && index == representative_index;
            let first_fifteen_occurrence_index = (response_time <= FIRST_FIFTEEN_MINUTES_SECONDS)
                .then(|| {
                    matches[..=index]
                        .iter()
                        .filter(|candidate| {
                            candidate.response.time_seconds.unwrap_or(f32::MAX)
                                <= FIRST_FIFTEEN_MINUTES_SECONDS
                        })
                        .count()
                });
            let mut score = tree_counter_base_score(
                matched,
                occurrence_count,
                source_to_responder_salute_count,
            );
            if is_representative {
                score += REPEATED_INTERACTION_BONUS;
            }

            let mut reasons = vec![
                format!(
                    "Hoodwink temporary tree created at tick {} and deleted on Quelling Blade tick {}",
                    matched.tree_created.tick, matched.tree_deleted.tick
                ),
                format!(
                    "matching CAST_TARGET_TREE order at tick {} ({response_delay_seconds:.2}s after Acorn Shot)",
                    matched.tree_order.tick
                ),
            ];
            if occurrence_count > 1 {
                reasons.push(format!(
                    "same responder repeated the tree counter {occurrence_count} times"
                ));
            }
            if matched.related_bushwhack {
                reasons.push("nearby Hoodwink Bushwhack cast".to_string());
            }
            if is_representative {
                reasons.push("representative moment from repeated counterplay".to_string());
            }
            if source_to_responder_salute_count > 0 {
                reasons.push(format!(
                    "Hoodwink saluted the responder {source_to_responder_salute_count} times"
                ));
            }

            candidates.push(HighlightCandidate {
                id: String::new(),
                rank: 0,
                kind: "mechanical_counterplay".to_string(),
                title: format!(
                    "Quelling Blade tree counter ({}/{occurrence_count})",
                    index + 1
                ),
                score,
                start_seconds: (trigger_time - TREE_COUNTER_SETUP_SECONDS).max(0.0),
                peak_seconds: response_time,
                end_seconds: (response_time + TREE_COUNTER_RESULT_SECONDS)
                    .min(timeline.replay.playback_time_seconds),
                hero_deaths: 0,
                anchor_tick: matched.response.tick,
                primary_hero: Some(responder.clone()),
                participants: BTreeSet::from([
                    "npc_dota_hero_hoodwink".to_string(),
                    responder.clone(),
                ])
                .into_iter()
                .collect(),
                reasons,
                interaction: Some(InteractionEvidence {
                    pattern_id: "hoodwink_ground_acorn_quelling_blade".to_string(),
                    occurrence_index: index + 1,
                    occurrence_count,
                    trigger_name: "hoodwink_acorn_shot".to_string(),
                    response_name: "item_quelling_blade".to_string(),
                    response_delay_seconds,
                    related_action: matched
                        .related_bushwhack
                        .then(|| "hoodwink_bushwhack".to_string()),
                    verification: Some(InteractionVerification {
                        method: "temp_tree_deleted_on_tree_targeted_quelling_blade".to_string(),
                        tree_entity_handle: matched.tree_created.entity_handle,
                        tree_created_tick: matched.tree_created.tick,
                        tree_deleted_tick: matched.tree_deleted.tick,
                        matched_tree_order_tick: matched.tree_order.tick,
                        first_fifteen_occurrence_index,
                        first_fifteen_occurrence_count,
                        source_to_responder_salute_count,
                    }),
                }),
                kill_sequence: None,
            });
        }
    }
    candidates
}

fn tree_counter_base_score(
    matched: &TreeCounterMatch<'_>,
    occurrence_count: usize,
    salute_count: usize,
) -> f32 {
    let trigger_time = matched.trigger.time_seconds.unwrap_or_default();
    let response_time = matched.response.time_seconds.unwrap_or(trigger_time);
    let response_delay = (response_time - trigger_time).max(0.0);
    let speed_bonus = ((TREE_COUNTER_WINDOW_SECONDS - response_delay)
        / TREE_COUNTER_WINDOW_SECONDS)
        .clamp(0.0, 1.0)
        * 14.0;
    let bushwhack_bonus = if matched.related_bushwhack { 10.0 } else { 0.0 };
    let repetition_bonus = occurrence_count.saturating_sub(1).min(5) as f32 * 3.0;
    let salute_bonus = salute_count.min(3) as f32 * 4.0;
    58.0 + speed_bonus + bushwhack_bonus + repetition_bonus + salute_bonus
}

fn repeated_mechanical_story_priority(candidate: &HighlightCandidate) -> bool {
    candidate.kind == "mechanical_counterplay"
        && candidate.interaction.as_ref().is_some_and(|evidence| {
            evidence.occurrence_count >= 3 && evidence.verification.is_some()
        })
}

fn hero_class_matches(class_name: &str, hero_name: &str) -> bool {
    let normalize = |value: &str| {
        value
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    let class_suffix = class_name
        .strip_prefix("CDOTA_Unit_Hero_")
        .unwrap_or(class_name);
    let hero_suffix = hero_name
        .strip_prefix("npc_dota_hero_")
        .unwrap_or(hero_name);
    normalize(class_suffix) == normalize(hero_suffix)
}

fn salute_count_between_heroes(
    timeline: &TimelineDocument,
    source_hero: &str,
    target_hero: &str,
) -> usize {
    let player_id = |hero: &str| {
        timeline
            .replay
            .players
            .iter()
            .find(|player| player.hero_name == hero)
            .map(|player| i32::from(player.slot))
    };
    let (Some(source_player_id), Some(target_player_id)) =
        (player_id(source_hero), player_id(target_hero))
    else {
        return 0;
    };
    timeline
        .salutes
        .iter()
        .filter(|salute| {
            salute.source_player_id == Some(source_player_id)
                && salute.target_player_id == Some(target_player_id)
        })
        .count()
}

fn heroes_are_opponents(timeline: &TimelineDocument, left: &str, right: &str) -> bool {
    let team_for = |hero: &str| {
        timeline
            .replay
            .players
            .iter()
            .find(|player| player.hero_name == hero)
            .and_then(|player| player.game_team)
    };
    match (team_for(left), team_for(right)) {
        (Some(left_team), Some(right_team)) => left_team != right_team,
        _ => true,
    }
}

fn objective_candidates(
    timeline: &TimelineDocument,
    fight_candidates: &[HighlightCandidate],
) -> Vec<HighlightCandidate> {
    timeline
        .events
        .iter()
        .filter_map(|event| {
            let time = event.time_seconds?;
            if fight_candidates
                .iter()
                .any(|candidate| time >= candidate.start_seconds && time <= candidate.end_seconds)
            {
                return None;
            }

            let (kind, title, score, reason) = match event.event_type.as_str() {
                "DotaCombatlogAegisTaken" => (
                    "roshan_reward",
                    "Aegis secured",
                    35.0,
                    "Aegis taken after Roshan",
                ),
                "DotaCombatlogBuyback" => ("buyback", "Buyback commitment", 22.0, "buyback event"),
                "DotaCombatlogTeamBuildingKill" => (
                    "objective",
                    "Building destroyed",
                    18.0,
                    "team building kill",
                ),
                _ => return None,
            };

            let participants = [event.attacker.as_ref(), event.target.as_ref()]
                .into_iter()
                .flatten()
                .filter(|name| name.starts_with("npc_dota_hero_"))
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();

            Some(HighlightCandidate {
                id: String::new(),
                rank: 0,
                kind: kind.to_string(),
                title: title.to_string(),
                score,
                start_seconds: (time - SETUP_SECONDS).max(0.0),
                peak_seconds: time,
                end_seconds: (time + RESULT_SECONDS).min(timeline.replay.playback_time_seconds),
                hero_deaths: 0,
                anchor_tick: event.tick,
                primary_hero: event
                    .attacker
                    .as_ref()
                    .filter(|name| name.starts_with("npc_dota_hero_"))
                    .cloned(),
                participants,
                reasons: vec![reason.to_string()],
                interaction: None,
                kill_sequence: None,
            })
        })
        .collect()
}

fn title_for(kind: &str, death_count: usize) -> String {
    match kind {
        "first_blood" => "First blood".to_string(),
        "multikill" => format!("Multikill sequence ({death_count} hero deaths)"),
        "team_fight" => format!("Team fight ({death_count} hero deaths)"),
        "trade" => "Two-way trade".to_string(),
        _ => "Hero pickoff".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use d2_highlights_core::{
        ParserIdentity, PlayerSaluteEvent, ReplayMetadata, ReplayPlayer, TemporaryTreeEvent,
        TemporaryTreeState, TreeOrderEvent,
    };

    fn event(time: f32, target_team: u32) -> CombatEvent {
        CombatEvent {
            tick: (time * 30.0) as u32,
            time_seconds: Some(time),
            event_type: "DotaCombatlogDeath".to_string(),
            attacker: Some("npc_dota_hero_axe".to_string()),
            target: Some(format!("npc_dota_hero_target_{target_team}")),
            inflictor: None,
            damage_source: None,
            value: None,
            health: Some(0),
            attacker_team: Some(if target_team == 2 { 3 } else { 2 }),
            target_team: Some(target_team),
            location_x: None,
            location_y: None,
            attacker_is_hero: Some(true),
            target_is_hero: Some(true),
            long_range_kill: Some(false),
            will_reincarnate: Some(false),
            assist_players: Vec::new(),
        }
    }

    fn mechanic_event(
        time: f32,
        event_type: &str,
        attacker: &str,
        target: Option<&str>,
        inflictor: &str,
    ) -> CombatEvent {
        let mut result = event(time, 3);
        result.event_type = event_type.to_string();
        result.attacker = Some(attacker.to_string());
        result.target = target.map(ToOwned::to_owned);
        result.inflictor = Some(inflictor.to_string());
        result.attacker_team = None;
        result.target_team = None;
        result.target_is_hero = target.map(|name| name.starts_with("npc_dota_hero_"));
        result
    }

    fn timeline(events: Vec<CombatEvent>) -> TimelineDocument {
        TimelineDocument {
            schema_version: "1.2".to_string(),
            source_sha256: "abc".to_string(),
            parser: ParserIdentity {
                name: "test".to_string(),
                version: "1".to_string(),
            },
            replay: ReplayMetadata {
                playback_ticks: 30_000,
                playback_time_seconds: 1_000.0,
                game_build: 1,
                match_id: Some(1),
                game_mode: Some(1),
                game_winner: Some(2),
                players: Vec::new(),
            },
            events,
            tree_orders: Vec::new(),
            temporary_trees: Vec::new(),
            salutes: Vec::new(),
        }
    }

    fn hero_kill_event(time: f32, target: &str, inflictor: Option<&str>) -> CombatEvent {
        let mut result = event(time, 3);
        result.attacker = Some("npc_dota_hero_mirana".to_string());
        result.target = Some(target.to_string());
        result.inflictor = inflictor.map(ToOwned::to_owned);
        result.attacker_team = Some(2);
        result.target_team = Some(3);
        result
    }

    fn buyback_event(time: f32, player_slot: u8) -> CombatEvent {
        let mut result = event(time, 3);
        result.event_type = "DotaCombatlogBuyback".to_string();
        result.attacker = None;
        result.target = None;
        result.target_is_hero = None;
        result.value = Some(u32::from(player_slot));
        result
    }

    fn hero_death_event(time: f32, hero: &str, will_reincarnate: bool) -> CombatEvent {
        let mut result = event(time, 3);
        result.target = Some(hero.to_string());
        result.will_reincarnate = Some(will_reincarnate);
        result
    }

    fn add_mirana_player(replay: &mut TimelineDocument) {
        replay.replay.players = vec![ReplayPlayer {
            slot: 0,
            hero_name: "npc_dota_hero_mirana".to_string(),
            player_name: None,
            game_team: Some(3),
            is_fake_client: false,
        }];
    }

    fn add_tree_proof(
        timeline: &mut TimelineDocument,
        trigger_time: f32,
        response_time: f32,
        responder: &str,
        handle: u32,
    ) {
        let trigger_tick = (trigger_time * 30.0) as u32;
        let response_tick = (response_time * 30.0) as u32;
        timeline.temporary_trees.extend([
            TemporaryTreeEvent {
                tick: trigger_tick + 6,
                entity_index: handle % 8192,
                entity_handle: handle,
                state: TemporaryTreeState::Created,
            },
            TemporaryTreeEvent {
                tick: response_tick,
                entity_index: handle % 8192,
                entity_handle: handle,
                state: TemporaryTreeState::Deleted,
            },
        ]);
        timeline.tree_orders.push(TreeOrderEvent {
            tick: response_tick.saturating_sub(2),
            player_entity_index: None,
            unit_entity_indices: Vec::new(),
            unit_class_names: vec![format!(
                "CDOTA_Unit_Hero_{}",
                responder
                    .strip_prefix("npc_dota_hero_")
                    .unwrap_or(responder)
            )],
            target_tree_index: Some(handle as i32),
            ability_entity_index: None,
            sequence_number: None,
        });
    }

    #[test]
    fn clusters_nearby_hero_deaths_into_a_team_fight() {
        let result = detect_highlights(
            &timeline(vec![event(100.0, 2), event(108.0, 3), event(114.0, 2)]),
            10,
            0.0,
        );

        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].kind, "team_fight");
        assert_eq!(result.candidates[0].hero_deaths, 3);
        assert_eq!(result.candidates[0].start_seconds, 88.0);
        assert_eq!(result.candidates[0].end_seconds, 122.0);
    }

    #[test]
    fn separates_fights_outside_the_cluster_gap() {
        let result = detect_highlights(&timeline(vec![event(100.0, 2), event(130.0, 3)]), 10, 0.0);

        assert_eq!(result.candidates.len(), 2);
    }

    #[test]
    fn returns_empty_document_without_hero_deaths() {
        let result = detect_highlights(&timeline(Vec::new()), 10, 0.0);
        assert!(result.candidates.is_empty());
    }

    #[test]
    fn builds_selected_hero_kill_sequences_from_skill_cast_to_death() {
        let mut replay = timeline(vec![
            mechanic_event(
                100.0,
                "DotaCombatlogAbility",
                "npc_dota_hero_mirana",
                None,
                "mirana_arrow",
            ),
            hero_kill_event(104.0, "npc_dota_hero_snapfire", Some("mirana_arrow")),
            mechanic_event(
                112.0,
                "DotaCombatlogAbility",
                "npc_dota_hero_mirana",
                None,
                "mirana_starfall",
            ),
            hero_kill_event(114.0, "npc_dota_hero_ogre_magi", Some("mirana_starfall")),
            mechanic_event(
                150.0,
                "DotaCombatlogAbility",
                "npc_dota_hero_mirana",
                None,
                "mirana_arrow",
            ),
            hero_kill_event(153.0, "npc_dota_hero_tidehunter", Some("mirana_arrow")),
        ]);
        replay.replay.players = vec![ReplayPlayer {
            slot: 0,
            hero_name: "npc_dota_hero_mirana".to_string(),
            player_name: None,
            game_team: Some(2),
            is_fake_client: false,
        }];

        let result = detect_highlights(&replay, 10, 0.0);
        let sequences = result
            .candidates
            .iter()
            .filter(|candidate| candidate.kind == "hero_kill_sequence")
            .collect::<Vec<_>>();

        assert_eq!(sequences.len(), 2);
        assert_eq!(
            sequences[0].primary_hero.as_deref(),
            Some("npc_dota_hero_mirana")
        );
        assert_eq!(sequences[0].hero_deaths, 2);
        assert_eq!(sequences[0].start_seconds, 99.0);
        assert_eq!(sequences[0].peak_seconds, 114.0);
        assert_eq!(sequences[0].end_seconds, 117.0);
        let evidence = sequences[0]
            .kill_sequence
            .as_ref()
            .expect("kill sequence evidence");
        assert_eq!(evidence.sequence_index, 1);
        assert_eq!(evidence.sequence_count, 2);
        assert_eq!(evidence.total_kills, 3);
        assert_eq!(
            evidence.kills[0].setup_action.as_deref(),
            Some("mirana_arrow")
        );
        assert_eq!(
            evidence.kills[1].setup_action.as_deref(),
            Some("mirana_starfall")
        );
    }

    #[test]
    fn creates_standalone_aegis_candidate() {
        let mut aegis = event(500.0, 2);
        aegis.event_type = "DotaCombatlogAegisTaken".to_string();
        aegis.target_is_hero = None;

        let result = detect_highlights(&timeline(vec![aegis]), 10, 18.0);

        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].kind, "roshan_reward");
        assert_eq!(result.candidates[0].score, 35.0);
    }

    #[test]
    fn scores_killstreak_buyback_and_building_inside_fight() {
        let mut killstreak = event(104.0, 2);
        killstreak.event_type = "DotaCombatlogKillstreak".to_string();
        killstreak.target_is_hero = None;

        let mut buyback = event(106.0, 2);
        buyback.event_type = "DotaCombatlogBuyback".to_string();
        buyback.target_is_hero = None;

        let mut building = event(109.0, 2);
        building.event_type = "DotaCombatlogTeamBuildingKill".to_string();
        building.target_is_hero = None;

        let result = detect_highlights(
            &timeline(vec![
                event(100.0, 2),
                killstreak,
                buyback,
                event(108.0, 3),
                building,
            ]),
            10,
            0.0,
        );
        let reasons = &result.candidates[0].reasons;

        assert!(reasons.iter().any(|reason| reason.contains("killstreak")));
        assert!(reasons.iter().any(|reason| reason.contains("buyback")));
        assert!(reasons.iter().any(|reason| reason.contains("building")));
    }

    #[test]
    fn creates_standalone_building_candidate() {
        let mut building = event(700.0, 2);
        building.event_type = "DotaCombatlogTeamBuildingKill".to_string();
        building.target_is_hero = None;

        let result = detect_highlights(&timeline(vec![building]), 10, 18.0);

        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].kind, "objective");
        assert_eq!(result.candidates[0].score, 18.0);
    }

    #[test]
    fn detects_repeated_tree_counterplay_without_hero_deaths() {
        let mut replay = timeline(vec![
            mechanic_event(
                100.0,
                "DotaCombatlogAbility",
                "npc_dota_hero_hoodwink",
                None,
                "hoodwink_acorn_shot",
            ),
            mechanic_event(
                101.0,
                "DotaCombatlogItem",
                "npc_dota_hero_windrunner",
                None,
                "item_quelling_blade",
            ),
            mechanic_event(
                200.0,
                "DotaCombatlogAbility",
                "npc_dota_hero_hoodwink",
                None,
                "hoodwink_acorn_shot",
            ),
            mechanic_event(
                202.0,
                "DotaCombatlogItem",
                "npc_dota_hero_windrunner",
                None,
                "item_quelling_blade",
            ),
        ]);
        add_tree_proof(
            &mut replay,
            100.0,
            101.0,
            "npc_dota_hero_windrunner",
            10_001,
        );
        add_tree_proof(
            &mut replay,
            200.0,
            202.0,
            "npc_dota_hero_windrunner",
            10_002,
        );
        let result = detect_highlights(&replay, 10, 18.0);
        let interactions = result
            .candidates
            .iter()
            .filter(|candidate| candidate.kind == "mechanical_counterplay")
            .collect::<Vec<_>>();

        assert_eq!(interactions.len(), 2);
        assert_eq!(interactions[0].hero_deaths, 0);
        assert_eq!(
            interactions[0].primary_hero.as_deref(),
            Some("npc_dota_hero_windrunner")
        );
        assert_eq!(
            interactions[0]
                .interaction
                .as_ref()
                .map(|evidence| evidence.occurrence_count),
            Some(2)
        );
        assert!(interactions[0].score > 150.0);
        assert_eq!(
            interactions[0]
                .interaction
                .as_ref()
                .and_then(|evidence| evidence.verification.as_ref())
                .map(|verification| verification.method.as_str()),
            Some("temp_tree_deleted_on_tree_targeted_quelling_blade")
        );
    }

    #[test]
    fn ignores_unit_targeted_acorn_shot() {
        let result = detect_highlights(
            &timeline(vec![
                mechanic_event(
                    100.0,
                    "DotaCombatlogAbility",
                    "npc_dota_hero_hoodwink",
                    Some("npc_dota_hero_windrunner"),
                    "hoodwink_acorn_shot",
                ),
                mechanic_event(
                    101.0,
                    "DotaCombatlogItem",
                    "npc_dota_hero_windrunner",
                    None,
                    "item_quelling_blade",
                ),
            ]),
            10,
            18.0,
        );

        assert!(
            result
                .candidates
                .iter()
                .all(|candidate| candidate.kind != "mechanical_counterplay")
        );
    }

    #[test]
    fn assigns_the_planted_tree_to_the_hero_whose_order_deletes_it() {
        let mut replay = timeline(vec![
            mechanic_event(
                100.0,
                "DotaCombatlogAbility",
                "npc_dota_hero_hoodwink",
                None,
                "hoodwink_acorn_shot",
            ),
            mechanic_event(
                100.5,
                "DotaCombatlogItem",
                "npc_dota_hero_skeleton_king",
                None,
                "item_quelling_blade",
            ),
            mechanic_event(
                100.8,
                "DotaCombatlogItem",
                "npc_dota_hero_windrunner",
                None,
                "item_quelling_blade",
            ),
        ]);
        add_tree_proof(
            &mut replay,
            100.0,
            100.8,
            "npc_dota_hero_windrunner",
            10_003,
        );
        let result = detect_highlights(&replay, 10, 18.0);
        let interaction = result
            .candidates
            .iter()
            .find(|candidate| candidate.kind == "mechanical_counterplay")
            .expect("tree counter candidate");

        assert_eq!(
            interaction.primary_hero.as_deref(),
            Some("npc_dota_hero_windrunner")
        );
        assert_eq!(
            result
                .candidates
                .iter()
                .filter(|candidate| candidate.kind == "mechanical_counterplay")
                .count(),
            1
        );
    }

    #[test]
    fn preserves_four_early_verified_cuts_and_three_hoodwink_salutes() {
        let moments = [
            (542.5, 543.6),
            (565.6, 566.5),
            (594.0, 595.0),
            (659.8, 661.6),
        ];
        let mut events = Vec::new();
        for (trigger, response) in moments {
            events.push(mechanic_event(
                trigger,
                "DotaCombatlogAbility",
                "npc_dota_hero_hoodwink",
                None,
                "hoodwink_acorn_shot",
            ));
            events.push(mechanic_event(
                response,
                "DotaCombatlogItem",
                "npc_dota_hero_windrunner",
                None,
                "item_quelling_blade",
            ));
        }
        let mut replay = timeline(events);
        replay.replay.players = vec![
            ReplayPlayer {
                slot: 0,
                hero_name: "npc_dota_hero_windrunner".to_string(),
                player_name: None,
                game_team: Some(2),
                is_fake_client: false,
            },
            ReplayPlayer {
                slot: 5,
                hero_name: "npc_dota_hero_hoodwink".to_string(),
                player_name: None,
                game_team: Some(3),
                is_fake_client: false,
            },
        ];
        for (index, (trigger, response)) in moments.into_iter().enumerate() {
            add_tree_proof(
                &mut replay,
                trigger,
                response,
                "npc_dota_hero_windrunner",
                20_000 + index as u32,
            );
        }
        replay.salutes = [16_849, 25_425, 27_319]
            .into_iter()
            .map(|tick| PlayerSaluteEvent {
                tick,
                source_player_id: Some(5),
                target_player_id: Some(0),
                tip_amount: Some(50),
                event_id: Some(19),
                num_recent_tips: Some(0),
            })
            .collect();

        let result = detect_highlights(&replay, 20, 18.0);
        let interactions = result
            .candidates
            .iter()
            .filter(|candidate| candidate.kind == "mechanical_counterplay")
            .collect::<Vec<_>>();

        assert_eq!(interactions.len(), 4);
        for candidate in interactions {
            let verification = candidate
                .interaction
                .as_ref()
                .and_then(|evidence| evidence.verification.as_ref())
                .expect("verified interaction");
            assert_eq!(verification.first_fifteen_occurrence_count, 4);
            assert_eq!(verification.source_to_responder_salute_count, 3);
        }
    }

    #[test]
    fn detects_same_hero_death_within_ninety_seconds_of_buyback() {
        let mut replay = timeline(vec![
            buyback_event(100.0, 0),
            hero_death_event(112.0, "npc_dota_hero_mirana", false),
        ]);
        add_mirana_player(&mut replay);

        let result = detect_highlights(&replay, 10, 0.0);
        let mistakes = result
            .candidates
            .iter()
            .filter(|candidate| candidate.kind == "buyback_dieback")
            .collect::<Vec<_>>();

        assert_eq!(mistakes.len(), 1);
        assert_eq!(
            mistakes[0].primary_hero.as_deref(),
            Some("npc_dota_hero_mirana")
        );
        assert_eq!(mistakes[0].peak_seconds, 112.0);
    }

    #[test]
    fn ignores_death_more_than_ninety_seconds_after_buyback() {
        let mut replay = timeline(vec![
            buyback_event(100.0, 0),
            hero_death_event(190.1, "npc_dota_hero_mirana", false),
        ]);
        add_mirana_player(&mut replay);

        let result = detect_highlights(&replay, 10, 0.0);

        assert!(
            result
                .candidates
                .iter()
                .all(|candidate| candidate.kind != "buyback_dieback")
        );
    }

    #[test]
    fn ignores_reincarnation_death_after_buyback() {
        let mut replay = timeline(vec![
            buyback_event(100.0, 0),
            hero_death_event(112.0, "npc_dota_hero_mirana", true),
        ]);
        add_mirana_player(&mut replay);

        let result = detect_highlights(&replay, 10, 0.0);

        assert!(
            result
                .candidates
                .iter()
                .all(|candidate| candidate.kind != "buyback_dieback")
        );
    }
}
