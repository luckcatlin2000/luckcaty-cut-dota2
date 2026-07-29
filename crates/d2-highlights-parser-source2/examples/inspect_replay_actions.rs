use source2_demo::prelude::*;
use source2_demo::proto::{
    CDotaUserMsgChatEvent, CDotaUserMsgChatMessage, CDotaUserMsgSalutePlayer,
    CDotaUserMsgSpectatorPlayerUnitOrders, CDotaUserMsgTipAlert, EDotaUserMessages,
};
use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::thread;

const PARSER_STACK_BYTES: usize = 64 * 1024 * 1024;

#[derive(Default)]
struct ReplayActionInspector {
    message_counts: BTreeMap<String, usize>,
    game_event_counts: BTreeMap<String, usize>,
    tree_orders: Vec<String>,
    chat_events: Vec<String>,
    chat_messages: Vec<String>,
    salutes: Vec<String>,
    tree_entities: Vec<String>,
    tip_messages: Vec<String>,
    tip_game_events: Vec<String>,
}

impl Observer for ReplayActionInspector {
    fn interests(&self) -> Interests {
        Interests::DOTA_USER_MESSAGE
            | Interests::BASE_GAME_EVENT
            | Interests::ENTITY_EVENTS
            | Interests::ENTITY_STATE
    }

    fn on_dota_user_message(
        &mut self,
        ctx: &Context,
        msg_type: EDotaUserMessages,
        payload: &[u8],
    ) -> ObserverResult {
        *self
            .message_counts
            .entry(format!("{msg_type:?}"))
            .or_default() += 1;

        match msg_type {
            EDotaUserMessages::DotaUmSpectatorPlayerUnitOrders => {
                let message = CDotaUserMsgSpectatorPlayerUnitOrders::decode(payload)?;
                if message.order_type == Some(7) {
                    let unit_classes = message
                        .units
                        .iter()
                        .filter_map(|index| {
                            ctx.entities()
                                .get_by_index(*index as usize)
                                .ok()
                                .map(|entity| entity.class().name().to_string())
                        })
                        .collect::<Vec<_>>();
                    let position = message
                        .position
                        .map(|value| {
                            format!(
                                "({:.1},{:.1},{:.1})",
                                value.x.unwrap_or_default(),
                                value.y.unwrap_or_default(),
                                value.z.unwrap_or_default()
                            )
                        })
                        .unwrap_or_else(|| "(none)".to_string());
                    self.tree_orders.push(format!(
                        "tick={} entindex={:?} units={:?} classes={:?} target_index={:?} ability_id={:?} position={} sequence={:?}",
                        ctx.tick(),
                        message.entindex,
                        message.units,
                        unit_classes,
                        message.target_index,
                        message.ability_id,
                        position,
                        message.sequence_number
                    ));
                }
            }
            EDotaUserMessages::DotaUmTipAlert => {
                let message = CDotaUserMsgTipAlert::decode(payload)?;
                self.tip_messages.push(format!(
                    "tick={} player_id={:?} text={:?}",
                    ctx.tick(),
                    message.player_id,
                    message.tip_text
                ));
            }
            EDotaUserMessages::DotaUmChatEvent => {
                let message = CDotaUserMsgChatEvent::decode(payload)?;
                self.chat_events.push(format!(
                    "tick={} type={:?} value={:?} value2={:?} value3={:?} players={:?} time={:?}",
                    ctx.tick(),
                    message.r#type,
                    message.value,
                    message.value2,
                    message.value3,
                    [
                        message.playerid_1,
                        message.playerid_2,
                        message.playerid_3,
                        message.playerid_4,
                        message.playerid_5,
                        message.playerid_6,
                    ],
                    message.time
                ));
            }
            EDotaUserMessages::DotaUmChatMessage => {
                let message = CDotaUserMsgChatMessage::decode(payload)?;
                self.chat_messages.push(format!(
                    "tick={} player_id={:?} channel={:?} text={:?}",
                    ctx.tick(),
                    message.source_player_id,
                    message.channel_type,
                    message.message_text
                ));
            }
            EDotaUserMessages::DotaUmSalutePlayer => {
                let message = CDotaUserMsgSalutePlayer::decode(payload)?;
                self.salutes.push(format!(
                    "tick={} source={:?} target={:?} amount={:?} event_id={:?} recent={:?} style={:?}",
                    ctx.tick(),
                    message.source_player_id,
                    message.target_player_id,
                    message.tip_amount,
                    message.event_id,
                    message.num_recent_tips,
                    message.custom_tip_style
                ));
            }
            _ => {}
        }

        Ok(())
    }

    fn on_game_event(&mut self, ctx: &Context, event: &GameEvent) -> ObserverResult {
        let name = event.name().to_string();
        *self.game_event_counts.entry(name.clone()).or_default() += 1;

        let lower = name.to_ascii_lowercase();
        if lower.contains("tip") || lower.contains("commend") {
            let values = event
                .iter()
                .map(|(key, value)| format!("{key}={value:?}"))
                .collect::<Vec<_>>()
                .join(" ");
            self.tip_game_events
                .push(format!("tick={} event={} {}", ctx.tick(), name, values));
        }

        Ok(())
    }

    fn on_entity(&mut self, ctx: &Context, event: EntityEvents, entity: &Entity) -> ObserverResult {
        let class_name = entity.class().name();
        if class_name.to_ascii_lowercase().contains("tree") {
            self.tree_entities.push(format!(
                "tick={} event={event:?} index={} handle={} class={class_name}",
                ctx.tick(),
                entity.index(),
                entity.handle()
            ));
        }
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let path = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: inspect_replay_actions <replay.dem>")?;
    thread::Builder::new()
        .name("inspect-replay-actions".to_string())
        .stack_size(PARSER_STACK_BYTES)
        .spawn(move || inspect(path))
        .map_err(|error| format!("unable to start parser worker: {error}"))?
        .join()
        .map_err(|_| "parser worker panicked")?
}

fn inspect(path: PathBuf) -> Result<(), Box<dyn Error + Send + Sync>> {
    let file = File::open(&path)?;
    let mut parser = Parser::from_reader(BufReader::new(file))?;
    let observer = parser.register_observer::<ReplayActionInspector>();
    parser.run_to_end()?;

    let result = observer.borrow();
    println!("tree_orders={}", result.tree_orders.len());
    for order in &result.tree_orders {
        println!("TREE {order}");
    }

    println!("chat_events={}", result.chat_events.len());
    for event in &result.chat_events {
        println!("CHAT_EVENT {event}");
    }

    println!("chat_messages={}", result.chat_messages.len());
    for message in &result.chat_messages {
        println!("CHAT_MESSAGE {message}");
    }

    println!("salutes={}", result.salutes.len());
    for salute in &result.salutes {
        println!("SALUTE {salute}");
    }

    println!("tree_entities={}", result.tree_entities.len());
    for entity in &result.tree_entities {
        println!("TREE_ENTITY {entity}");
    }

    println!("tip_messages={}", result.tip_messages.len());
    for tip in &result.tip_messages {
        println!("TIP_MESSAGE {tip}");
    }

    println!("tip_game_events={}", result.tip_game_events.len());
    for event in &result.tip_game_events {
        println!("TIP_EVENT {event}");
    }

    println!(
        "spectator_order_messages={}",
        result
            .message_counts
            .get("DotaUmSpectatorPlayerUnitOrders")
            .copied()
            .unwrap_or_default()
    );
    println!(
        "tip_alert_messages={}",
        result
            .message_counts
            .get("DotaUmTipAlert")
            .copied()
            .unwrap_or_default()
    );
    println!("message_counts:");
    for (name, count) in &result.message_counts {
        println!("MESSAGE_COUNT {name}={count}");
    }

    Ok(())
}
