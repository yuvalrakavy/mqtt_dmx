//! Compile only frozen published content, with per-request parameters supplied
//! separately. Ownership always comes from the published @all definition.
use crate::engine::{Channel, Effect, Player, Target};
use dmx_protocol::{
    configuration::{Array, Configuration, Content, Node, Ticks, Universe},
    ChannelPlanes, Error, Id, Level,
};
use std::collections::{BTreeMap, BTreeSet};

pub struct CompiledConfiguration {
    configuration: Configuration,
    ownership: BTreeMap<Id, Vec<Channel>>,
}

impl CompiledConfiguration {
    pub fn compile(configuration: Configuration) -> Result<Self, Error> {
        configuration.encode()?;
        let content = &configuration.content;
        let mut names = BTreeSet::new();
        let mut addresses = BTreeSet::new();
        let mut identities = BTreeSet::new();
        let mut node_count = 0;
        for (id, universe) in &content.universes {
            label(&universe.name)?;
            let address = physical(universe, 0);
            if !names.insert(&universe.name)
                || !identities.insert(id)
                || !addresses.insert(address)
                || universe.channels == 0
                || universe.channels > 512
                || universe.net > 127
                || universe.subnet > 15
                || universe.universe > 15
                || universe.endpoint.port() == 0
            {
                return Err(invalid("invalid, duplicate or aliased universe"));
            }
        }
        names.clear();
        validate_values(&content.values)?;
        for (id, effect) in &content.effects {
            if !identities.insert(id) {
                return Err(invalid("duplicate object identity"));
            }
            label(&effect.name)?;
            count_nodes(&effect.definition, 0, &mut node_count)?;
        }
        let mut ownership = BTreeMap::new();
        let mut owners = BTreeMap::new();
        let mut generations = BTreeSet::new();
        for (id, array) in &content.arrays {
            label(&array.name)?;
            validate_values(&array.defaults)?;
            if !identities.insert(id)
                || !names.insert(&array.name)
                || !generations.insert(&array.generation)
                || !identities.insert(&array.generation)
            {
                return Err(invalid("duplicate array identity, name or generation"));
            }
            for (effect_id, effect) in &array.effects {
                if !identities.insert(effect_id) {
                    return Err(invalid("duplicate effect identity"));
                }
                label(&effect.name)?;
                count_nodes(&effect.definition, 0, &mut node_count)?;
            }
            let parameters = values(content, array, &BTreeMap::new())?;
            if !content.universes.values().any(|u| u.name == array.universe) {
                return Err(invalid("unknown default universe"));
            }
            for name in array.lights.keys() {
                label(name)?;
            }
            let lights = expand_lights(content, array, "@all", &parameters)?;
            let owned: BTreeSet<_> = lights
                .into_iter()
                .flat_map(|light| light.channels)
                .collect();
            for channel in &owned {
                if owners.insert(channel.clone(), id).is_some() {
                    return Err(invalid("arrays overlap physical channel ownership"));
                }
            }
            ownership.insert(id.clone(), owned.into_iter().collect());
            for selected in [&array.on, &array.off].into_iter().flatten() {
                if !array.effects.contains_key(selected) && !content.effects.contains_key(selected)
                {
                    return Err(invalid("array selects an unpublished effect"));
                }
            }
        }
        let compiled = Self {
            configuration,
            ownership,
        };
        // Definitions with required request parameters are admitted structurally;
        // every resolved execution is checked again against fixed ownership.
        for (id, array) in &compiled.content().arrays {
            for effect in array.effects.values().chain(
                [&array.on, &array.off]
                    .into_iter()
                    .flatten()
                    .filter_map(|id| compiled.content().effects.get(id)),
            ) {
                let parameters = values(compiled.content(), array, &BTreeMap::new())?;
                validate_leaves(
                    compiled.content(),
                    array,
                    &effect.definition,
                    &parameters,
                    &compiled.ownership[id],
                )?;
            }
        }
        Ok(compiled)
    }
    pub fn effect(
        &self,
        array: &Id,
        effect: Option<&Id>,
        parameters: &BTreeMap<String, String>,
        off: bool,
        fade_ms: u64,
    ) -> Result<Effect, Error> {
        let definition = self
            .content()
            .arrays
            .get(array)
            .ok_or_else(|| invalid("unknown array"))?;
        let selected = effect.or(if off {
            definition.off.as_ref()
        } else {
            definition.on.as_ref()
        });
        // Off deliberately excludes selected-On parameters.
        let empty = BTreeMap::new();
        let parameters = values(
            self.content(),
            definition,
            if off { &empty } else { parameters },
        )?;
        let effect = if let Some(selected) = selected {
            let node = definition
                .effects
                .get(selected)
                .or_else(|| self.content().effects.get(selected))
                .ok_or_else(|| invalid("unpublished effect"))?;
            compile_node(self.content(), definition, &node.definition, &parameters)?
        } else {
            let ticks = u32::try_from(fade_ms.div_ceil(50))
                .map_err(|_| invalid("fade duration exceeds tick range"))?;
            let value = if off { 0 } else { 255 };
            Effect::Fade {
                ticks,
                targets: self.ownership[array]
                    .iter()
                    .cloned()
                    .map(|channel| Target {
                        channel,
                        planes: ChannelPlanes {
                            dimmable: value,
                            independent: 0,
                        },
                    })
                    .collect(),
            }
        };
        // Player validation normalizes aliases and enforces expanded budgets. It
        // has no I/O; discard its execution and retain only the checked plan.
        Ok(
            Player::start(self.ownership[array].clone(), effect, vec![], Level::FULL)?
                .snapshot()
                .effect,
        )
    }
    pub fn content(&self) -> &Content {
        &self.configuration.content
    }
    pub(crate) fn has_effect(&self, array: &Id, effect: &Id) -> bool {
        self.content()
            .arrays
            .get(array)
            .is_some_and(|array| array.effects.contains_key(effect))
            || self.content().effects.contains_key(effect)
    }
    pub fn configuration(&self) -> &Configuration {
        &self.configuration
    }
    pub fn ownership(&self) -> &BTreeMap<Id, Vec<Channel>> {
        &self.ownership
    }
}

fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}
fn label(value: &str) -> Result<(), Error> {
    if value.is_empty() || value.len() > 128 || value.contains([',', '@', '$', '`', '/']) {
        return Err(invalid("invalid authored reference name"));
    }
    Ok(())
}
fn validate_values(values: &BTreeMap<String, String>) -> Result<(), Error> {
    for (name, value) in values {
        if name.is_empty() || name.len() > 128 || value.len() > 1024 || name.contains(['`', '=']) {
            return Err(invalid("invalid parameter name/value"));
        }
    }
    Ok(())
}
fn values(
    content: &Content,
    array: &Array,
    overrides: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, Error> {
    validate_values(overrides)?;
    let mut values = content.values.clone();
    values.extend(array.defaults.clone());
    values.extend(overrides.clone());
    Ok(values)
}
fn expand(text: &str, values: &BTreeMap<String, String>) -> Result<String, Error> {
    let mut rest = text;
    let mut result = String::new();
    while let Some(start) = rest.find('`') {
        result.push_str(&rest[..start]);
        rest = &rest[start + 1..];
        let end = rest
            .find('`')
            .ok_or_else(|| invalid("unterminated parameter expression"))?;
        let expression = &rest[..end];
        let (name, default) = expression
            .split_once('=')
            .map_or((expression, None), |(n, v)| (n, Some(v)));
        if name.is_empty() || name.len() > 128 {
            return Err(invalid("invalid parameter name"));
        }
        let value = values
            .get(name)
            .map(String::as_str)
            .or(default)
            .ok_or_else(|| invalid(&format!("missing parameter: {name}")))?;
        result.push_str(value);
        rest = &rest[end + 1..];
        if result.len() > 128 * 1024 {
            return Err(invalid("expanded expression too large"));
        }
    }
    result.push_str(rest);
    if result.len() > 128 * 1024 {
        return Err(invalid("expanded expression too large"));
    }
    Ok(result)
}
fn physical(universe: &Universe, channel: u16) -> Channel {
    Channel {
        endpoint: universe.endpoint,
        net: universe.net,
        subnet: universe.subnet,
        universe: universe.universe,
        channel,
    }
    .canonical()
}
struct Light {
    kind: String,
    channels: Vec<Channel>,
}
fn expand_lights(
    content: &Content,
    array: &Array,
    text: &str,
    values: &BTreeMap<String, String>,
) -> Result<Vec<Light>, Error> {
    fn visit(
        content: &Content,
        array: &Array,
        text: &str,
        values: &BTreeMap<String, String>,
        stack: &mut BTreeSet<String>,
        result: &mut Vec<Light>,
        budget: &mut usize,
    ) -> Result<(), Error> {
        let text = expand(text, values)?;
        let mut universe = array.universe.as_str();
        for entry in text.split(',').map(str::trim) {
            *budget += 1;
            if *budget > 32768 {
                return Err(invalid("expanded lights exceed reference budget"));
            }
            if let Some(group) = entry.strip_prefix('@') {
                if stack.len() >= 32 || !stack.insert(group.to_owned()) {
                    return Err(invalid("circular/deep light group"));
                }
                let text = array
                    .lights
                    .get(group)
                    .ok_or_else(|| invalid("unknown light group"))?;
                visit(content, array, text, values, stack, result, budget)?;
                stack.remove(group);
            } else if let Some(name) = entry.strip_prefix('$') {
                universe = name;
            } else {
                let universe = content
                    .universes
                    .values()
                    .find(|u| u.name == universe)
                    .ok_or_else(|| invalid("unknown universe reference"))?;
                let (kind, channels) = entry.split_once(':').unwrap_or(("s", entry));
                let kind = kind.trim().to_ascii_lowercase();
                let mut channels = channels
                    .split('/')
                    .map(|c| {
                        c.trim()
                            .parse::<u16>()
                            .map_err(|_| invalid("invalid channel number"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let width = match kind.as_str() {
                    "s" => 1,
                    "rgb" | "w" => 3,
                    _ => return Err(invalid("invalid channel kind")),
                };
                if channels.len() == 1 && width == 3 {
                    let first = channels[0];
                    channels.push(
                        first
                            .checked_add(1)
                            .ok_or_else(|| invalid("channel overflow"))?,
                    );
                    channels.push(
                        first
                            .checked_add(2)
                            .ok_or_else(|| invalid("channel overflow"))?,
                    );
                }
                if channels.len() != width
                    || channels.iter().any(|c| *c >= universe.channels)
                    || channels.iter().collect::<BTreeSet<_>>().len() != width
                {
                    return Err(invalid("invalid channel range/tuple"));
                }
                result.push(Light {
                    kind,
                    channels: channels
                        .into_iter()
                        .map(|c| physical(universe, c))
                        .collect(),
                });
            }
        }
        Ok(())
    }
    let mut result = Vec::new();
    visit(
        content,
        array,
        text,
        values,
        &mut BTreeSet::new(),
        &mut result,
        &mut 0,
    )?;
    Ok(result)
}
fn targets(text: &str) -> Result<BTreeMap<String, Vec<u8>>, Error> {
    let mut result = BTreeMap::new();
    for entry in text.split(';').map(str::trim) {
        let (kind, arguments) = entry
            .split_once('(')
            .ok_or_else(|| invalid("target needs parentheses"))?;
        let kind = kind.trim().to_ascii_lowercase();
        let arguments = arguments
            .strip_suffix(')')
            .ok_or_else(|| invalid("invalid target suffix"))?;
        let values = arguments
            .split(',')
            .map(|v| {
                v.trim()
                    .parse::<u8>()
                    .map_err(|_| invalid("invalid target byte"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let width = match kind.as_str() {
            "s" => 1,
            "rgb" | "w" => 3,
            _ => return Err(invalid("unknown target kind")),
        };
        if values.len() != width || result.insert(kind, values).is_some() {
            return Err(invalid("ambiguous or invalid target"));
        }
    }
    Ok(result)
}
fn ticks(ticks: &Ticks, values: &BTreeMap<String, String>) -> Result<u32, Error> {
    match ticks {
        Ticks::Number(n) => Ok(*n),
        Ticks::Parameter(text) => expand(text, values)?
            .parse()
            .map_err(|_| invalid("invalid tick count")),
    }
}
fn compile_node(
    content: &Content,
    array: &Array,
    node: &Node,
    values: &BTreeMap<String, String>,
) -> Result<Effect, Error> {
    compile_bounded(content, array, node, values, &mut 0)
}
fn compile_bounded(
    content: &Content,
    array: &Array,
    node: &Node,
    values: &BTreeMap<String, String>,
    references: &mut usize,
) -> Result<Effect, Error> {
    Ok(match node {
        Node::Delay { ticks: n } => Effect::Delay {
            ticks: ticks(n, values)?,
        },
        Node::Sequence { nodes } => Effect::Sequence {
            nodes: nodes
                .iter()
                .map(|n| compile_bounded(content, array, n, values, references))
                .collect::<Result<_, _>>()?,
        },
        Node::Parallel { nodes } => Effect::Parallel {
            nodes: nodes
                .iter()
                .map(|n| compile_bounded(content, array, n, values, references))
                .collect::<Result<_, _>>()?,
        },
        Node::Fade {
            lights,
            ticks: n,
            target,
            no_dimming,
        } => {
            let ticks = ticks(n, values)?;
            let target = targets(&expand(target, values)?)?;
            let mut output = Vec::new();
            for light in expand_lights(content, array, lights, values)? {
                if let Some(values) = target.get(&light.kind) {
                    for (channel, value) in light.channels.into_iter().zip(values.iter().copied()) {
                        *references += 1;
                        if *references > 32768 {
                            return Err(invalid("effect exceeds expanded channel budget"));
                        }
                        output.push(Target {
                            channel,
                            planes: if *no_dimming {
                                ChannelPlanes {
                                    dimmable: 0,
                                    independent: value,
                                }
                            } else {
                                ChannelPlanes {
                                    dimmable: value,
                                    independent: 0,
                                }
                            },
                        });
                    }
                }
            }
            Effect::Fade {
                ticks,
                targets: output,
            }
        }
    })
}
fn count_nodes(node: &Node, depth: usize, count: &mut usize) -> Result<(), Error> {
    *count += 1;
    if depth > 32 || *count > 1024 {
        return Err(invalid("configuration exceeds effect node/depth budget"));
    }
    if let Node::Sequence { nodes } | Node::Parallel { nodes } = node {
        for node in nodes {
            count_nodes(node, depth + 1, count)?;
        }
    }
    Ok(())
}
fn validate_leaves(
    content: &Content,
    array: &Array,
    node: &Node,
    values: &BTreeMap<String, String>,
    owned: &[Channel],
) -> Result<(), Error> {
    if let Node::Sequence { nodes } | Node::Parallel { nodes } = node {
        for node in nodes {
            validate_leaves(content, array, node, values, owned)?;
        }
        return Ok(());
    }
    match compile_node(content, array, node, values) {
        Ok(effect) => {
            Player::start(owned.iter().cloned(), effect, vec![], Level::FULL)?;
            Ok(())
        }
        Err(Error::Invalid(reason)) if reason.starts_with("missing parameter: ") => Ok(()),
        Err(error) => Err(error),
    }
}
