use d2_highlights_core::{
    CombatEvent, CombatEventContext, ParserIdentity, PlayerSaluteEvent, ReplayMetadata,
    ReplayPlayer, ReplayPlayerState, ReplayStateSnapshot, TIMELINE_SCHEMA_VERSION,
    TemporaryTreeEvent, TemporaryTreeState, TimelineDocument, TreeOrderEvent,
};
use source2_demo::prelude::*;
use source2_demo::proto::{CDotaUserMsgSalutePlayer, CDotaUserMsgSpectatorPlayerUnitOrders};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::thread;
use thiserror::Error;

pub const SOURCE2_DEMO_VERSION: &str = "0.5.8";
/// Bump when adapter semantics change without changing the timeline schema.
pub const PARSER_ADAPTER_CACHE_VERSION: &str = "2";
pub const COMBAT_ONLY_PARSER_NAME: &str = "source2-demo/combat-log-only";
const PARSER_STACK_BYTES: usize = 64 * 1024 * 1024;
const DIRECTOR_SNAPSHOT_INTERVAL_SECONDS: f64 = 30.0;

#[derive(Debug, Error)]
pub enum ParserAdapterError {
    #[error("unable to open DEM {path}: {source}")]
    Open {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("source2-demo failed: {0}")]
    Parse(#[from] source2_demo::error::ParserError),
    #[error("unable to start the DEM parser worker: {0}")]
    ThreadSpawn(std::io::Error),
    #[error("the DEM parser worker panicked")]
    WorkerPanic,
}

#[derive(Clone, Default)]
struct LivePlayerMetrics {
    net_worth: Option<u32>,
    last_hits: Option<u32>,
    gpm: Option<u32>,
    xpm: Option<u32>,
    observer_wards_placed: Option<u32>,
}

struct CombatObserver {
    events: Vec<CombatEvent>,
    tree_orders: Vec<TreeOrderEvent>,
    temporary_trees: Vec<TemporaryTreeEvent>,
    salutes: Vec<PlayerSaluteEvent>,
    state_snapshots: Vec<ReplayStateSnapshot>,
    heroes_by_roster: Vec<Option<String>>,
    latest_player_metrics: HashMap<String, LivePlayerMetrics>,
    seconds_per_tick: f64,
    snapshot_interval_ticks: u32,
    last_snapshot_tick: Option<u32>,
    last_clock: Option<(u32, f32)>,
    latest_at_night_time: Option<bool>,
}

impl CombatObserver {
    fn new(seconds_per_tick: f64, heroes_by_roster: Vec<Option<String>>) -> Self {
        let snapshot_interval_ticks = (DIRECTOR_SNAPSHOT_INTERVAL_SECONDS / seconds_per_tick)
            .round()
            .clamp(1.0, f64::from(u32::MAX)) as u32;
        Self {
            events: Vec::new(),
            tree_orders: Vec::new(),
            temporary_trees: Vec::new(),
            salutes: Vec::new(),
            state_snapshots: Vec::new(),
            heroes_by_roster,
            latest_player_metrics: HashMap::new(),
            seconds_per_tick,
            snapshot_interval_ticks,
            last_snapshot_tick: None,
            last_clock: None,
            latest_at_night_time: None,
        }
    }

    fn estimated_time_seconds(&self, tick: u32) -> Option<f32> {
        self.last_clock.map(|(clock_tick, clock_time)| {
            clock_time
                + (f64::from(tick) - f64::from(clock_tick)) as f32 * self.seconds_per_tick as f32
        })
    }

    fn capture_snapshot(&mut self, ctx: &Context, force: bool) {
        let tick = normalized_tick(ctx.tick());
        if !force
            && self
                .last_snapshot_tick
                .is_some_and(|last| tick.saturating_sub(last) < self.snapshot_interval_ticks)
        {
            return;
        }
        let Some(resource) = ctx
            .entities()
            .iter()
            .find(|entity| entity.class().name() == "CDOTA_PlayerResource")
        else {
            return;
        };

        let mut players = Vec::new();
        for (roster_index, hero_name) in self.heroes_by_roster.iter().enumerate() {
            let Some(hero_name) = hero_name.as_ref() else {
                continue;
            };
            let hero = selected_hero_handle(resource, roster_index)
                .and_then(|handle| ctx.entities().get_by_handle(handle as usize).ok());
            let level = hero
                .and_then(|entity| entity_u32_field(entity, "m_iCurrentLevel"))
                .or_else(|| {
                    entity_u32_field(
                        resource,
                        &format!("m_vecPlayerTeamData.{roster_index:04}.m_iLevel"),
                    )
                });
            let items = hero.map_or_else(Vec::new, |entity| inventory_items(ctx, entity));
            let metrics = self
                .latest_player_metrics
                .get(hero_name)
                .cloned()
                .unwrap_or_default();
            players.push(ReplayPlayerState {
                roster_index: u8::try_from(roster_index).unwrap_or(u8::MAX),
                hero_name: hero_name.clone(),
                level,
                net_worth: metrics.net_worth,
                last_hits: metrics.last_hits,
                gpm: metrics.gpm,
                xpm: metrics.xpm,
                observer_wards_placed: metrics.observer_wards_placed,
                items,
            });
        }
        if players.is_empty() {
            return;
        }

        let snapshot = ReplayStateSnapshot {
            tick,
            time_seconds: self.estimated_time_seconds(tick),
            at_night_time: self.latest_at_night_time,
            players,
        };
        if self
            .state_snapshots
            .last()
            .is_some_and(|existing| existing.tick == tick)
        {
            if let Some(existing) = self.state_snapshots.last_mut() {
                *existing = snapshot;
            }
        } else {
            self.state_snapshots.push(snapshot);
        }
        self.last_snapshot_tick = Some(tick);
    }
}

#[observer]
#[uses_entities]
#[uses_combat_log]
impl CombatObserver {
    #[on_combat_log]
    fn on_combat(&mut self, ctx: &Context, entry: &CombatLogEntry) -> ObserverResult {
        let event_type = format!("{:?}", entry.r#type());
        let time_seconds = entry.timestamp().ok();
        if let Some(time_seconds) = time_seconds {
            self.last_clock = Some((normalized_tick(ctx.tick()), time_seconds));
        }
        let at_night_time = entry.at_night_time().ok();
        let context = combat_event_context(entry, &event_type);
        if let Some(at_night_time) = at_night_time {
            self.latest_at_night_time = Some(at_night_time);
        }
        if event_type == "DotaCombatlogPlayerstats"
            && let Some(hero_name) = context
                .value_name
                .as_deref()
                .filter(|name| name.starts_with("npc_dota_hero_"))
        {
            self.latest_player_metrics.insert(
                hero_name.to_string(),
                LivePlayerMetrics {
                    net_worth: context.net_worth,
                    last_hits: context.last_hits,
                    gpm: context.gpm,
                    xpm: context.xpm,
                    observer_wards_placed: context.observer_wards_placed,
                },
            );
        }
        self.events.push(CombatEvent {
            tick: ctx.tick(),
            time_seconds,
            event_type,
            attacker: entry.attacker_name().ok().map(ToOwned::to_owned),
            target: entry.target_name().ok().map(ToOwned::to_owned),
            inflictor: entry.inflictor_name().ok().map(ToOwned::to_owned),
            damage_source: entry.damage_source_name().ok().map(ToOwned::to_owned),
            value: entry.value().ok(),
            health: entry.health().ok(),
            attacker_team: entry.attacker_team().ok(),
            target_team: entry.target_team().ok(),
            location_x: entry.location_x().ok(),
            location_y: entry.location_y().ok(),
            attacker_is_hero: entry.is_attacker_hero().ok(),
            target_is_hero: entry.is_target_hero().ok(),
            long_range_kill: entry.long_range_kill().ok(),
            will_reincarnate: entry.will_reincarnate().ok(),
            assist_players: entry.assist_players().to_vec(),
            context,
        });
        Ok(())
    }

    #[on_tick_end]
    fn on_tick_end(&mut self, ctx: &Context) -> ObserverResult {
        self.capture_snapshot(ctx, false);
        Ok(())
    }

    #[on_stop]
    fn on_stop(&mut self, ctx: &Context) -> ObserverResult {
        self.capture_snapshot(ctx, true);
        Ok(())
    }

    #[on_message]
    fn on_tree_order(
        &mut self,
        ctx: &Context,
        message: CDotaUserMsgSpectatorPlayerUnitOrders,
    ) -> ObserverResult {
        if message.order_type != Some(7) {
            return Ok(());
        }
        let unit_class_names = message
            .units
            .iter()
            .filter_map(|index| {
                usize::try_from(*index)
                    .ok()
                    .and_then(|index| ctx.entities().get_by_index(index).ok())
                    .map(|entity| entity.class().name().to_string())
            })
            .collect();
        self.tree_orders.push(TreeOrderEvent {
            tick: ctx.tick(),
            player_entity_index: message.entindex,
            unit_entity_indices: message.units,
            unit_class_names,
            target_tree_index: message.target_index,
            ability_entity_index: message.ability_id,
            sequence_number: message.sequence_number,
        });
        Ok(())
    }

    #[on_message]
    fn on_salute(&mut self, ctx: &Context, message: CDotaUserMsgSalutePlayer) -> ObserverResult {
        self.salutes.push(PlayerSaluteEvent {
            tick: ctx.tick(),
            source_player_id: message.source_player_id,
            target_player_id: message.target_player_id,
            tip_amount: message.tip_amount,
            event_id: message.event_id,
            num_recent_tips: message.num_recent_tips,
        });
        Ok(())
    }

    #[on_entity("CDOTA_TempTree")]
    fn on_temporary_tree(
        &mut self,
        ctx: &Context,
        event: EntityEvents,
        entity: &Entity,
    ) -> ObserverResult {
        let state = match event {
            EntityEvents::Created => TemporaryTreeState::Created,
            EntityEvents::Deleted => TemporaryTreeState::Deleted,
            EntityEvents::Updated => return Ok(()),
        };
        self.temporary_trees.push(TemporaryTreeEvent {
            tick: ctx.tick(),
            entity_index: entity.index(),
            entity_handle: entity.handle(),
            state,
        });
        Ok(())
    }
}

struct CombatOnlyObserver(CombatObserver);
impl Observer for CombatOnlyObserver {
    fn interests(&self) -> Interests {
        Interests::COMBAT_LOG_ENTRIES | Interests::STRING_TABLE_STATE
    }
    fn on_combat_log(&mut self, ctx: &Context, entry: &CombatLogEntry) -> ObserverResult {
        Observer::on_combat_log(&mut self.0, ctx, entry)
    }
}

pub fn parse_combat_timeline(
    path: &Path,
    source_sha256: &str,
) -> Result<TimelineDocument, ParserAdapterError> {
    match parse_worker(path, source_sha256, true) {
        Err(ParserAdapterError::WorkerPanic | ParserAdapterError::Parse(_)) => {
            parse_worker(path, source_sha256, false)
        }
        result => result,
    }
}

/// Recorder path: retain the same combat-log decoding and replay metadata, but
/// do not register entity, tree-order, salute or director-state observers.
pub fn parse_highlight_timeline(
    path: &Path,
    source_sha256: &str,
) -> Result<TimelineDocument, ParserAdapterError> {
    parse_worker(path, source_sha256, false)
}

fn parse_worker(
    path: &Path,
    source_sha256: &str,
    entities: bool,
) -> Result<TimelineDocument, ParserAdapterError> {
    let owned_path = path.to_path_buf();
    let owned_sha256 = source_sha256.to_string();
    thread::Builder::new()
        .name("d2-dem-parser".to_string())
        .stack_size(PARSER_STACK_BYTES)
        .spawn(move || parse_combat_timeline_inner(&owned_path, &owned_sha256, entities))
        .map_err(ParserAdapterError::ThreadSpawn)?
        .join()
        .map_err(|_| ParserAdapterError::WorkerPanic)?
}

fn parse_combat_timeline_inner(
    path: &Path,
    source_sha256: &str,
    entities: bool,
) -> Result<TimelineDocument, ParserAdapterError> {
    let file = File::open(path).map_err(|source| ParserAdapterError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let mut parser = Parser::from_reader(BufReader::new(file))?;
    let replay_info = parser.replay_info().clone();
    let dota_info = replay_info
        .game_info
        .as_ref()
        .and_then(|game_info| game_info.dota.as_ref());
    let players: Vec<ReplayPlayer> = dota_info
        .map(|info| {
            info.player_info
                .iter()
                .enumerate()
                .filter_map(|(slot, player)| {
                    let hero_name = player.hero_name.as_deref()?.trim();
                    if hero_name.is_empty() {
                        return None;
                    }
                    Some(ReplayPlayer {
                        slot: u8::try_from(slot).ok()?,
                        hero_name: hero_name.to_string(),
                        player_name: decode_player_name(player.player_name.as_deref()),
                        game_team: player.game_team,
                        is_fake_client: player.is_fake_client.unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut heroes_by_roster = vec![None; 10];
    for player in &players {
        if let Some(hero) = heroes_by_roster.get_mut(usize::from(player.slot)) {
            *hero = Some(player.hero_name.clone());
        }
    }
    let seconds_per_tick = if replay_info.playback_ticks() > 0 {
        f64::from(replay_info.playback_time()) / f64::from(replay_info.playback_ticks())
    } else {
        1.0 / 30.0
    };
    let observer = CombatObserver::new(seconds_per_tick, heroes_by_roster);
    let (events, tree_orders, temporary_trees, salutes, state_snapshots) = if entities {
        let observer = parser.add_observer(observer);
        parser.run_to_end()?;
        let o = observer.borrow();
        (
            o.events.clone(),
            o.tree_orders.clone(),
            o.temporary_trees.clone(),
            o.salutes.clone(),
            o.state_snapshots.clone(),
        )
    } else {
        let observer = parser.add_observer(CombatOnlyObserver(observer));
        parser.run_to_end()?;
        let o = observer.borrow();
        (
            o.0.events.clone(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
    };
    let game_build = parser.context().game_build();

    Ok(TimelineDocument {
        schema_version: TIMELINE_SCHEMA_VERSION.to_string(),
        source_sha256: source_sha256.to_string(),
        parser: ParserIdentity {
            name: if entities {
                "source2-demo"
            } else {
                COMBAT_ONLY_PARSER_NAME
            }
            .to_string(),
            version: SOURCE2_DEMO_VERSION.to_string(),
        },
        replay: ReplayMetadata {
            playback_ticks: replay_info.playback_ticks(),
            playback_time_seconds: replay_info.playback_time(),
            game_build,
            match_id: dota_info.and_then(|info| info.match_id),
            game_mode: dota_info.and_then(|info| info.game_mode),
            game_winner: dota_info.and_then(|info| info.game_winner),
            players,
        },
        events,
        tree_orders,
        temporary_trees,
        salutes,
        state_snapshots,
    })
}

fn normalized_tick(tick: u32) -> u32 {
    if tick == u32::MAX { 0 } else { tick }
}

fn combat_event_context(entry: &CombatLogEntry<'_>, event_type: &str) -> CombatEventContext {
    match event_type {
        "DotaCombatlogAbility" => CombatEventContext {
            ability_level: entry.ability_level().ok(),
            is_ultimate_ability: entry.is_ultimate_ability().ok(),
            at_night_time: entry.at_night_time().ok(),
            visible_to_radiant: entry.is_visible_radiant().ok(),
            visible_to_dire: entry.is_visible_dire().ok(),
            ..Default::default()
        },
        "DotaCombatlogModifierAdd" => CombatEventContext {
            stun_duration_seconds: entry.stun_duration().ok(),
            slow_duration_seconds: entry.slow_duration().ok(),
            modifier_duration_seconds: entry.modifier_duration().ok(),
            ..Default::default()
        },
        "DotaCombatlogPlayerstats" => CombatEventContext {
            value_name: entry.value_name().ok().map(ToOwned::to_owned),
            net_worth: entry.networth().ok(),
            last_hits: entry.last_hits().ok(),
            gpm: entry.gpm().ok(),
            xpm: entry.xpm().ok(),
            observer_wards_placed: entry.obs_wards_placed().ok(),
            ..Default::default()
        },
        _ => CombatEventContext::default(),
    }
}

fn selected_hero_handle(resource: &Entity, roster_index: usize) -> Option<u32> {
    let field_name = format!("m_vecPlayerTeamData.{roster_index:04}.m_hSelectedHero");
    entity_u32_field(resource, &field_name).filter(|handle| *handle != 0 && *handle != 0x00ff_ffff)
}

fn entity_u32_field(entity: &Entity, field_name: &str) -> Option<u32> {
    entity
        .fields()
        .into_iter()
        .find(|field| field.name == field_name || field.name.ends_with(field_name))
        .and_then(|field| field.value)
        .and_then(field_value_u32)
}

fn field_value_u32(value: &FieldValue) -> Option<u32> {
    match value {
        FieldValue::Unsigned8(value) => Some(u32::from(*value)),
        FieldValue::Unsigned16(value) => Some(u32::from(*value)),
        FieldValue::Unsigned32(value) => Some(*value),
        FieldValue::Unsigned64(value) => u32::try_from(*value).ok(),
        FieldValue::Signed8(value) => u32::try_from(*value).ok(),
        FieldValue::Signed16(value) => u32::try_from(*value).ok(),
        FieldValue::Signed32(value) => u32::try_from(*value).ok(),
        FieldValue::Signed64(value) => u32::try_from(*value).ok(),
        _ => None,
    }
}

fn inventory_items(ctx: &Context, hero: &Entity) -> Vec<String> {
    let mut items = hero
        .fields()
        .into_iter()
        .filter_map(|field| {
            let slot = field
                .name
                .strip_prefix("m_Inventory.m_hItems.")?
                .parse::<u32>()
                .ok()?;
            if slot > 9 {
                return None;
            }
            let handle = field.value.and_then(field_value_u32)?;
            if handle == 0 || handle == 0x00ff_ffff {
                return None;
            }
            let item = ctx.entities().get_by_handle(handle as usize).ok()?;
            item.class()
                .name()
                .starts_with("CDOTA_Item_")
                .then(|| (slot, item_internal_name(item.class().name())))
        })
        .collect::<Vec<_>>();
    items.sort_by_key(|(slot, _)| *slot);
    items.into_iter().map(|(_, item)| item).collect()
}

fn item_internal_name(class_name: &str) -> String {
    format!(
        "item_{}",
        snake_case(class_name.strip_prefix("CDOTA_Item_").unwrap_or(class_name))
    )
}

fn snake_case(value: &str) -> String {
    let mut result = String::new();
    for (index, character) in value.chars().enumerate() {
        if character.is_ascii_uppercase() && index > 0 && !result.ends_with('_') {
            result.push('_');
        }
        result.push(character.to_ascii_lowercase());
    }
    result
}

fn decode_player_name(bytes: Option<&[u8]>) -> Option<String> {
    let decoded = String::from_utf8_lossy(bytes?);
    let trimmed =
        decoded.trim_matches(|character: char| character == '\0' || character.is_whitespace());
    let cleaned = trimmed
        .chars()
        .filter(|character| !character.is_control())
        .take(64)
        .collect::<String>();
    (!cleaned.is_empty()).then_some(cleaned)
}

#[cfg(test)]
mod tests {
    use super::decode_player_name;

    #[test]
    fn decodes_utf8_player_names_and_trims_demo_padding() {
        assert_eq!(
            decode_player_name(Some("  测试玩家 A\0\0".as_bytes())).as_deref(),
            Some("测试玩家 A")
        );
    }

    #[test]
    fn rejects_empty_names_and_removes_control_characters() {
        assert_eq!(decode_player_name(Some(b" \0\r\n ")), None);
        assert_eq!(
            decode_player_name(Some(b"Luck\tCat")).as_deref(),
            Some("LuckCat")
        );
    }
}
