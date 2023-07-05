use std::collections::HashMap;

use super::*;
use crate::defs::{DmxArray, DIMMING_AMOUNT_MAX};
use crate::dmx::{ChannelDefinition, ChannelType};

#[test]
fn test_verify_array() {
    let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "all": "rgb:0"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                },
                "presets": [
                    {
                        "values": {
                        }
                    }
                ]
            }"#;

    let mut array_manager = ArrayManager::new();
    let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

    array_manager.add_array("test", array).unwrap();

    let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "all": "rgb:10"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    },
                    "custom": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(128); rgb(0,0,0); w(0)"
                    }
                },
                "values": {
                },
                "presets": [
                    {
                        "on": "custom",
                        "values": {
                        }
                    }
                ]
            }"#;

    let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

    array_manager.add_array("test2", array).unwrap();

    let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "all": "rgb:10"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                },
                "values": {
                },
                "presets": [
                    {
                        "on": "custom",
                        "values": {
                        }
                    }
                ]
            }"#;

    let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

    if let Err(e) = array_manager.add_array("test3", array) {
        let t = e.to_string();
        assert_eq!(
            t,
            "Array 'test3' preset 0 'on' effect is 'custom' which is not defined"
        );
    } else {
        panic!("Expected error");
    }

    let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "all": "@center,@spot,@frame"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                },
                "values": {
                },
                "presets": [
                    {
                        "off": "custom",
                        "values": {
                        }
                    }
                ]
            }"#;

    let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

    if let Err(e) = array_manager.add_array("test3", array) {
        let t = e.to_string();
        assert_eq!(
            t,
            "Array 'test3' preset 0 'off' effect is 'custom' which is not defined"
        );
    } else {
        panic!("Expected error");
    }

    let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "center": "rgb:10",
                    "spot": "s:20",
                    "frame": "w:30",
                    "all": "@center,@spot,@frame"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                }
            }"#;

    let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

    array_manager.add_array("test2", array).unwrap();

    let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "center": "rgb:10",
                    "spot": "s:20",
                    "frame": "w:30",
                    "outside": "rgb:40",
                    "all": "@center,@spot,@frame"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                }
            }"#;

    let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

    if let Err(e) = array_manager.add_array("test2", array) {
        let t = e.to_string();
        assert_eq!(t, "Array 'test2' in universe '0': channel 40 is defined as light red component in group @outside but is not included in @all group");
    }

    let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "all": "rgb:0,rgb:1"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                }
            }"#;

    let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

    if let Err(e) = array_manager.add_array("test2", array) {
        let t = e.to_string();
        assert_eq!(t, "Array 'test2' in universe '0': channel 1 was defined as light green component and is redefined as light red component in group @@all");
    }

    let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "all": "rgb:0,x:5"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                },
                "values": {
                },
                "presets": [
                ]
            }"#;

    let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

    if let Err(e) = array_manager.add_array("test2", array) {
        let t = e.to_string();
        assert_eq!(t, "Array 'test2' Light '@all -> rgb:0,x:5' (x:5) is invalid channel definition (s:n, rgb:n or w:n)");
    }

    let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "all": "rgb:0,@loop",
                    "loop": "rgb:3,@circle",
                    "circle": "@loop"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                },
                "values": {
                },
                "presets": [
                ]
            }"#;

    let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

    if let Err(e) = array_manager.add_array("test2", array) {
        let t = e.to_string();
        assert_eq!(t, "Array 'test2' Light '@all -> rgb:0,@loop -> rgb:3,@circle -> @loop -> rgb:3,@circle -> @loop' (@loop) contain circular reference to @circle");
    }
}

#[test]
fn test_get_array_light_channels() {
    let mut array_manager = ArrayManager::new();
    let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "center": "rgb:1,rgb:4",
                    "frame": "s:7",
                    "spot": "$2,w:100",
                    "all": "@center,@spot,@frame"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                },
                "values": {
                },
                "presets": [
                ]
            }"#;

    let array = serde_json::from_str::<DmxArray>(array_json).unwrap();
    array_manager.add_array("test".to_string(), array).unwrap();
    let scope = Scope::new(&array_manager, "test", None, None, DIMMING_AMOUNT_MAX).unwrap();

    let result = scope.get_light_channels("@all").unwrap();
    let u0 = if result[0].universe_id == "0" { 0 } else { 1 };
    let u1 = if result[0].universe_id == "2" { 0 } else { 1 };

    assert_eq!(result.len(), 2);
    assert_eq!(result[u0].universe_id, "0");
    assert_eq!(result[u0].channels.len(), 3);
    assert_eq!(
        result[u0].channels[0],
        ChannelDefinition {
            channel: 1,
            channel_type: ChannelType::Rgb
        }
    );
    assert_eq!(
        result[u0].channels[1],
        ChannelDefinition {
            channel: 4,
            channel_type: ChannelType::Rgb
        }
    );
    assert_eq!(
        result[u0].channels[2],
        ChannelDefinition {
            channel: 7,
            channel_type: ChannelType::Single
        }
    );
    assert_eq!(result[u1].universe_id, "2");
    assert_eq!(result[u1].channels.len(), 1);
    assert_eq!(
        result[u1].channels[0],
        ChannelDefinition {
            channel: 100,
            channel_type: ChannelType::TriWhite
        }
    );
}

#[test]
fn test_expand_values() {
    let mut array_manager = ArrayManager::new();
    let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "center": "rgb:1,rgb:4",
                    "frame": "s:7",
                    "spot": "$2,w:100",
                    "all": "@center,@spot,@frame"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": "`ticks`",
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                },
                "values": {
                    "test": "test-array-value",
                    "test2": "test2-array-value",
                    "ticks": "20"
                },
                "presets": [
                    {
                        "description": "Test preset",
                        "values": {
                            "preset1-value": "preset1-value-value",
                            "test2": "test2-preset-value"
                        }
                    }
                ]
            }"#;

    let array = serde_json::from_str::<DmxArray>(array_json).unwrap();
    array_manager.add_array("test".to_string(), array).unwrap();

    let scope = Scope::new(&array_manager, "test", None, None, DIMMING_AMOUNT_MAX).unwrap();
    let result = scope.expand_values("hello `test` world").unwrap();
    assert_eq!(result, "hello test-array-value world");

    let result = array_manager
        .expand_values(&scope, "hello `void=default` world")
        .unwrap();
    assert_eq!(result, "hello default world");

    let scope = Scope::new(&&array_manager, "test", Some(0), None, DIMMING_AMOUNT_MAX).unwrap();
    let result = scope.expand_values("hello `test2` world").unwrap();
    assert_eq!(result, "hello test2-preset-value world");

    let scope = Scope::new(
        &array_manager,
        "test",
        Some(0),
        Some(HashMap::from([(
            "test".to_string(),
            "test-local-value".to_string(),
        )])), DIMMING_AMOUNT_MAX
    )
    .unwrap();

    let result = scope.expand_values("hello `test` world").unwrap();
    assert_eq!(result, "hello test-local-value world");

    let result = scope.expand_values("hello `NONE` world");
    assert!(result.is_err());

    if let Err(e) = result {
        let t = e.to_string();
        assert_eq!(
            t,
            "Array 'test' preset# 0 'hello `NONE` world' has no value for NONE"
        );
    }

    let result = scope.expand_values("hello `NONE world");
    assert!(result.is_err());

    if let Err(e) = result {
        let t = e.to_string();
        assert_eq!(
            t,
            "Array 'test' 'hello `NONE world' has unterminated `value` expression"
        );
    }
}

#[test]
fn test_effect_management() {
    use crate::defs;
    let mut array_manager = ArrayManager::new();

    let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "all": "rgb:0"
                }
            }"#;

    let array = serde_json::from_str::<DmxArray>(array_json).unwrap();
    array_manager.add_array("test".to_string(), array).unwrap();

    let on_effect = array_manager.get_usage_effect_definition(&defs::EffectUsage::On, "test", None).unwrap();
    let t = format!("{:?}", on_effect);
    assert_eq!(t, r#"Fade(FadeEffectNodeDefinition { lights: "@all", ticks: Variable("`default_ticks=10`"), target: "`default_target=s(255);rgb(255,255,255);w(255,255,255)`" })"#);

    let _ = array_manager.get_usage_effect_runtime(&defs::EffectUsage::On, "test", None, None, DIMMING_AMOUNT_MAX).unwrap();

    let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "all": "rgb:0"
                },
                "effects": {
                    "simple_on": {
                        "type": "sequence",
                        "nodes": [
                            {
                                "type": "fade",
                                "lights": "@all",
                                "ticks": 10,
                                "target": "s(20); rgb(20,20,20); w(100, 20, 30)"
                            },
                            {
                                "type": "delay",
                                "ticks": 10
                            },
                            {
                                "type": "fade",
                                "lights": "@all",
                                "ticks": 10,
                                "target": "s(255); rgb(255,255,255); w(255, 255, 255)"
                            }
                        ]
                    }
                },
                "presets": [
                    {
                        "description": "Test preset",
                        "on": "simple_on"
                    }
                ]
            }"#;

    let array = serde_json::from_str::<DmxArray>(array_json).unwrap();
    array_manager.add_array("test".to_string(), array).unwrap();

    let d = array_manager.get_usage_effect_runtime(&defs::EffectUsage::On, "test", Some(0), None, DIMMING_AMOUNT_MAX).unwrap();
    let t = format!("{:?}", d);
    println!("{}", t);

    let d = array_manager.get_usage_effect_runtime(&defs::EffectUsage::On, "test", Some(0), None, 500).unwrap();
    let t = format!("{:?}", d);
    println!("{}", t);

}