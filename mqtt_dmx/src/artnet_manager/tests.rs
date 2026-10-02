#[cfg(test)]
mod test_universe {
    use crate::artnet_manager::manager::{ArtnetController, Universe, DMX_DATA_OFFSET};
    use crate::artnet_manager::ArtnetError;
    use crate::defs::UniverseDefinition;
    use crate::dmx::{ChannelDefinition, ChannelValue, DimmerValue};
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
            channel: ChannelDefinition::Single(0),
            value: DimmerValue::Single(255),
        };

        universe.set_channel(&channel_value).unwrap();
        let packet_bytes = universe.get_packet_bytes();
        assert_eq!(packet_bytes[DMX_DATA_OFFSET], 255);

        let channel_value = ChannelValue {
            channel: ChannelDefinition::Single(305),
            value: DimmerValue::Single(255),
        };

        assert!(universe.set_channel(&channel_value).is_ok());
        let packet_bytes = universe.get_packet_bytes();
        assert_eq!(packet_bytes[DMX_DATA_OFFSET + 305], 255);

        let channel_value = ChannelValue {
            channel: ChannelDefinition::Rgb(0, 1, 2),
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
            channel: ChannelDefinition::Single(0),
            value: DimmerValue::Single(255),
        };

        universe.set_channel(&channel_value).unwrap();
        let channel_value = universe.get_channel(&ChannelDefinition::Single(0)).unwrap();
        assert_eq!(channel_value.value, DimmerValue::Single(255));
        assert_eq!(channel_value.channel, ChannelDefinition::Single(0));

        let channel_value = ChannelValue {
            channel: ChannelDefinition::TriWhite(10, 11, 12),
            value: DimmerValue::TriWhite(19, 23, 30),
        };
        universe.set_channel(&channel_value).unwrap();
        let channel_value = universe
            .get_channel(&ChannelDefinition::TriWhite(10, 11, 12))
            .unwrap();
        assert_eq!(channel_value.value, DimmerValue::TriWhite(19, 23, 30));
        assert_eq!(
            channel_value.channel,
            ChannelDefinition::TriWhite(10, 11, 12)
        );
    }

    #[test]
    fn test_set_error_handling() {
        let mut universe = get_universe("test");

        let channel_value = ChannelValue {
            channel: ChannelDefinition::Single(305),
            value: DimmerValue::Single(255),
        };
        assert!(universe.set_channel(&channel_value).is_ok());

        let channel_value = ChannelValue {
            channel: ChannelDefinition::Rgb(305, 306, 307),
            value: DimmerValue::Rgb(10, 11, 12),
        };
        let result = universe.set_channel(&channel_value);

        match &result {
            Err(report) => match report.current_context() {
                ArtnetError::InvalidChannel(d, 306, 306) if d == "test (Test Universe)" => {}
                other => panic!("Expected InvalidChannel error, got {:?}", other),
            },
            _ => panic!("Expected InvalidChannel error, got {:?}", result),
        }
    }

    #[test]
    fn test_get_error_handling() {
        let universe = get_universe("test");

        assert!(universe
            .get_channel(&ChannelDefinition::Single(305))
            .is_ok());
        let result = universe.get_channel(&ChannelDefinition::Rgb(306, 100, 200));

        match &result {
            Err(report) => match report.current_context() {
                ArtnetError::InvalidChannel(d, 306, 306) if d == "test (Test Universe)" => {}
                other => panic!("Expected InvalidChannel error, got {:?}", other),
            },
            _ => panic!("Expected InvalidChannel error, got {:?}", result),
        }
    }
}

#[cfg(test)]
mod test_artnet_manager {
    use crate::{
        artnet_manager::{ArtnetError, ArtnetManager, EffectNodeRuntime},
        defs::UniverseDefinition,
        dmx::{ChannelDefinition, ChannelValue, DimmerValue},
        messages::{ToArtnetManagerMessage, ToMqttPublisherMessage},
    };

    use error_stack::Report;
    use std::{net::IpAddr, str::FromStr, sync::Arc, time::Duration};
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
        let (to_artnet_manager_sender, to_artnet_manager_receiver) =
            tokio::sync::mpsc::channel::<ToArtnetManagerMessage>(10);
        let (to_mqtt_publisher_sender, _) =
            async_channel::bounded::<ToMqttPublisherMessage>(10);

        tokio::spawn(async move {
            let mut manager = ArtnetManager::new();
            manager
                .run(cancel, to_artnet_manager_receiver, to_mqtt_publisher_sender)
                .await;
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
        assert!(manager.universes.is_empty());
        assert!(manager.controllers.is_empty());
    }

    #[test]
    fn test_universe_set() {
        let mut manager = ArtnetManager::new();

        let universe_definition = get_universe_definition();
        assert!(manager.add_universe("test", universe_definition).is_ok());

        let channel_value = ChannelValue {
            channel: ChannelDefinition::Single(5),
            value: DimmerValue::Single(255),
        };

        assert!(manager.set_channel("test", &channel_value).is_ok());
        let v = manager
            .get_channel("test", &ChannelDefinition::Single(5))
            .unwrap();
        assert_eq!(v.value, DimmerValue::Single(255));

        let channel_value = ChannelValue {
            channel: ChannelDefinition::Rgb(10, 11, 12),
            value: DimmerValue::Rgb(3, 5, 8),
        };

        assert!(manager.set_channel("test", &channel_value).is_ok());
        let v = manager
            .get_channel("test", &ChannelDefinition::Rgb(10, 11, 12))
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
                Arc::from("test"),
                universe_definition,
                tx,
            ))
            .await
            .unwrap();
        let result = rx.await.unwrap();
        cancel.cancel();
        assert!(result.is_ok());
    }

    /// An effect whose every tick fails. Its first failure stops it (`ArtnetManager::tick`), so it
    /// makes one error report, and the effects beside it run on.
    #[derive(Debug)]
    struct FailsItsTick(usize);

    impl EffectNodeRuntime for FailsItsTick {
        fn tick(&mut self, _: &mut ArtnetManager) -> Result<(), Report<ArtnetError>> {
            Err(Report::new(ArtnetError::Context(format!("test effect {} failed", self.0))))
        }

        fn is_done(&self) -> bool {
            false
        }
    }

    /// Asks the manager something and waits a bounded time for its answer.
    async fn answers(sender: &Sender<ToArtnetManagerMessage>, message: impl FnOnce(ReplyTx) -> ToArtnetManagerMessage) -> bool {
        let (tx, rx) = tokio::sync::oneshot::channel();
        sender.send(message(tx)).await.expect("the ArtNet manager's queue is open");
        tokio::time::timeout(Duration::from_secs(2), rx).await.is_ok()
    }

    type ReplyTx = tokio::sync::oneshot::Sender<Result<(), Report<ArtnetError>>>;

    /// Reports dropped from a full error queue are one WARN per episode, its own kind, each report
    /// at DEBUG, and an INFO with the episode's total once the publisher has drained the queue
    /// (Store no-hang 3b review, C-25: it was one WARN per dropped report, 20 a second during a
    /// failing fade).
    #[test]
    fn a_full_error_queue_is_one_warn_per_episode_and_an_info_with_its_total_once_drained() {
        use crate::artnet_manager::manager::Reporter;
        use tracing::Level;

        let (to_mqtt_publisher, reports) = async_channel::bounded::<ToMqttPublisherMessage>(2);
        let mut reporter = Reporter::new(to_mqtt_publisher);
        let events = crate::test_log::capture(|| {
            for dropped in [5, 2] {
                // Two fill the queue; each one after that displaces the oldest.
                for n in 0..2 + dropped {
                    assert!(reporter.report(format!("error {n}")));
                    reporter.tick();
                }
                // The publisher catches up.
                while reports.try_recv().is_ok() {}
                reporter.tick();
            }
        });

        let totals: Vec<_> = events
            .iter()
            .filter(|e| e.level == Level::INFO && e.kind() == Some("error_report_drop_ended"))
            .map(|e| e.field("dropped").unwrap_or_default().to_string())
            .collect();
        assert_eq!(totals, ["5", "2"], "no INFO with each episode's total once the queue drained");
        let warns: Vec<_> = events.iter().filter(|e| e.level <= Level::WARN).collect();
        assert_eq!(warns.len(), 2, "not one WARN per episode of dropped reports: {warns:#?}");
        assert!(warns.iter().all(|w| w.kind() == Some("error_report_dropped")), "a WARN without its own kind: {warns:#?}");
        let each = events.iter().filter(|e| e.level == Level::DEBUG && e.message.contains("dropped")).count();
        assert_eq!(each, 7, "the dropped reports are not each at DEBUG");
    }

    /// An effect that counts its ticks and never finishes, like a long fade.
    #[derive(Debug)]
    struct CountsItsTicks(Arc<std::sync::atomic::AtomicUsize>);

    impl EffectNodeRuntime for CountsItsTicks {
        fn tick(&mut self, _: &mut ArtnetManager) -> Result<(), Report<ArtnetError>> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        fn is_done(&self) -> bool {
            false
        }
    }

    /// One effect failing its tick stops that effect alone, reported once: every other running
    /// effect keeps ticking (Store no-hang 3b review, C-26 — the tick returned at the first
    /// failure, and the effects it had taken out to run were never put back).
    #[test]
    fn a_failing_effect_does_not_stop_the_others() {
        const TICKS: usize = 4;
        let mut manager = ArtnetManager::new();
        let ticked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        manager.start_effect("healthy", Box::new(CountsItsTicks(ticked.clone()))).unwrap();
        manager.start_effect("failing", Box::new(FailsItsTick(0))).unwrap();
        let failed: Vec<String> = (0..TICKS).flat_map(|_| manager.tick()).map(|(id, _)| id).collect();
        assert_eq!(
            ticked.load(std::sync::atomic::Ordering::SeqCst),
            TICKS,
            "a failing effect stopped the healthy one: it ticked fewer than {TICKS} times"
        );
        assert_eq!(failed, ["failing"], "the failing effect was not reported once, then stopped");
    }

    /// The ArtNet manager never waits on MQTT (Store no-hang §14.3): with the error queue full —
    /// the broker slow, or the bridge between MQTT sessions, so nobody takes from it — it keeps
    /// ticking and answering, and the newest reports displace the oldest.
    #[tokio::test]
    async fn the_ticker_keeps_answering_while_the_mqtt_error_queue_is_full() {
        const QUEUE: usize = 2;
        let cancel = CancellationToken::new();
        let (sender, receiver) = tokio::sync::mpsc::channel::<ToArtnetManagerMessage>(10);
        // Nobody reads this queue.
        let (to_mqtt_publisher, reports) = async_channel::bounded::<ToMqttPublisherMessage>(QUEUE);
        let manager_cancel = cancel.clone();
        tokio::spawn(async move {
            ArtnetManager::new().run(manager_cancel, receiver, to_mqtt_publisher).await;
        });
        const STALLED: &str = "the ArtNet manager stopped answering while the MQTT error queue was full — its ticker waited on MQTT";

        for n in 0..=QUEUE {
            let effect = Box::new(FailsItsTick(n));
            assert!(answers(&sender, |tx| ToArtnetManagerMessage::StartEffect(Arc::from(format!("failing-{n}")), effect, tx)).await, "{STALLED}");
            if n < QUEUE {
                // Its failed tick's report is queued.
                let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
                while reports.len() < n + 1 && tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                assert_eq!(reports.len(), n + 1, "the failed tick of effect {n} reported no error");
            } else {
                // A few ticks: one more report, with the queue already full.
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        assert!(answers(&sender, |tx| ToArtnetManagerMessage::StopEffect(Arc::from("failing-0"), tx)).await, "{STALLED}");

        let queued: Vec<String> = std::iter::from_fn(|| reports.try_recv().ok())
            .map(|ToMqttPublisherMessage::Error(e)| e)
            .collect();
        assert!(
            queued.len() == QUEUE && queued[0].contains("test effect 1") && queued[1].contains("test effect 2"),
            "the error queue kept {queued:?}, not the newest reports"
        );
        cancel.cancel();
    }
}

#[cfg(test)]
mod test_effect_nodes {
    use std::{net::IpAddr, str::FromStr, sync::Arc};

    use crate::{
        array_manager::ArrayManager,
        artnet_manager::{ArtnetManager, EffectNodeRuntime},
        defs,
        defs::{DmxArray, UniverseDefinition},
        dmx::{ChannelValue, DimmerValue, ChannelDefinition},
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

        array_manager.add_array(Arc::from("test"), Box::new(array)).unwrap();
        let node = array_manager
            .get_usage_effect_runtime(
                &defs::EffectUsage::On,
                "test",
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
            [
                ChannelValue {
                    channel: ChannelDefinition::Single(0),
                    value: DimmerValue::Single(64)
                },
                ChannelValue {
                    channel: ChannelDefinition::Single(0),
                    value: DimmerValue::Single(128)
                },
                ChannelValue {
                    channel: ChannelDefinition::Single(0),
                    value: DimmerValue::Single(191)
                },
                ChannelValue {
                    channel: ChannelDefinition::Single(0),
                    value: DimmerValue::Single(255)
                },
            ]
        );

        let node = array_manager
            .get_usage_effect_runtime(
                &defs::EffectUsage::Off,
                "test",
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
            [
                ChannelValue {
                    channel: ChannelDefinition::Single(0),
                    value: DimmerValue::Single(223)
                },
                ChannelValue {
                    channel: ChannelDefinition::Single(0),
                    value: DimmerValue::Single(191)
                },
                ChannelValue {
                    channel: ChannelDefinition::Single(0),
                    value: DimmerValue::Single(159)
                },
                ChannelValue {
                    channel: ChannelDefinition::Single(0),
                    value: DimmerValue::Single(127)
                },
                ChannelValue {
                    channel: ChannelDefinition::Single(0),
                    value: DimmerValue::Single(96)
                },
                ChannelValue {
                    channel: ChannelDefinition::Single(0),
                    value: DimmerValue::Single(64)
                },
                ChannelValue {
                    channel: ChannelDefinition::Single(0),
                    value: DimmerValue::Single(32)
                },
                ChannelValue {
                    channel: ChannelDefinition::Single(0),
                    value: DimmerValue::Single(0)
                },
            ]
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

        array_manager.add_array(Arc::from("test"), Box::new(array)).unwrap();

        let node = array_manager
            .get_usage_effect_runtime(
                &defs::EffectUsage::On,
                "test",
                None,
                800, // 80% dimming
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
                defs::DIMMING_AMOUNT_MAX, // 100% dimming
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
                100, // 10% dimming
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
                0, // 0% dimming
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
