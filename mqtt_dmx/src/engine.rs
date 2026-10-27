//! Controlled-tick effect execution. The owning service supplies accepted, resolved
//! effects and a fixed physical ownership set; this component does no I/O.

use dmx_protocol::{ChannelPlanes, Error, Level};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, SocketAddr};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    pub endpoint: SocketAddr,
    pub net: u8,
    pub subnet: u8,
    pub universe: u8,
    pub channel: u16,
}

impl Channel {
    pub(crate) fn canonical(mut self) -> Self {
        if let IpAddr::V6(address) = self.endpoint.ip() {
            if let Some(address) = address.to_ipv4_mapped() {
                self.endpoint = SocketAddr::new(address.into(), self.endpoint.port());
            }
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub channel: Channel,
    pub planes: ChannelPlanes,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Effect {
    Fade { ticks: u32, targets: Vec<Target> },
    Delay { ticks: u32 },
    Sequence { nodes: Vec<Effect> },
    Parallel { nodes: Vec<Effect> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Running,
    Completed,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub effect: Effect,
    pub initial: Vec<Target>,
    pub held: Vec<Target>,
    pub level: Level,
    pub status: Status,
}

#[derive(Clone)]
pub struct Player {
    effect: Effect,
    initial: BTreeMap<Channel, ChannelPlanes>,
    output: BTreeMap<Channel, ChannelPlanes>,
    level: Level,
    status: Status,
    runtime: Option<Runtime>,
}

impl Player {
    pub fn start(
        owned: impl IntoIterator<Item = Channel>,
        mut effect: Effect,
        initial: Vec<Target>,
        level: Level,
    ) -> Result<Self, Error> {
        let owned = ownership(owned)?;
        validate_effect(&mut effect, &owned, 0, &mut (0, 0))?;
        crate::memory::reserve_player(&effect, owned.len())?;
        let initial = checked_planes(initial, &owned)?;
        let runtime = Some(Runtime::new(&effect));
        let mut player = Self {
            effect,
            output: initial.clone(),
            initial,
            level,
            status: Status::Running,
            runtime,
        };
        player.advance(false);
        Ok(player)
    }
    pub fn tick(&mut self) {
        self.advance(true);
    }
    fn advance(&mut self, allow_tick: bool) {
        if let Some(runtime) = &mut self.runtime {
            if runtime.advance(&mut self.output, allow_tick).done {
                self.runtime = None;
                self.status = Status::Completed;
            }
        }
    }
    pub fn status(&self) -> Status {
        self.status
    }
    pub(crate) fn effect(&self) -> &Effect {
        &self.effect
    }
    pub fn set_level(&mut self, level: Level) {
        self.level = level;
    }
    pub fn stop(&mut self) {
        self.status = Status::Stopped;
        self.runtime = None;
    }
    pub fn output(&self, channel: &Channel) -> u8 {
        self.output
            .get(&channel.clone().canonical())
            .copied()
            .unwrap_or(ChannelPlanes {
                dimmable: 0,
                independent: 0,
            })
            .render(self.level)
    }
    pub fn snapshot(&self) -> Snapshot {
        let targets = |planes: &BTreeMap<Channel, ChannelPlanes>| {
            planes
                .iter()
                .map(|(channel, planes)| Target {
                    channel: channel.clone(),
                    planes: *planes,
                })
                .collect()
        };
        Snapshot {
            effect: self.effect.clone(),
            initial: targets(&self.initial),
            held: targets(&self.output),
            level: self.level,
            status: self.status,
        }
    }
    pub fn restore(
        owned: impl IntoIterator<Item = Channel>,
        snapshot: Snapshot,
    ) -> Result<Self, Error> {
        let owned = ownership(owned)?;
        if snapshot.initial.len() != owned.len() || snapshot.held.len() != owned.len() {
            return Err(invalid("snapshot must cover every owned physical channel"));
        }
        let held = checked_planes(snapshot.held, &owned)?;
        let mut player = Self::start(owned, snapshot.effect, snapshot.initial, snapshot.level)?;
        if snapshot.status != Status::Running {
            player.output = held;
            player.status = snapshot.status;
            player.runtime = None;
        }
        Ok(player)
    }
}

type Planes = BTreeMap<Channel, ChannelPlanes>;
const ZERO: ChannelPlanes = ChannelPlanes {
    dimmable: 0,
    independent: 0,
};

fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}

fn validate_channel(channel: &Channel) -> Result<(), Error> {
    if channel.net > 127
        || channel.subnet > 15
        || channel.universe > 15
        || channel.channel >= 512
        || channel.endpoint.port() == 0
    {
        return Err(invalid("invalid physical Art-Net channel"));
    }
    Ok(())
}

fn ownership(channels: impl IntoIterator<Item = Channel>) -> Result<BTreeSet<Channel>, Error> {
    let mut owned = BTreeSet::new();
    for channel in channels {
        let channel = channel.canonical();
        validate_channel(&channel)?;
        owned.insert(channel);
        if owned.len() > 32 * 512 {
            return Err(invalid("array ownership exceeds channel limit"));
        }
    }
    Ok(owned)
}

fn checked_planes(targets: Vec<Target>, owned: &BTreeSet<Channel>) -> Result<Planes, Error> {
    let mut seen = BTreeSet::new();
    let mut result = owned.iter().cloned().map(|c| (c, ZERO)).collect::<Planes>();
    for target in targets {
        let channel = target.channel.canonical();
        if !owned.contains(&channel) {
            return Err(invalid("snapshot channel is outside array ownership"));
        }
        if !seen.insert(channel.clone()) {
            return Err(invalid("duplicate physical snapshot channel"));
        }
        result.insert(channel, target.planes);
    }
    Ok(result)
}

fn validate_effect(
    effect: &mut Effect,
    owned: &BTreeSet<Channel>,
    depth: usize,
    counts: &mut (usize, usize),
) -> Result<(), Error> {
    counts.0 += 1;
    if depth > 32 || counts.0 > 1024 {
        return Err(invalid("effect exceeds node/depth limit"));
    }
    match effect {
        Effect::Fade { targets, .. } => {
            counts.1 = counts
                .1
                .checked_add(targets.len())
                .ok_or_else(|| invalid("too many channel references"))?;
            if counts.1 > 32768 {
                return Err(invalid("effect exceeds expanded channel-reference budget"));
            }
            let mut unique = BTreeMap::new();
            for target in targets.iter_mut() {
                target.channel = target.channel.clone().canonical();
                if !owned.contains(&target.channel) {
                    return Err(invalid("effect channel is outside array ownership"));
                }
                unique.insert(target.channel.clone(), target.planes);
            }
            *targets = unique
                .into_iter()
                .map(|(channel, planes)| Target { channel, planes })
                .collect();
        }
        Effect::Sequence { nodes } | Effect::Parallel { nodes } => {
            for node in nodes {
                validate_effect(node, owned, depth + 1, counts)?;
            }
        }
        Effect::Delay { .. } => {}
    }
    Ok(())
}

struct Step {
    done: bool,
    consumed: bool,
}

#[derive(Clone)]
enum Runtime {
    Fade {
        ticks: u32,
        elapsed: u32,
        targets: Vec<Target>,
        initial: Option<Planes>,
    },
    Delay {
        remaining: u32,
    },
    Sequence {
        nodes: Vec<Runtime>,
        cursor: usize,
    },
    Parallel {
        nodes: Vec<Runtime>,
    },
}

impl Runtime {
    fn new(effect: &Effect) -> Self {
        match effect {
            Effect::Fade { ticks, targets } => Self::Fade {
                ticks: *ticks,
                elapsed: 0,
                targets: targets.clone(),
                initial: None,
            },
            Effect::Delay { ticks } => Self::Delay { remaining: *ticks },
            Effect::Sequence { nodes } => Self::Sequence {
                nodes: nodes.iter().map(Self::new).collect(),
                cursor: 0,
            },
            Effect::Parallel { nodes } => Self::Parallel {
                nodes: nodes.iter().map(Self::new).collect(),
            },
        }
    }

    fn advance(&mut self, output: &mut Planes, allow_tick: bool) -> Step {
        match self {
            Self::Fade {
                ticks,
                elapsed,
                targets,
                initial,
            } => {
                if *ticks == 0 {
                    if initial.is_none() {
                        for target in targets.iter() {
                            output.insert(target.channel.clone(), target.planes);
                        }
                        *initial = Some(BTreeMap::new());
                    }
                    return Step {
                        done: true,
                        consumed: false,
                    };
                }
                if *elapsed == *ticks {
                    return Step {
                        done: true,
                        consumed: false,
                    };
                }
                if !allow_tick {
                    return Step {
                        done: false,
                        consumed: false,
                    };
                }
                let initial = initial.get_or_insert_with(|| {
                    targets
                        .iter()
                        .map(|target| {
                            (
                                target.channel.clone(),
                                *output.get(&target.channel).unwrap_or(&ZERO),
                            )
                        })
                        .collect()
                });
                *elapsed += 1;
                for target in targets {
                    let start = initial[&target.channel];
                    output.insert(
                        target.channel.clone(),
                        ChannelPlanes {
                            dimmable: blend(
                                start.dimmable,
                                target.planes.dimmable,
                                *elapsed,
                                *ticks,
                            ),
                            independent: blend(
                                start.independent,
                                target.planes.independent,
                                *elapsed,
                                *ticks,
                            ),
                        },
                    );
                }
                Step {
                    done: *elapsed == *ticks,
                    consumed: true,
                }
            }
            Self::Delay { remaining } => {
                let consumed = *remaining != 0 && allow_tick;
                if consumed {
                    *remaining -= 1;
                }
                Step {
                    done: *remaining == 0,
                    consumed,
                }
            }
            Self::Sequence { nodes, cursor } => {
                let mut consumed = false;
                while *cursor < nodes.len() {
                    let step = nodes[*cursor].advance(output, allow_tick && !consumed);
                    consumed |= step.consumed;
                    if !step.done {
                        break;
                    }
                    *cursor += 1;
                }
                Step {
                    done: *cursor == nodes.len(),
                    consumed,
                }
            }
            Self::Parallel { nodes } => {
                let mut done = true;
                let mut consumed = false;
                for node in nodes {
                    let step = node.advance(output, allow_tick);
                    done &= step.done;
                    consumed |= step.consumed;
                }
                Step { done, consumed }
            }
        }
    }
}

fn blend(from: u8, to: u8, elapsed: u32, ticks: u32) -> u8 {
    let ticks = u64::from(ticks);
    let elapsed = u64::from(elapsed);
    let weighted = u64::from(from) * (ticks - elapsed) + u64::from(to) * elapsed;
    ((weighted + ticks / 2) / ticks) as u8
}
