use std::sync::Arc;

use super::*;
use crate::defs::{DmxArray, DIMMING_AMOUNT_MAX, SymbolTable};
use crate::dmx::ChannelDefinition;

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

    array_manager
        .add_array(Arc::from("test"), Box::new(array))
        .unwrap();

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

    array_manager
        .add_array(Arc::from("test2"), Box::new(array))
        .unwrap();

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

    if let Err(e) = array_manager.add_array(Arc::from("test3"), Box::new(array)) {
        let t = e.to_string();
        assert_eq!(
            t,
            "Array 'test3' Lights @all -> @center,@spot,@frame does not contain definition for center"
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

    array_manager
        .add_array(Arc::from("test2"), Box::new(array))
        .unwrap();

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

    if let Err(e) = array_manager.add_array(Arc::from("test2"), Box::new(array)) {
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

    if let Err(e) = array_manager.add_array(Arc::from("test2"), Box::new(array)) {
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

    if let Err(e) = array_manager.add_array(Arc::from("test2"), Box::new(array)) {
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

    if let Err(e) = array_manager.add_array(Arc::from("test2"), Box::new(array)) {
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
    array_manager
        .add_array(Arc::from("test"), Box::new(array))
        .unwrap();
    let scope = Scope::new(
        &array_manager,
        Arc::from("test"),
        None,
        DIMMING_AMOUNT_MAX,
    )
    .unwrap();

    let result = scope.get_light_channels("@all").unwrap();
    let u0 = if result[0].universe_id == "0" { 0 } else { 1 };
    let u1 = if result[0].universe_id == "2" { 0 } else { 1 };

    assert_eq!(result.len(), 2);
    assert_eq!(result[u0].universe_id, "0");
    assert_eq!(result[u0].channels.len(), 3);
    assert_eq!(result[u0].channels[0], ChannelDefinition::Rgb(1, 2, 3));
    assert_eq!(result[u0].channels[1], ChannelDefinition::Rgb(4, 5, 6));
    assert_eq!(result[u0].channels[2], ChannelDefinition::Single(7));
    assert_eq!(result[u1].universe_id, "2");
    assert_eq!(result[u1].channels.len(), 1);
    assert_eq!(
        result[u1].channels[0],
        ChannelDefinition::TriWhite(100, 101, 102)
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
                }
            }"#;

    let array = serde_json::from_str::<DmxArray>(array_json).unwrap();
    let array_id = <Arc<str>>::from("test");
    array_manager
        .add_array(array_id.clone(), Box::new(array))
        .unwrap();

    // "values": {
    //     "test": "test-array-value",
    //     "test2": "test2-array-value",
    //     "ticks": "20"
    // }
    let values: SymbolTable = [
        (Arc::from("test"), "test-local-value".to_string()),
        (Arc::from("test2"), "test2-local-value".to_string()),
        (Arc::from("ticks"), "20".to_string()),
    ].iter().cloned().collect();

    array_manager.initialize_array_values(array_id.clone(), values).unwrap();

    let scope = Scope::new(
        &array_manager,
        array_id.clone(),
        None,
        DIMMING_AMOUNT_MAX,
    )
    .unwrap();
    let result = scope.expand_values("hello `test` world").unwrap();
    assert_eq!(result, "hello test-local-value world");


    let result = array_manager
        .expand_values(array_id.clone(), "hello `void=default` world")
        .unwrap();
    assert_eq!(result, "hello default world");

    let scope = Scope::new(
        &&array_manager,
        array_id.clone(),
        None,
        DIMMING_AMOUNT_MAX,
    )
    .unwrap();
    let result = scope.expand_values("hello `test2` world").unwrap();
    assert_eq!(result, "hello test2-local-value world");

    let scope = Scope::new(
        &array_manager,
        array_id.clone(),
        None,
        DIMMING_AMOUNT_MAX,
    )
    .unwrap();

    let result = scope.expand_values("hello `test` world").unwrap();
    assert_eq!(result, "hello test-local-value world");

    let result = scope.expand_values("hello `NONE` world");
    assert!(result.is_err());

    if let Err(e) = result {
        let t = e.to_string();
        assert_eq!(t, "Array 'test' 'hello `NONE` world' has no value for NONE");
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
    array_manager
        .add_array(Arc::from("test"), Box::new(array))
        .unwrap();

    let on_effect = array_manager
        .get_usage_effect_definition(&defs::EffectUsage::On, "test", None)
        .unwrap();
    let t = format!("{:?}", on_effect);
    assert_eq!(
        t,
        r#"Fade(FadeEffectNodeDefinition { lights: "@all", ticks: Variable("`on_ticks=10`"), target: "`target=s(255);rgb(255,255,255);w(255,255,255)`", no_dimming: false })"#
    );

    let _ = array_manager
        .get_usage_effect_runtime(
            &defs::EffectUsage::On,
            "test",
            None,
            DIMMING_AMOUNT_MAX,
        )
        .unwrap();

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
                }
            }"#;

    let array = serde_json::from_str::<DmxArray>(array_json).unwrap();
    array_manager
        .add_array(Arc::from("test"), Box::new(array))
        .unwrap();

    let d = array_manager
        .get_usage_effect_runtime(
            &defs::EffectUsage::On,
            "test",
            Some(&Arc::from("simple_on")),
            DIMMING_AMOUNT_MAX,
        )
        .unwrap();
    let t = format!("{:?}", d);
    println!("{}", t);

    let d = array_manager
        .get_usage_effect_runtime(
            &defs::EffectUsage::On,
            "test",
            Some(&Arc::from("simple_on")),
            500,
        )
        .unwrap();
    let t = format!("{:?}", d);
    println!("{}", t);
}
