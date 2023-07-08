#[cfg(test)]
mod test_universe {
    use crate::artnet_manager::manager::{ArtnetController, Universe, DMX_DATA_OFFSET};
    use crate::artnet_manager::ArtnetError;
    use crate::defs::UniverseDefinition;
    use crate::dmx::{ChannelDefinition, ChannelType, ChannelValue, DimmerValue};
    use std::{net::IpAddr, str::FromStr, sync::Arc};

    fn get_universe_definition() -> UniverseDefinition {
        UniverseDefinition {
            description: "Test Universe".to_string(),
            controller: IpAddr::from_str("10.0.1.228").unwrap(),
            net: 0,
            subnet: 0,
            universe: 0,
            channels: 306,
            log: false,
            disable_send: true,
        }
    }

    fn get_universe(universe_id: &str) -> Universe {
        let controller =
            Arc::new(ArtnetController::new(&IpAddr::from_str("10.0.1.228").unwrap()).unwrap());
        Universe::new(controller, universe_id, get_universe_definition()).unwrap()
    }

    #[test]
    fn test_universe_new() {
        let universe = get_universe("test");

        assert_eq!(universe.get_packet_bytes().len(), 306 + DMX_DATA_OFFSET);
    }

    #[test]
    fn test_set_channel() {
        let mut universe = get_universe("test");

        let channel_value = ChannelValue {
            channel: 0,
            value: DimmerValue::Single(255),
        };

        universe.set_channel(&channel_value).unwrap();
        let packet_bytes = universe.get_packet_bytes();
        assert_eq!(packet_bytes[DMX_DATA_OFFSET], 255);

        let channel_value = ChannelValue {
            channel: 305,
            value: DimmerValue::Single(255),
        };

        assert!(universe.set_channel(&channel_value).is_ok());
        let packet_bytes = universe.get_packet_bytes();
        assert_eq!(packet_bytes[DMX_DATA_OFFSET + 305], 255);

        let channel_value = ChannelValue {
            channel: 0,
            value: DimmerValue::Rgb(10, 20, 30),
        };

        assert!(universe.set_channel(&channel_value).is_ok());
        let packet_bytes = universe.get_packet_bytes();
        assert_eq!(packet_bytes[DMX_DATA_OFFSET], 10);
        assert_eq!(packet_bytes[DMX_DATA_OFFSET + 1], 20);
        assert_eq!(packet_bytes[DMX_DATA_OFFSET + 2], 30);
    }

    #[test]
    fn test_get_channel() {
        let mut universe = get_universe("test");

        let channel_value = ChannelValue {
            channel: 0,
            value: DimmerValue::Single(255),
        };

        universe.set_channel(&channel_value).unwrap();
        let channel_value = universe
            .get_channel(&ChannelDefinition {
                channel: 0,
                channel_type: ChannelType::Single,
            })
            .unwrap();
        assert_eq!(channel_value.value, DimmerValue::Single(255));
        assert_eq!(channel_value.channel, 0);

        let channel_value = ChannelValue {
            channel: 10,
            value: DimmerValue::TriWhite(19, 23, 30),
        };
        universe.set_channel(&channel_value).unwrap();
        let channel_value = universe
            .get_channel(&ChannelDefinition {
                channel: 10,
                channel_type: ChannelType::TriWhite,
            })
            .unwrap();
        assert_eq!(channel_value.value, DimmerValue::TriWhite(19, 23, 30));
        assert_eq!(channel_value.channel, 10);
    }

    #[test]
    fn test_set_error_handling() {
        let mut universe = get_universe("test");

        let channel_value = ChannelValue {
            channel: 305,
            value: DimmerValue::Single(255),
        };
        assert!(universe.set_channel(&channel_value).is_ok());

        let channel_value = ChannelValue {
            channel: 305,
            value: DimmerValue::Rgb(10, 11, 12),
        };
        let result = universe.set_channel(&channel_value);

        match result {
            Err(ArtnetError::InvalidChannel(d, 305, 306)) if d == "test (Test Universe)" => {}
            _ => panic!("Expected InvalidChannel error, got {:?}", result),
        }
    }

    #[test]
    fn test_get_error_handling() {
        let universe = get_universe("test");

        assert!(universe
            .get_channel(&ChannelDefinition {
                channel: 305,
                channel_type: ChannelType::Single
            })
            .is_ok());
        let result = universe.get_channel(&ChannelDefinition {
            channel: 305,
            channel_type: ChannelType::Rgb,
        });

        match result {
            Err(ArtnetError::InvalidChannel(d, 305, 306)) if d == "test (Test Universe)" => {}
            _ => panic!("Expected InvalidChannel error, got {:?}", result),
        }
    }
}

#[cfg(test)]
mod test_artnet_manager {
    use crate::{
        artnet_manager::ArtnetManager,
        defs::UniverseDefinition,
        dmx::{ChannelDefinition, ChannelType, ChannelValue, DimmerValue},
        messages::{ToArtnetManagerMessage, ToMqttPublisherMessage},
    };

    use std::{net::IpAddr, str::FromStr};
    use tokio::sync::mpsc::Sender;
    use tokio_util::sync::CancellationToken;

    fn get_universe_definition() -> UniverseDefinition {
        UniverseDefinition {
            description: "Test Universe".to_string(),
            controller: IpAddr::from_str("10.0.1.228").unwrap(),
            net: 0,
            subnet: 0,
            universe: 0,
            channels: 306,
            log: false,
            disable_send: true,
        }
    }

    fn start_artnet_manager(cancel: CancellationToken) -> Sender<ToArtnetManagerMessage> {
        let (to_artnet_manager_sender, to_artnet_manager_receiver) = tokio::sync::mpsc::channel::<ToArtnetManagerMessage>(10);
        let (to_mqtt_publisher_sender, _) = tokio::sync::mpsc::channel::<ToMqttPublisherMessage>(10);
        
        tokio::spawn(async move {
            let mut manager = ArtnetManager::new();
            manager.run(cancel, to_artnet_manager_receiver, to_mqtt_publisher_sender).await;
        });

        to_artnet_manager_sender
    }

    #[test]
    fn test_add_universe() {
        let mut manager = ArtnetManager::new();

        let universe_definition = get_universe_definition();
        assert!(manager.add_universe("test", universe_definition).is_ok());
        assert!(manager.controllers.len() == 1);
        assert!(manager.controllers.len() == 1);
    }

    #[test]
    fn test_remove_universe() {
        let mut manager = ArtnetManager::new();

        let universe_definition = get_universe_definition();
        assert!(manager.add_universe("test1", universe_definition).is_ok());
        assert!(manager.controllers.len() == 1);
        assert!(manager.universes.len() == 1);

        let universe_definition = get_universe_definition();
        assert!(manager.add_universe("test2", universe_definition).is_ok());
        assert!(manager.controllers.len() == 1);
        assert!(manager.universes.len() == 2);

        // Remove universe and ensure that the controller is still there
        assert!(manager.remove_universe("test1").is_ok());
        assert!(manager.universes.len() == 1);
        assert!(manager.controllers.len() == 1);

        // Remove the second universe and ensure that the controller is gone
        assert!(manager.remove_universe("test2").is_ok());
        assert!(manager.universes.len() == 0);
        assert!(manager.controllers.len() == 0);
    }

    #[test]
    fn test_universe_set() {
        let mut manager = ArtnetManager::new();

        let universe_definition = get_universe_definition();
        assert!(manager.add_universe("test", universe_definition).is_ok());

        let channel_value = ChannelValue {
            channel: 5,
            value: DimmerValue::Single(255),
        };

        assert!(manager.set_channel("test", &channel_value).is_ok());
        let v = manager
            .get_channel(
                "test",
                &ChannelDefinition {
                    channel: 5,
                    channel_type: ChannelType::Single,
                },
            )
            .unwrap();
        assert_eq!(v.value, DimmerValue::Single(255));

        let channel_value = ChannelValue {
            channel: 10,
            value: DimmerValue::Rgb(3, 5, 8),
        };

        assert!(manager.set_channel("test", &channel_value).is_ok());
        let v = manager
            .get_channel(
                "test",
                &ChannelDefinition {
                    channel: 10,
                    channel_type: ChannelType::Rgb,
                },
            )
            .unwrap();
        assert_eq!(v.value, DimmerValue::Rgb(3, 5, 8));
    }

    #[tokio::test]
    async fn test_messaging() {
        let cancel = CancellationToken::new();
        let sender = start_artnet_manager(cancel.clone());
        let universe_definition = get_universe_definition();
        let (tx, rx) = tokio::sync::oneshot::channel();

        sender
            .send(ToArtnetManagerMessage::AddUniverse(
                "test".to_string(),
                universe_definition,
                tx,
            ))
            .await
            .unwrap();
        let result = rx.await.unwrap();
        cancel.cancel();
        assert!(result.is_ok());
    }
}

#[cfg(test)]
mod test_effect_nodes {
    use std::{net::IpAddr, str::FromStr};

    use crate::{
        array_manager::ArrayManager,
        artnet_manager::{ArtnetManager, EffectNodeRuntime},
        defs,
        defs::{DmxArray, UniverseDefinition},
        dmx::{ChannelValue, DimmerValue},
    };

    fn get_universe_definition() -> UniverseDefinition {
        UniverseDefinition {
            description: "Test Universe".to_string(),
            controller: IpAddr::from_str("10.0.1.228").unwrap(),
            net: 0,
            subnet: 0,
            universe: 0,
            channels: 306,
            log: true,
            disable_send: false,
        }
    }

    #[test]
    fn test_fade_node1() {
        let array_json = r#"
        {
            "universe_id": "0",
            "description": "Test array",
            "lights": {
                "all": "s:0"
            },
            "effects": {
                "on": {
                    "type": "fade",
                    "lights": "@all",
                    "ticks": 4,
                    "target": "s(255); rgb(255,255,255); w(255,255,255)"
                },
                "off": {
                    "type": "fade",
                    "lights": "@all",
                    "ticks": 8,
                    "target": "s(0); rgb(0,0,0); w(0,0,0)"
                }
            }
        }"#;

        let mut array_manager = ArrayManager::new();
        let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

        let mut artnet_manager = ArtnetManager::new();
        artnet_manager
            .add_universe("0", get_universe_definition())
            .unwrap();

        array_manager.add_array("test", array).unwrap();
        let node = array_manager
            .get_usage_effect_runtime(
                &defs::EffectUsage::On,
                "test",
                None,
                None,
                defs::DIMMING_AMOUNT_MAX,
            )
            .unwrap();

        println!("{:?}", node);

        run_node(node, &mut artnet_manager);
        println!("{:?}", artnet_manager.set_channel_log);

        assert_eq!(artnet_manager.set_channel_log.len(), 4);
        assert_eq!(
            artnet_manager.set_channel_log,
            [ChannelValue {
                channel: 0,
                value: DimmerValue::Single(64)
            },
            ChannelValue {
                channel: 0,
                value: DimmerValue::Single(128)
            },
            ChannelValue {
                channel: 0,
                value: DimmerValue::Single(191)
            },
            ChannelValue {
                channel: 0,
                value: DimmerValue::Single(255)
            },]
        );

        let node = array_manager
            .get_usage_effect_runtime(
                &defs::EffectUsage::Off,
                "test",
                None,
                None,
                defs::DIMMING_AMOUNT_MAX,
            )
            .unwrap();

        println!("{:?}", node);

        run_node(node, &mut artnet_manager);
        println!("{:?}", artnet_manager.set_channel_log);

        assert_eq!(artnet_manager.set_channel_log.len(), 8);
        assert_eq!(
            artnet_manager.set_channel_log,
            [ChannelValue {
                channel: 0,
                value: DimmerValue::Single(223)
            },
            ChannelValue {
                channel: 0,
                value: DimmerValue::Single(191)
            },
            ChannelValue {
                channel: 0,
                value: DimmerValue::Single(159)
            },
            ChannelValue {
                channel: 0,
                value: DimmerValue::Single(127)
            },
            ChannelValue {
                channel: 0,
                value: DimmerValue::Single(96)
            },
            ChannelValue {
                channel: 0,
                value: DimmerValue::Single(64)
            },
            ChannelValue {
                channel: 0,
                value: DimmerValue::Single(32)
            },
            ChannelValue {
                channel: 0,
                value: DimmerValue::Single(0)
            },]
        );
    }

    #[test]
    fn test_fade_node2() {
        let array_json = r#"
        {
            "universe_id": "0",
            "description": "Test array",
            "lights": {
                "all": "rgb:0"
            },
            "effects": {
                "on": {
                    "type": "fade",
                    "lights": "@all",
                    "ticks": 4,
                    "target": "s(255); rgb(255,255,255); w(255,255,255)"
                },
                "off": {
                    "type": "fade",
                    "lights": "@all",
                    "ticks": 8,
                    "target": "s(0); rgb(0,0,0); w(0,0,0)"
                }
            }
        }"#;

        let mut array_manager = ArrayManager::new();
        let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

        let mut artnet_manager = ArtnetManager::new();
        artnet_manager
            .add_universe("0", get_universe_definition())
            .unwrap();

        array_manager.add_array("test", array).unwrap();

        let node = array_manager
            .get_usage_effect_runtime(
                &defs::EffectUsage::On,
                "test",
                None,
                None,
                800,        // 80% dimming
            )
            .unwrap();

        println!("{:?}", node);

        run_node(node, &mut artnet_manager);
        println!("{:?}", artnet_manager.set_channel_log);

        let node = array_manager
            .get_usage_effect_runtime(
                &defs::EffectUsage::On,
                "test",
                None,
                None,
                defs::DIMMING_AMOUNT_MAX,        // 100% dimming
            )
            .unwrap();

        println!("{:?}", node);

        run_node(node, &mut artnet_manager);
        println!("{:?}", artnet_manager.set_channel_log);

        let node = array_manager
            .get_usage_effect_runtime(
                &defs::EffectUsage::On,
                "test",
                None,
                None,
                100,        // 10% dimming
            )
            .unwrap();

        println!("{:?}", node);

        run_node(node, &mut artnet_manager);
        println!("{:?}", artnet_manager.set_channel_log);

        let node = array_manager
            .get_usage_effect_runtime(
                &defs::EffectUsage::On,
                "test",
                None,
                None,
                0,        // 0% dimming
            )
            .unwrap();

        println!("{:?}", node);

        run_node(node, &mut artnet_manager);
        println!("{:?}", artnet_manager.set_channel_log);

    }

    fn run_node(mut node: Box<dyn EffectNodeRuntime>, artnet_manager: &mut ArtnetManager) {
        let mut loop_limit = 100;

        artnet_manager.set_channel_log.clear();
        while !node.is_done() {
            node.tick(artnet_manager).unwrap();

            loop_limit -= 1;
            if loop_limit <= 0 {
                panic!("Loop limit exceeded");
            }
        }
    }
}
