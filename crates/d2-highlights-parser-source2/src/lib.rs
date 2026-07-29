use d2_highlights_core::{
    CombatEvent, ParserIdentity, PlayerSaluteEvent, ReplayMetadata, ReplayPlayer,
    TIMELINE_SCHEMA_VERSION, TemporaryTreeEvent, TemporaryTreeState, TimelineDocument,
    TreeOrderEvent,
};
use source2_demo::prelude::*;
use source2_demo::proto::{CDotaUserMsgSalutePlayer, CDotaUserMsgSpectatorPlayerUnitOrders};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::thread;
use thiserror::Error;

const SOURCE2_DEMO_VERSION: &str = "0.5.8";
const PARSER_STACK_BYTES: usize = 64 * 1024 * 1024;

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

#[derive(Default)]
struct CombatObserver {
    events: Vec<CombatEvent>,
    tree_orders: Vec<TreeOrderEvent>,
    temporary_trees: Vec<TemporaryTreeEvent>,
    salutes: Vec<PlayerSaluteEvent>,
}

#[observer]
#[uses_entities]
#[uses_combat_log]
impl CombatObserver {
    #[on_combat_log]
    fn on_combat(&mut self, ctx: &Context, entry: &CombatLogEntry) -> ObserverResult {
        self.events.push(CombatEvent {
            tick: ctx.tick(),
            time_seconds: entry.timestamp().ok(),
            event_type: format!("{:?}", entry.r#type()),
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
        });
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

pub fn parse_combat_timeline(
    path: &Path,
    source_sha256: &str,
) -> Result<TimelineDocument, ParserAdapterError> {
    let owned_path = path.to_path_buf();
    let owned_sha256 = source_sha256.to_string();
    thread::Builder::new()
        .name("d2-dem-parser".to_string())
        .stack_size(PARSER_STACK_BYTES)
        .spawn(move || parse_combat_timeline_inner(&owned_path, &owned_sha256))
        .map_err(ParserAdapterError::ThreadSpawn)?
        .join()
        .map_err(|_| ParserAdapterError::WorkerPanic)?
}

fn parse_combat_timeline_inner(
    path: &Path,
    source_sha256: &str,
) -> Result<TimelineDocument, ParserAdapterError> {
    let file = File::open(path).map_err(|source| ParserAdapterError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let mut parser = Parser::from_reader(BufReader::new(file))?;
    let replay_info = parser.replay_info().clone();
    let observer = parser.register_observer::<CombatObserver>();
    parser.run_to_end()?;

    let game_build = parser.context().game_build();
    let observer = observer.borrow();
    let events = observer.events.clone();
    let tree_orders = observer.tree_orders.clone();
    let temporary_trees = observer.temporary_trees.clone();
    let salutes = observer.salutes.clone();
    let dota_info = replay_info
        .game_info
        .as_ref()
        .and_then(|game_info| game_info.dota.as_ref());
    let players = dota_info
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
                        game_team: player.game_team,
                        is_fake_client: player.is_fake_client.unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(TimelineDocument {
        schema_version: TIMELINE_SCHEMA_VERSION.to_string(),
        source_sha256: source_sha256.to_string(),
        parser: ParserIdentity {
            name: "source2-demo".to_string(),
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
    })
}
