use tracing::{info, debug, trace, warn};
use error_stack::{Report, ResultExt};
use std::{
    collections::HashMap,
    fmt::Debug,
    mem,
    net::{IpAddr, UdpSocket},
    sync::{Arc, Weak},
    time::{Duration, Instant},
};
use tokio::{select, sync::mpsc::Receiver, time::interval};
use tokio_util::sync::CancellationToken;

use super::ArtnetError;
use crate::{
    defs::UniverseDefinition,
    defs::{self, TargetValue},
    dmx::*,
    messages::{ToArtnetManagerMessage, ToMqttPublisherMessage},
};

//NOTE: Actual Artnet packet sending is commented out

#[derive(Debug)]
pub(super) struct ArtnetController {
    socket: UdpSocket,
}

#[derive(Debug)]
pub(super) struct Universe {
    description: String,

    controller: Arc<ArtnetController>,
    packet_bytes: Vec<u8>,
    modified: bool,
    log: bool,
    disable_send: bool,
    non_modified_ticks: usize, // Number of ticks in which this universe was not modified (used to determine when to send a packet)
}

pub trait EffectNodeRuntime: Debug + Send {
    fn tick(&mut self, artnet_manager: &mut ArtnetManager) -> Result<(), Report<ArtnetError>>;
    fn is_done(&self) -> bool;
}

pub struct ArtnetManager {
    pub(super) universes: HashMap<String, Universe>,
    pub(super) controllers: HashMap<IpAddr, Weak<ArtnetController>>,
    active_effects: HashMap<String, Box<dyn EffectNodeRuntime>>,
    #[cfg(test)]
    pub(super) set_channel_log: Vec<ChannelValue>,
}

pub(super) const DMX_DATA_OFFSET: usize = 18;
const DMX_SEQ_OFFSET: usize = 12;
const DMX_UDP_PORT: u16 = 0x1936;
const ARTNET_OPCODE_OUTPUT: u16 = 0x5000;
const TICK_DURATION: Duration = Duration::from_millis(50);
const SEND_UNMODIFIED_UNIVERSE_EVERY: usize = 20 * 4; // 20 ticks per second, send every 4 seconds

impl ArtnetManager {
    pub fn new() -> ArtnetManager {
        ArtnetManager {
            universes: HashMap::new(),
            controllers: HashMap::new(),
            active_effects: HashMap::new(),
            #[cfg(test)]
            set_channel_log: Vec::new(),
        }
    }

    pub(super) fn add_universe(
        &mut self,
        universe_id: &str,
        definition: UniverseDefinition,
    ) -> Result<(), Report<ArtnetError>> {
        let controller = match self.controllers.get(&definition.controller) {
            Some(c) => match c.upgrade() {
                Some(c) => c,
                None => {
                    let controller = Arc::new(ArtnetController::new(&definition.controller)?);
                    self.controllers
                        .insert(definition.controller, Arc::downgrade(&controller));
                    controller
                }
            },
            None => {
                let controller = Arc::new(ArtnetController::new(&definition.controller)?);
                self.controllers
                    .insert(definition.controller, Arc::downgrade(&controller));
                controller
            }
        };

        let universe = Universe::new(controller, universe_id, definition)?;
        self.universes.insert(universe_id.to_owned(), universe);

        Ok(())
    }

    pub(super) fn remove_universe(&mut self, universe_id: &str) -> Result<(), Report<ArtnetError>> {
        self.universes
            .remove(universe_id)
            .ok_or_else(|| ArtnetError::InvalidUniverse(universe_id.to_string()))?;

        let to_remove = self
            .controllers
            .iter()
            .filter(|(_, c)| c.upgrade().is_none())
            .map(|(ip, _)| *ip)
            .collect::<Vec<IpAddr>>();

        for ip in to_remove.iter() {
            self.controllers.remove(ip);
        }

        Ok(())
    }

    pub(super) fn start_effect(
        &mut self,
        effect_id: &str,
        effect: Box<dyn EffectNodeRuntime>,
    ) -> Result<(), Report<ArtnetError>> {
        info!("Starting effect {}: {:?}", effect_id, effect);
        self.active_effects.insert(effect_id.to_owned(), effect);
        Ok(())
    }

    fn stop_effect(&mut self, effect_id: &str) -> Result<(), Report<ArtnetError>> {
        info!("Stopping effect {}", effect_id);
        self.active_effects.remove(effect_id);
        Ok(())
    }

    /// Ticks every running effect. An effect whose tick fails is stopped, and its id and error
    /// returned; every other effect runs on. (It used to return at the first failure, and the
    /// effects taken out to run were never put back: one bad effect stopped every fade.)
    pub(super) fn tick(&mut self) -> Vec<(String, Report<ArtnetError>)> {
        let mut active_effects = mem::take(&mut self.active_effects);
        let mut failed = Vec::new();

        active_effects.retain(|effect_id, effect| match effect.tick(self) {
            Ok(()) if effect.is_done() => {
                trace!("Effect {} completed", effect_id);
                false
            }
            Ok(()) => true,
            Err(e) => {
                info!(effect_id = %effect_id, error = %e, "Effect stopped: its tick failed");
                failed.push((effect_id.clone(), e));
                false
            }
        });

        self.active_effects = active_effects; // Move it back
        failed
    }

    pub fn set_channel(&mut self, universe_id: &str, v: &ChannelValue) -> Result<(), Report<ArtnetError>> {
        trace!("Setting channel {} to {:?}", v.channel, v.value);

        match self.universes.get_mut(universe_id) {
            Some(u) => {
                if u.log {
                    #[cfg(test)]
                    self.set_channel_log.push(v.clone());
                }
                u.set_channel(v)
            }
            None => Err(ArtnetError::InvalidUniverse(universe_id.to_string()).into()),
        }
    }

    pub fn get_channel(
        &self,
        universe_id: &str,
        channel_definition: &ChannelDefinition,
    ) -> Result<ChannelValue, Report<ArtnetError>> {
        match self.universes.get(universe_id) {
            Some(u) => u.get_channel(channel_definition),
            None => Err(ArtnetError::InvalidUniverse(universe_id.to_string()).into()),
        }
    }

    /// Sends every universe that is due; how many packets went out on the wire (a universe with
    /// `disable_send` sends nothing).
    fn send_modified_universes(&mut self) -> Result<usize, Report<ArtnetError>> {
        let mut sent = 0;
        for (universe_id, universe) in self.universes.iter_mut() {
            if !universe.modified {
                universe.non_modified_ticks += 1;
                if universe.non_modified_ticks >= SEND_UNMODIFIED_UNIVERSE_EVERY {
                    universe.modified = true;
                }
            }

            if universe.modified {
                debug!("Sending packet to {}", universe_id);
                universe.send()?;
                if !universe.disable_send {
                    sent += 1;
                }
            }
        }
        Ok(sent)
    }

    fn set_channels(
        &mut self,
        parameters: &defs::SetChannelsParameters,
    ) -> Result<(), Report<ArtnetError>> {
        let into_context = || ArtnetError::Context(format!("Setting channels {:?}", parameters));
        let mut target = parameters.target.parse::<TargetValue>()?;
        let channels = parameters
            .channels
            .split(',')
            .map(|c| c.parse::<ChannelDefinition>().change_context_lazy(into_context))
            .collect::<Result<Vec<ChannelDefinition>, Report<_>>>()?;

        if let Some(dimming_amount) = parameters.dimming_amount {
            target = target.get_dimmed_value(dimming_amount);
        }

        for channel_definition in channels.iter() {
            let channel_value = target.get(channel_definition);

            if let Some(channel_value) = channel_value {
                let channel_value = ChannelValue {
                    channel: channel_definition.clone(),
                    value: channel_value,
                };
                self.set_channel(&parameters.universe_id, &channel_value)?;
            } else {
                return Err(ArtnetError::MissingTargetValue(
                    channel_definition.to_string(),
                    parameters.target.to_string(),
                ).into());
            }
        }

        Ok(())
    }

    fn handle_message(&mut self, message: ToArtnetManagerMessage) {
        match message {
            ToArtnetManagerMessage::AddUniverse(universe_id, definition, reply_tx) => {
                let _ = reply_tx.send(self.add_universe(&universe_id, definition));
            }
            ToArtnetManagerMessage::RemoveUniverse(universe_id, sender) => {
                let _ = sender.send(self.remove_universe(&universe_id));
            }
            ToArtnetManagerMessage::StartEffect(effect_id, effect_node_runtime, reply_tx) => {
                let _ = reply_tx.send(self.start_effect(&effect_id, effect_node_runtime));
            }
            ToArtnetManagerMessage::StopEffect(effect_id, sender) => {
                let _ = sender.send(self.stop_effect(&effect_id));
            }
            ToArtnetManagerMessage::SetChannels(parameters, sender) => {
                let _ = sender.send(self.set_channels(&parameters));
            }
        }
    }

    pub async fn run(
        &mut self,
        cancel: CancellationToken,
        mut receiver: Receiver<ToArtnetManagerMessage>,
        to_mqtt_publisher: async_channel::Sender<ToMqttPublisherMessage>,
    ) {
        // Set tick timer
        let mut tick_timer = interval(TICK_DURATION);
        let mut reporter = Reporter::new(to_mqtt_publisher);
        let mut send_outage = SendOutage::default();

        'run: loop {
            // WAIT: artnet-loop
            select! {
                _ = cancel.cancelled() => break,

                _ = tick_timer.tick() => {
                    for (effect_id, e) in self.tick() {
                        // A command that failed, logged where it failed (the logging policy):
                        // once, since the tick stops the effect.
                        warn!(kind = "command_rejected", effect_id = %effect_id, error = %e,
                              "DMX effect failed and was stopped");
                        if !reporter.report(format!("Effect {effect_id}: {e}")) {
                            break 'run;
                        }
                    }

                    match self.send_modified_universes() {
                        Ok(0) => {}
                        Ok(_) => send_outage.sent(),
                        Err(e) => {
                            send_outage.failed(&e);
                            if !reporter.report(e.to_string()) {
                                break;
                            }
                        }
                    }

                    reporter.tick();
                },

                message = receiver.recv() => match message {
                    None => break,
                    Some(message) => self.handle_message(message),
                },
            }
        }

        info!("ArtnetManager stopped");
    }
}

/// How long the ArtNet sends may keep failing before it is a WARN, once per outage.
const SEND_OUTAGE_WARN_AFTER: Duration = Duration::from_secs(30);

/// The ArtNet nodes' outage, as the sends see it: a send the kernel refuses — no route to the
/// node, its port unreachable — is a failed attempt, and the tick tries again 50 ms later. One
/// episode in the log (no-hang F2; it was a WARN `external_failure` per tick, 20 a second): INFO
/// `device_connection_lost` at the first failure, DEBUG for the rest, one WARN `device_unreachable`
/// once it has lasted 30 s, and INFO `device_recovered` with its length and `attempts` when a packet
/// goes out again — the only proof of life ArtNet gives, since a node never answers.
#[derive(Default)]
pub(super) struct SendOutage {
    since: Option<Instant>,
    attempts: u64,
    warned: bool,
}

impl SendOutage {
    pub(super) fn failed(&mut self, error: &Report<ArtnetError>) {
        self.failed_at(Instant::now(), &error.to_string());
    }

    pub(super) fn sent(&mut self) {
        self.sent_at(Instant::now());
    }

    pub(super) fn failed_at(&mut self, now: Instant, error: &str) {
        self.attempts += 1;
        let Some(since) = self.since else {
            self.since = Some(now);
            info!(kind = "device_connection_lost", error, "ArtNet send failed: the node is unreachable");
            return;
        };
        let down_for_ms = now.saturating_duration_since(since).as_millis() as u64;
        debug!(kind = "device_connection_lost", error, attempts = self.attempts, down_for_ms, "ArtNet send failed again");
        if !self.warned && now.saturating_duration_since(since) >= SEND_OUTAGE_WARN_AFTER {
            self.warned = true;
            warn!(kind = "device_unreachable", attempts = self.attempts, down_for_ms, error,
                  "ArtNet node unreachable: sends have failed for over 30 s");
        }
    }

    pub(super) fn sent_at(&mut self, now: Instant) {
        if let Some(since) = self.since.take() {
            info!(kind = "device_recovered", down_for_ms = now.saturating_duration_since(since).as_millis() as u64,
                  attempts = self.attempts, "ArtNet sends go out again");
        }
        self.attempts = 0;
        self.warned = false;
    }
}

/// Hands the ArtNet manager's error reports to the MQTT publisher without waiting (Store no-hang
/// §14.3). The ticker never waits on MQTT: the commands that wait on its replies come through the
/// MQTT session, and while the broker is slow, or the bridge is between sessions and nobody takes
/// from the queue, a wait here would also freeze every running fade.
///
/// With the queue full, the newest report displaces the oldest. A run of displaced reports is one
/// episode: each report at DEBUG, one WARN when it starts, and an INFO with how many were dropped
/// once the publisher has drained the queue to half (a fade failing at 20 ticks a second would
/// otherwise be 20 WARNs a second).
pub(super) struct Reporter {
    to_mqtt_publisher: async_channel::Sender<ToMqttPublisherMessage>,
    /// While reports are being dropped: since when, and how many so far.
    dropping: Option<(Instant, u64)>,
}

impl Reporter {
    pub(super) fn new(to_mqtt_publisher: async_channel::Sender<ToMqttPublisherMessage>) -> Reporter {
        Reporter { to_mqtt_publisher, dropping: None }
    }

    /// Queues `error`, displacing the oldest report when the queue is full. `false` once the
    /// queue is closed.
    pub(super) fn report(&mut self, error: String) -> bool {
        match self.to_mqtt_publisher.force_send(ToMqttPublisherMessage::Error(error)) {
            Ok(None) => true,
            Ok(Some(ToMqttPublisherMessage::Error(displaced))) => {
                debug!(error = %displaced, "DMX error report dropped");
                match &mut self.dropping {
                    Some((_, dropped)) => *dropped += 1,
                    None => {
                        self.dropping = Some((Instant::now(), 1));
                        warn!(kind = "error_report_dropped", queue = self.to_mqtt_publisher.capacity(),
                              "DMX error reports are being dropped: the MQTT error queue is full");
                    }
                }
                true
            }
            Err(_) => false,
        }
    }

    /// Ends an episode of dropped reports once the publisher has drained the queue to half: an
    /// INFO with its total. Called every tick.
    pub(super) fn tick(&mut self) {
        if let Some((since, dropped)) = self.dropping {
            let half = self.to_mqtt_publisher.capacity().map_or(usize::MAX, |c| c / 2);
            if self.to_mqtt_publisher.len() <= half {
                self.dropping = None;
                info!(kind = "error_report_drop_ended", dropped, lasted_ms = since.elapsed().as_millis() as u64,
                      "DMX error reports reach the MQTT error queue again");
            }
        }
    }
}

impl ArtnetController {
    pub fn new(controller: &IpAddr) -> Result<ArtnetController, Report<ArtnetError>> {
        let into_context = || ArtnetError::Context(format!("Creating artnet controller at {}", controller));

        let socket = UdpSocket::bind("0.0.0.0:0").change_context_lazy(into_context)?;
        socket.connect((*controller, DMX_UDP_PORT)).change_context_lazy(into_context)?;
        socket.set_nonblocking(true).change_context_lazy(into_context)?;

        Ok(ArtnetController { socket })
    }

    pub fn send(&self, packet_bytes: &[u8]) -> Result<(), Report<ArtnetError>> {
        match self.socket.send(packet_bytes) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(Report::new(e)
                .change_context(ArtnetError::Context(String::from("Sending Artnet packet")))),
        }
    }
}

impl Universe {
    pub fn new(
        controller: Arc<ArtnetController>,
        universe_id: &str,
        definition: UniverseDefinition,
    ) -> Result<Universe, Report<ArtnetError>> {
        let into_context = || ArtnetError::Context(format!("Creating universe {}", universe_id));

        if definition.universe > 15 {
            return Err(ArtnetError::InvalidUniverseNumber(definition.universe)).change_context_lazy(into_context);
        }
        if definition.subnet > 15 {
            return Err(ArtnetError::InvalidSubnet(definition.subnet)).change_context_lazy(into_context);
        }
        if definition.net > 127 {
            return Err(ArtnetError::InvalidNet(definition.net)).change_context_lazy(into_context);
        }
        if definition.channels > 512 {
            return Err(ArtnetError::TooManyChannels(definition.channels)).change_context_lazy(into_context);
        }

        let channel_count = (definition.channels + 1) as usize & !1; // Round up to even number of channels
        let mut packet_bytes = Vec::<u8>::with_capacity(channel_count + DMX_DATA_OFFSET);

        packet_bytes.append(&mut vec![b'A', b'r', b't', b'-', b'N', b'e', b't', 0x00]);
        packet_bytes.push((ARTNET_OPCODE_OUTPUT & 0xff) as u8);
        packet_bytes.push((ARTNET_OPCODE_OUTPUT >> 8) as u8);
        packet_bytes.push(0x00); // Protocol version Hi
        packet_bytes.push(0x14); // Protocol version Lo
        packet_bytes.push(0x00); // Sequence
        packet_bytes.push(0x00); // Physical
        packet_bytes.push(definition.subnet << 4 | definition.universe); // Subuniverse
        packet_bytes.push(definition.net); // net
        packet_bytes.push((channel_count >> 8) as u8); // Length Hi
        packet_bytes.push((channel_count & 0xff) as u8); // Length Lo

        if packet_bytes.len() != DMX_DATA_OFFSET {
            return Err(ArtnetError::Context(format!(
                "Internal error: packet header size mismatch ({} != {})",
                packet_bytes.len(),
                DMX_DATA_OFFSET
            )))
            .change_context_lazy(into_context);
        }
        packet_bytes.extend(std::iter::repeat_n(0x00, channel_count));

        Ok(Universe {
            description: format!("{0} ({1})", universe_id, definition.description),
            controller,
            log: definition.log,
            disable_send: definition.disable_send,
            packet_bytes,
            modified: false,
            non_modified_ticks: 0,
        })
    }

    #[cfg(test)]
    pub(super) fn get_packet_bytes(&self) -> &Vec<u8> {
        &self.packet_bytes
    }

    fn get_channel_count(&self) -> u16 {
        (self.packet_bytes.len() - DMX_DATA_OFFSET) as u16
    }

    fn validate_channel(&self, channel: u16) -> Result<(), Report<ArtnetError>> {
        if channel >= self.get_channel_count() {
            Err(ArtnetError::InvalidChannel(
                self.description.clone(),
                channel,
                self.get_channel_count(),
            ).into())
        } else {
            Ok(())
        }
    }

    pub fn set_channel(&mut self, v: &ChannelValue) -> Result<(), Report<ArtnetError>> {
        match v.channel {
            ChannelDefinition::Single(channel) => {
                self.validate_channel(channel)?;
                if let DimmerValue::Single(value) = v.value {
                    self.packet_bytes[DMX_DATA_OFFSET + channel as usize] = value;
                    Ok(())
                } else {
                    Err(ArtnetError::ChannelValueMismatch(
                        self.description.clone(),
                        v.channel.to_string(),
                        v.value.to_string(),
                    ))
                }
            },
            ChannelDefinition::Rgb(r_channel, g_channel, b_channel) => {
                self.validate_channel(r_channel)?;
                self.validate_channel(g_channel)?;
                self.validate_channel(b_channel)?;
                if let DimmerValue::Rgb(r, g, b) = v.value {
                    self.packet_bytes[DMX_DATA_OFFSET + r_channel as usize] = r;
                    self.packet_bytes[DMX_DATA_OFFSET + g_channel as usize] = g;
                    self.packet_bytes[DMX_DATA_OFFSET + b_channel as usize] = b;
                    Ok(())
                } else {
                    Err(ArtnetError::ChannelValueMismatch(
                        self.description.clone(),
                        v.channel.to_string(),
                        v.value.to_string(),
                    ))
                }
            }
            ChannelDefinition::TriWhite(w1_channel, w2_channel, w3_channel) => {
                self.validate_channel(w1_channel)?;
                self.validate_channel(w2_channel)?;
                self.validate_channel(w3_channel)?;
                if let DimmerValue::TriWhite(w1, w2, w3) = v.value {
                    self.packet_bytes[DMX_DATA_OFFSET + w1_channel as usize] = w1;
                    self.packet_bytes[DMX_DATA_OFFSET + w2_channel as usize] = w2;
                    self.packet_bytes[DMX_DATA_OFFSET + w3_channel as usize] = w3;
                    Ok(())
                } else {
                    Err(ArtnetError::ChannelValueMismatch(
                        self.description.clone(),
                        v.channel.to_string(),
                        v.value.to_string(),
                    ))
                }
            }
        }?;

        self.modified = true;
        Ok(())
    }

    pub fn get_channel(
        &self,
        channel_definition: &ChannelDefinition,
    ) -> Result<ChannelValue, Report<ArtnetError>> {
        match channel_definition {
            ChannelDefinition::Single(s) => {
                self.validate_channel(*s)?;
                Ok(ChannelValue {
                    channel: channel_definition.clone(),
                    value: DimmerValue::Single(
                        self.packet_bytes[DMX_DATA_OFFSET + *s as usize],
                    ),
                })
            },
            ChannelDefinition::Rgb(r, g, b) => {
                self.validate_channel(*r)?;
                self.validate_channel(*g)?;
                self.validate_channel(*b)?;
                Ok(ChannelValue {
                    channel: channel_definition.clone(),
                    value: DimmerValue::Rgb(
                        self.packet_bytes[DMX_DATA_OFFSET + *r as usize],
                        self.packet_bytes[DMX_DATA_OFFSET + *g as usize],
                        self.packet_bytes[DMX_DATA_OFFSET + *b as usize],
                    ),
                })

            },
            ChannelDefinition::TriWhite(w1, w2, w3) => {
                self.validate_channel(*w1)?;
                self.validate_channel(*w2)?;
                self.validate_channel(*w3)?;
                Ok(ChannelValue {
                    channel: channel_definition.clone(),
                    value: DimmerValue::TriWhite(
                        self.packet_bytes[DMX_DATA_OFFSET + *w1 as usize],
                        self.packet_bytes[DMX_DATA_OFFSET + *w2 as usize],
                        self.packet_bytes[DMX_DATA_OFFSET + *w3 as usize],
                    ),
                })
            },
        }
    }

    pub fn send(&mut self) -> Result<(), Report<ArtnetError>> {
        if !self.disable_send {
            self.controller.send(self.packet_bytes.as_slice())?;
        }
        self.packet_bytes[DMX_SEQ_OFFSET] = self.packet_bytes[DMX_SEQ_OFFSET].wrapping_add(1);
        self.modified = false;
        self.non_modified_ticks = 0;
        Ok(())
    }
}

#[cfg(test)]
mod send_outage_tests {
    use super::SendOutage;
    use crate::test_log::capture;
    use std::time::{Duration, Instant};
    use tracing::Level;

    /// Sends that keep failing — the node's network gone, a tick every 50 ms — are one episode in
    /// the log: one INFO `device_connection_lost`, one WARN `device_unreachable` past 30 s, and one
    /// INFO `device_recovered` with its length when a packet goes out again. Never `external_failure`
    /// per tick, as the publisher logged each one (Store no-hang 3b, round 2).
    #[test]
    fn failing_sends_are_one_outage_episode() {
        let mut outage = SendOutage::default();
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let events = capture(|| {
            for ms in (0..40_000).step_by(50) {
                outage.failed_at(at(ms), "No route to host");
            }
            outage.sent_at(at(40_000));
            outage.sent_at(at(40_050));
        });
        let of = |level: Level, kind: &str| events.iter().filter(|e| e.level == level && e.kind() == Some(kind)).count();
        assert_eq!(of(Level::INFO, "device_connection_lost"), 1, "not one INFO as the sends began to fail: {events:#?}");
        assert_eq!(of(Level::DEBUG, "device_connection_lost"), 799, "the later failures are not DEBUG");
        let warns: Vec<_> = events.iter().filter(|e| e.level <= Level::WARN).collect();
        assert!(
            warns.len() == 1 && warns[0].kind() == Some("device_unreachable") && warns[0].field("down_for_ms") == Some("30000"),
            "not one WARN device_unreachable once the sends had failed for 30 s: {warns:#?}"
        );
        let recovered: Vec<_> = events.iter().filter(|e| e.level == Level::INFO && e.kind() == Some("device_recovered")).collect();
        assert!(
            recovered.len() == 1 && recovered[0].field("down_for_ms") == Some("40000"),
            "not one INFO device_recovered with the outage's length: {recovered:#?}"
        );
        // The fleet's field for an outage's count (Store's kind table; review round 3, X5).
        assert_eq!(warns[0].field("attempts"), Some("601"), "the WARN does not carry the outage's attempts");
        assert_eq!(recovered[0].field("attempts"), Some("800"), "the recovery does not carry the outage's attempts");
    }
}
