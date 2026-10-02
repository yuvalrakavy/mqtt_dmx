# The wait registry

Every place mqtt_dmx waits on something else — a lock, a channel, the broker, a dependency's
`async fn` — carries a tag naming a row here:

```text
// WAIT: mqtt-pump-queue
let event = self.rx.recv().await;
```

The tag goes on its own line, directly above its statement: rustfmt moves a tag that trails a
`{` into the block, where it covers no wait (`tests/wait_registry.rs` refuses a trailing tag).
The row says, once, why that wait cannot hang the bridge: **`acyclic`** (nothing it waits on can
wait back on any of its waiters) or **`bounded`** (it ends within a stated bound, and the row says
what happens on expiry). The rule and the tag format are Store's no-hang spec, §13.3 and §14
(`Store/docs/superpowers/specs/2026-09-30-no-hang-wait-graph-design.md`); the check is the
`wait-lint` crate, run by `tests/wait_registry.rs`.

It does not prove the order locks are taken in, nor see a wait reached through a call to this
crate's own `async fn`; the arguments are checked by reading, and the waiters block below makes
every new waiting function a reviewed diff. Regenerate it after a change, from a checkout of
tracing-init:

```text
cargo run --manifest-path wait-lint/Cargo.toml -- --root <mqtt_dmx>/mqtt_dmx --src src --registry docs/wait-registry.md --write
```

## The tasks

The **pump** (`Pump::start`) polls rumqttc's event loop and forwards incoming publishes; the
**subscriber** (`mqtt_subscriber::session`) handles them, asking the ArtNet and array managers and
queueing error reports; the **publisher** (`mqtt_publisher::session`) publishes those reports; the
**ArtNet manager** ticks every 50 ms, sending DMX and queueing its own error reports; the **array
manager** answers questions about arrays and effects. `Service::mqtt_session` runs one connection,
and `Service::mqtt` reconnects. The wait graph runs subscriber → managers, subscriber → publisher →
request channel → pump, and nothing waits back on the subscriber; the pump waits on nothing here.

## The rows

| Key | Kind | Waits on | Argument |
|---|---|---|---|
| `mqtt-request` | acyclic | rumqttc's request channel (`publish`, `publish_with_properties`, `subscribe`) | The channel drains only while the event loop is polled, and the pump polls it in a task of its own that waits on nothing but the network. Its waiters — `Service::announce` and the publisher — are never waited on by the pump, so a full channel is back-pressure on them, never a cycle: with the broker stalled they wait until it recovers, the pump reading its acks meanwhile. When the connection fails the pump drops the event loop, and a waiting publish fails instead (`a_burst_of_failing_commands_against_a_stalled_broker_reports_every_error_once_it_recovers`; on the old session, whose poller queued its own error reports, 1 of 100 arrived). |
| `mqtt-poll` | acyclic | the broker, over the network | The pump waits on nothing in this process, so nothing here can wait back on it. How long one poll takes is up to the network, and only partly bounded: a connect and its CONNACK end within rumqttc's 5 s connection timeout, and a peer that stops answering fails the poll at the second unanswered keep-alive ping (about 10 s at the 5 s keep-alive), when the pump hands the subscriber `Ended` and stops. But rumqttc awaits its network write and flush inside its `select!` with no timeout, so a peer that keeps the connection open and stops reading (its receive window closed) holds the poll until the kernel's TCP timeouts end the connection — minutes, and no bound at all while a live peer host keeps answering zero-window probes. This bridge has few bytes outstanding (QoS 1 publishes capped by the broker's receive maximum, pings, acks), so a full socket buffer is unlikely, not impossible. While it lasts the session stalls — the publisher waits on the request channel, the subscriber on the forward queue — and nothing outside the session waits on it: the ArtNet manager never waits on MQTT, and shutdown aborts the session's tasks. |
| `mqtt-pump-queue` | acyclic | the pump's forward queue | Unbounded (no-hang §14.6): the pump never waits on the subscriber, so the subscriber's wait ends with the next message, or with `Ended` when the connection fails — the session then ends and `Service::mqtt` reconnects. Past 1000 unread publishes it is a WARN (`mqtt_backlog_high`), never a drop. |
| `mqtt-backlog-lock` | acyclic | the forward queue's high-water flag | Held to read or set one timestamp; nothing waits under it. |
| `error-queue-send` | acyclic | room in the error queue to the publisher (10 reports) | Only the subscriber waits here. The publisher that drains the queue waits only on the request channel (`mqtt-request`), which the pump drains, and nothing on that path waits on the subscriber. A session that ends while it waits aborts it (`mqtt_session` shuts both workers down), so it never outlives the publisher. |
| `error-queue-recv` | acyclic | a report in the error queue | The publisher waits for a report only when the queue is empty; its producers never wait on it then: the subscriber waits only for room (`error-queue-send`), and the ArtNet manager never waits (`Reporter::report`, a `force_send`). |
| `artnet-queue` | acyclic | room in the ArtNet manager's command queue (10) | The ArtNet manager drains it in its loop (`artnet-loop`), which waits on nothing but its timer, its cancel token and this queue: its error reports never wait on MQTT (`Reporter::report`; `the_ticker_keeps_answering_while_the_mqtt_error_queue_is_full` failed on the old manager, which awaited the error queue). Its waiters — the subscriber and `Service::start`'s replay — are never waited on by it. If it has stopped, the send fails. |
| `artnet-reply` | acyclic | the ArtNet manager's reply | `ArtnetManager::handle_message` is synchronous and answers before the loop waits again, so the reply comes once the manager reaches the message, and the manager waits on nothing its waiters hold (`artnet-queue`). A manager that stops drops its queue and the reply senders in it, ending the wait with an error. `commands_that_wait_on_artnet_complete_while_its_errors_back_up_behind_a_stalled_broker` drives this wait with commands that report no error while the ArtNet manager's reports back up behind a stalled broker; with the manager's `Reporter::report` reverted to an awaited send it fails ("the bridge stopped taking commands while the broker was stalled"): the subscriber waited here while the manager waited on the full error queue. |
| `array-queue` | acyclic | room in the array manager's command queue (10) | The array manager drains it in its loop (`array-loop`), which waits on nothing but its cancel token and this queue. If it has stopped, the send fails. |
| `array-reply` | acyclic | the array manager's reply | `ArrayManager::handle_message` is synchronous and answers before the loop waits again; a manager that stops drops the reply senders, ending the wait with an error. |
| `artnet-loop` | acyclic | the ArtNet manager's `select!`: its cancel token, its 50 ms tick, its command queue | The tick fires every 50 ms on its own, and the arms' bodies are synchronous (DMX state, non-blocking UDP sends, `Reporter`), so the loop waits on no other task. |
| `array-loop` | acyclic | the array manager's `select!`: its cancel token, its command queue | The arms' bodies are synchronous; the loop waits on no other task. |
| `mqtt-workers` | acyclic | the session's publisher or subscriber ending (`join_next`) | Neither worker waits on the session task. A failed connection ends both — the pump hands the subscriber `Ended`, and drops the event loop so a waiting publish fails — so a session ends when its poll fails (`mqtt-poll` says how long that can take). |
| `task-shutdown` | acyclic | `JoinSet::shutdown`: aborting tasks, and waiting for them to end | Abort ends each task at its next await point, and no aborted task waits on the task shutting it down; a task inside a synchronous call (a persistence write) delays it by that call alone. Shutdown publishes nothing, so there is nothing to flush; the broker publishes the last will. |
| `ctrl-c` | acyclic | the operating system's interrupt signal | Nothing in the bridge waits on `main`; the wait ends when the service is told to stop. |

## Settings

```wait-lint
# rumqttc's client calls, which wait on its request channel, and its event loop's poll (no-hang
# §14.2): a wait at every `.name(..).await`, even should this crate define an `async fn` so named.
wait-methods = publish, publish_with_properties, subscribe, unsubscribe, disconnect, poll
# Dependency calls whose .await waits on nothing in this process.
not-waits = sleep, sleep_until, yield_now
# This code's own async methods, awaited on a receiver other than `self` (checked: the
# subscriber's `MqttSubscriber::handle_message`; the managers' are synchronous).
local-methods = handle_message
```

## Waiters (generated)

```wait-lint-waiters
array-loop src/array_manager/manager.rs ArrayManager::run
array-queue src/mqtt_subscriber.rs MqttSubscriber::send_array
array-queue src/service.rs Service::start
array-reply src/mqtt_subscriber.rs MqttSubscriber::handle_array_message
array-reply src/mqtt_subscriber.rs MqttSubscriber::handle_command_message
array-reply src/mqtt_subscriber.rs MqttSubscriber::handle_effect_message
array-reply src/mqtt_subscriber.rs MqttSubscriber::handle_value_message
array-reply src/service.rs Service::start
artnet-loop src/artnet_manager/manager.rs ArtnetManager::run
artnet-queue src/mqtt_subscriber.rs MqttSubscriber::send_artnet
artnet-queue src/service.rs Service::start
artnet-reply src/mqtt_subscriber.rs MqttSubscriber::handle_command_message
artnet-reply src/mqtt_subscriber.rs MqttSubscriber::handle_universe_message
artnet-reply src/service.rs Service::start
ctrl-c src/main.rs main
error-queue-recv src/mqtt_publisher.rs session
error-queue-send src/mqtt_subscriber.rs session
mqtt-backlog-lock src/mqtt_pump.rs Backlog::popped
mqtt-backlog-lock src/mqtt_pump.rs Backlog::pushed
mqtt-poll src/mqtt_pump.rs Pump::start
mqtt-pump-queue src/mqtt_pump.rs Incoming::recv
mqtt-pump-queue src/mqtt_subscriber.rs session
mqtt-request src/mqtt_publisher.rs session
mqtt-request src/service.rs Service::announce
mqtt-workers src/service.rs Service::mqtt_session
task-shutdown src/service.rs Service::mqtt_session
task-shutdown src/service.rs Service::stop
```
