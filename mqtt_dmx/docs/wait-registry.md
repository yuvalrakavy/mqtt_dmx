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
**subscriber** (`mqtt_subscriber::session`) handles them, asking the ArtNet and array managers,
posting config saves and queueing error reports; the **publisher** (`mqtt_publisher::session`)
publishes those reports; the **ArtNet manager** ticks every 50 ms, sending DMX and queueing its own
error reports; the **array manager** answers questions about arrays and effects; the **writer**
(`Persistence::write_saves`) writes each posted save on a blocking thread. `Service::mqtt_session`
runs one connection, and `Service::mqtt` reconnects, keeping across sessions the broker's outage
state (`Outage`, atomics only, which each session's pump tells of a CONNACK and of the connection's
traffic) and the last error report (`LastError`), which each connection republishes. `main` builds
the runtime, waits for SIGTERM or SIGINT, stops the service within a bound and ends the runtime
within another. The wait graph runs subscriber → managers, subscriber → publisher → request
channel → pump, and nothing waits back on the subscriber; the pump waits on nothing here; nothing
on an async worker touches the filesystem, and nothing waits on the writer but the stop.

## The rows

| Key | Kind | Waits on | Argument |
|---|---|---|---|
| `mqtt-request` | acyclic | rumqttc's request channel (`publish`, `publish_with_properties`, `subscribe`) | The channel drains only while the event loop is polled, and the pump polls it in a task of its own that waits on nothing but the network. Its waiters — `Service::announce` (directly and through `publish_retained`) and the publisher — are never waited on by the pump, so a full channel is back-pressure on them, never a cycle: with the broker stalled they wait until it recovers, the pump reading its acks meanwhile. When the connection fails the pump drops the event loop, and a waiting publish fails instead (`a_burst_of_failing_commands_against_a_stalled_broker_reports_every_error_once_it_recovers`; on the old session, whose poller queued its own error reports, 1 of 100 arrived). `announce` subscribes first, republishes the retained model from the bridge's state (its version, the last error report), and says `Active=true` last (no-hang F4; `active_goes_out_only_after_the_subscription`, failing first: `Active` went out before the subscription). |
| `mqtt-poll` | acyclic | the broker, over the network | The pump waits on nothing in this process, so nothing here can wait back on it. How long one poll takes is up to the network, and only partly bounded: a connect and its CONNACK end within rumqttc's 5 s connection timeout, and a peer that stops answering fails the poll at the second unanswered keep-alive ping (about 10 s at the 5 s keep-alive), when the pump hands the subscriber `Ended` and stops. But rumqttc awaits its network write and flush inside its `select!` with no timeout, so a peer that keeps the connection open and stops reading (its receive window closed) holds the poll until the kernel's TCP timeouts end the connection — minutes, and no bound at all while a live peer host keeps answering zero-window probes. This bridge has few bytes outstanding (QoS 1 publishes capped by the broker's receive maximum, pings, acks), so a full socket buffer is unlikely, not impossible. While it lasts the session stalls — the publisher waits on the request channel, the subscriber on the forward queue — and nothing outside the session waits on it: the ArtNet manager never waits on MQTT, and shutdown aborts the session's tasks. |
| `mqtt-pump-queue` | acyclic | the pump's forward queue | Unbounded (no-hang §14.6): the pump never waits on the subscriber, so the subscriber's wait ends with the next message, or with `Ended` when the connection fails — the session then ends and `Service::mqtt` reconnects. Past 1000 unread publishes it is a WARN (`mqtt_backlog_high`), never a drop while the session lasts; what is still unread when the session ends (the publisher ended first, and the subscriber was aborted) is discarded, counted in a WARN (`mqtt_commands_discarded`). |
| `mqtt-backlog-lock` | acyclic | the forward queue's high-water flag | Held to read or set one timestamp (`Backlog::raise`, `Backlog::clear`), and nothing else: the WARN and INFO it decides are logged after the guard is dropped, by the fleet rule that nothing is logged under a lock (`the_backlog_logs_nothing_while_it_holds_its_lock`, failing first: the WARN was logged under the lock; tracing-init's writers were synchronous then, and are non-blocking and lossy since 97eebba). Nothing under it waits. |
| `error-queue-send` | acyclic | room in the error queue to the publisher (10 reports) | Only the subscriber waits here. The publisher that drains the queue waits only on the request channel (`mqtt-request`), which the pump drains, and nothing on that path waits on the subscriber. A session that ends while it waits aborts it (`mqtt_session` shuts both workers down), so it never outlives the publisher. |
| `error-queue-recv` | acyclic | a report in the error queue | The publisher waits for a report only when the queue is empty; its producers never wait on it then: the subscriber waits only for room (`error-queue-send`), and the ArtNet manager never waits (`Reporter::report`, a `force_send`). |
| `artnet-queue` | acyclic | room in the ArtNet manager's command queue (10) | The ArtNet manager drains it in its loop (`artnet-loop`), which waits on nothing but its timer, its cancel token and this queue: its error reports never wait on MQTT (`Reporter::report`; `the_ticker_keeps_answering_while_the_mqtt_error_queue_is_full` failed on the old manager, which awaited the error queue). Its waiters — the subscriber and `Service::start`'s replay — are never waited on by it. If it has stopped, the send fails. |
| `artnet-reply` | acyclic | the ArtNet manager's reply | `ArtnetManager::handle_message` is synchronous and answers before the loop waits again, so the reply comes once the manager reaches the message, and the manager waits on nothing its waiters hold (`artnet-queue`). A manager that stops drops its queue and the reply senders in it, ending the wait with an error. `commands_that_wait_on_artnet_complete_while_its_errors_back_up_behind_a_stalled_broker` drives this wait with commands that report no error while the ArtNet manager's reports back up behind a stalled broker; with the manager's `Reporter::report` reverted to an awaited send it fails ("the bridge stopped taking commands while the broker was stalled"): the subscriber waited here while the manager waited on the full error queue. |
| `array-queue` | acyclic | room in the array manager's command queue (10) | The array manager drains it in its loop (`array-loop`), which waits on nothing but its cancel token and this queue. If it has stopped, the send fails. |
| `array-reply` | acyclic | the array manager's reply | `ArrayManager::handle_message` is synchronous and answers before the loop waits again; a manager that stops drops the reply senders, ending the wait with an error. |
| `artnet-loop` | acyclic | the ArtNet manager's `select!`: its cancel token, its 50 ms tick, its command queue | The tick fires every 50 ms on its own, and the arms' bodies are synchronous (DMX state, non-blocking UDP sends, `Reporter`), so the loop waits on no other task. |
| `array-loop` | acyclic | the array manager's `select!`: its cancel token, its command queue | The arms' bodies are synchronous; the loop waits on no other task. |
| `mqtt-workers` | acyclic | the session's publisher or subscriber ending (`join_next`) | Neither worker waits on the session task. A failed connection ends both — the pump hands the subscriber `Ended`, and drops the event loop so a waiting publish fails — so a session ends when its poll fails (`mqtt-poll` says how long that can take). |
| `task-shutdown` | acyclic | `JoinSet::shutdown`: a session's publisher and subscriber (`Service::mqtt_session`), or the service's tasks (`Service::stop_within`) — aborting them, and waiting for them to end | Abort ends each task at its next await point, and none of them waits on the task shutting it down. None of them calls into the filesystem (the subscriber posts its saves to the writer), so none is inside a synchronous call for longer than a computation. In `stop_within` it runs inside `service-stop`'s bound. Shutdown publishes nothing, so there is nothing to flush; the broker publishes the last will. |
| `service-stop` | bounded | `Service::stop_within`: the service's tasks — the ArtNet manager, the array manager, the MQTT loop — aborted and waited for (`task-shutdown`), then the writer left to write what was saved (`writer-end`) | Within 5 s (`STOP_WITHIN`), all of it. Abort ends each task at its next await point, and none of them waits on `main`, which stops them, so they end at once unless one is inside a synchronous call; the writer ends once its pending saves are written. On expiry: a WARN (`shutdown_timeout`, with the tasks left and whether the writer was still `writing`), the writer is aborted, and the stop returns; `main` then ends the runtime within its own bound (`main-run`), leaving behind a thread still inside a synchronous call (`stopping_is_bounded_when_a_task_will_not_end`: failing first, the stop waited 3 s for a task in a 3 s synchronous call; now 0.3 s and a WARN). |
| `stop-signal` | acyclic | SIGTERM or SIGINT from the operating system (`StopSignals::recv`, `recv_or_never`), raced in `run` against the startup | Nothing in the bridge waits on `main`, so nothing here can wait back on it; the wait ends when systemd (SIGTERM) or an operator (SIGINT) stops the bridge. The handlers are registered first thing, each on its own, so neither signal ends the process unhandled (`sigterm_stops_the_bridge_through_its_shutdown`: failing first, SIGTERM killed it); one that cannot be registered is a WARN (`signal_handler_unavailable`) once logging is up, and only that signal ends the process by its default action (no-hang F3). A stop during the startup ends it where it is, its bounded waits included (`a_stop_during_a_stalled_config_read_at_startup_is_prompt`: failing first, the bridge never exited). |
| `logging-start` | bounded | the logging's start, on a blocking thread (`start_logging`), raced in `run` against the stop | Within 15 s (`LOGGING_START`), past tracing-init's own 5 s bound on each destination's start; tracing-init's read of its configuration has no bound of its own. On expiry the bridge goes on without a log — logging never stops it — and the thread is left to the runtime's bounded end (`main-run`). A stop ends the wait at once: `run` returns, and the runtime's end abandons the thread (Store 3b review round 3, B1). `a_stop_during_a_held_logging_start_is_prompt` holds the start with the debug-build-only test seam `MQTT_DMX_TEST_LOGGING_GATE` — a file `start_logging` reads before anything else, here a FIFO the test keeps open — proves the read began, and sees SIGTERM end the bridge cleanly within the runtime's drain; the seam is inert unless set and compiled out of release builds. Failing first, on the real stall it stands for: with the log file a FIFO nobody reads, the old code acted on a SIGTERM sent at 1 s about 4 s later, when tracing-init gave the destination up. |
| `main-run` | acyclic | `main` running the bridge on its runtime (`block_on(run(..))`) | The bridge's life: `run` returns once a stop signal has come (`stop-signal`, `logging-start`) and the service's bounded stop has returned (`service-stop`), and nothing on the runtime waits on `main`. After it `main` ends the runtime with `shutdown_timeout`, within 1 s (`RUNTIME_DRAIN`): a thread still inside a synchronous call — a write to a stalled disk, the startup's read, a name lookup in rumqttc's connect — is left behind and ends with the process, where dropping the runtime would wait for it without limit (no-hang F1). Then the log's guard is dropped, which tracing-init bounds at about 4 s. Its console, file and GELF writers are non-blocking and lossy (a WARN `log_lines_dropped` counts what a stalled destination cost), and starting the file and GELF destinations is bounded at 5 s (then skipped, WARN `log_destination_skipped`), so a stalled log destination costs log lines, never a thread (tracing-init 97eebba; before it, a stalled log disk blocked every thread that logged, `main` included). So SIGTERM ends the process within about 5 + 1 + 4 s (`a_stalled_config_write_does_not_hold_up_the_stop`: failing first, the old `main`, which dropped the runtime, never exited). |
| `persistence-load` | bounded | the saved configuration, read on a blocking thread (`Persistence::load`, awaited by `Service::start`) | Within 10 s (`LOAD_WITHIN`). On expiry: a WARN (`config_load_failed`), and the bridge starts without its saved configuration — the Store publishes every config retained, so the broker restores it when the bridge subscribes — while the read is left behind on its thread, which the runtime's shutdown does not wait for (`main-run`). A stop signal during the wait ends the startup at once (`stop-signal`). (`a_stalled_config_read_at_startup_does_not_keep_the_bridge_off_its_broker`: failing first, `main` read the files itself and never reached the broker.) |
| `persistence-lock` | acyclic | the configuration's state (`Persistence::locked`): the configuration as last saved, the saves not yet written, whether it is closed | Held to read or change that state alone — by a save, the writer, the startup's load and the stop's close — with no I/O, no logging and no other wait under it. |
| `persistence-wake` | acyclic | the writer waiting for a save to be posted, or for the close | Posting (`save_*`, `close`) never waits: it records under `persistence-lock` and notifies, and the notification keeps a permit for a post that comes between the writer's look and its wait. Nothing waits on the writer but the stop (`writer-end`). |
| `persistence-write` | bounded | one file's write, on a blocking thread (`Persistence::write_saves`) | Within 5 s (`SAVE_WITHIN`). The disk waits on nothing in the bridge, and nothing waits on the writer but the stop, within its own bound (`writer-end`): the subscriber posts its saves and goes on (`a_stalled_config_write_holds_up_no_command`: failing first, the old subscriber wrote the file itself, on an async worker, and took no command after). On expiry: a WARN (`config_save_failed`, the first failure of an episode; later ones DEBUG, and `config_save_recovered` with the episode's length when a save works again), and the writer waits for that write to end (`persistence-write-stalled`). A write that fails keeps its content for the next try (`persistence-retry`). |
| `persistence-write-stalled` | acyclic | a write past its 5 s bound, to its end (`Persistence::write_saves`) | A write to a stalled disk cannot be interrupted, so the writer waits for it before the next: one thread held, never one per save, and the saves posted meanwhile replace each other, one pending content per file. The disk waits on nothing in the bridge, posting never waits on the writer (`persistence-wake`), and the stop leaves a stuck writer behind past its own bound (`writer-end`, `main-run`). |
| `persistence-retry` | bounded | the writer's pause after a failed write (`Persistence::write_saves`) | Within 5 s (`SAVE_RETRY_AFTER`); saves posted meanwhile do not cut it short, so a disk that refuses every write at once is tried every 5 s, never spun on (`a_failed_save_is_kept_and_tried_again_at_a_pace`, failing first: the content was dropped and never written). On expiry: the writer tries the kept content again — the failed write's, unless a newer save of that file replaced it. The stop ends the pause at once (`close`), and the writer ends, giving up what is still unwritten. Nothing waits on the writer but the stop (`writer-end`). |
| `writer-end` | acyclic | `Service::stop_within` waiting for the writer, once closed, to write what was saved and end | Inside `service-stop`'s 5 s bound; the writer waits only on the disk (`persistence-write`) and on posts (`persistence-wake`), never on the stop. A writer stuck on a stalled disk is left behind past the bound, aborted, with the WARN (`a_stop_writes_what_was_saved_before_it`; `a_stalled_config_write_does_not_hold_up_the_stop`). |
| `last-error-lock` | acyclic | the last error report (`LastError`) | Held to set it (the publisher, before it publishes a report) or read it (`announce`, to republish it), and nothing else: no I/O, no logging, no other wait under it. |

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
error-queue-recv src/mqtt_publisher.rs session
error-queue-send src/mqtt_subscriber.rs session
last-error-lock src/mqtt_publisher.rs LastError::get
last-error-lock src/mqtt_publisher.rs LastError::set
logging-start src/main.rs run
main-run src/main.rs main
mqtt-backlog-lock src/mqtt_pump.rs Backlog::clear
mqtt-backlog-lock src/mqtt_pump.rs Backlog::raise
mqtt-poll src/mqtt_pump.rs Pump::start
mqtt-pump-queue src/mqtt_pump.rs Incoming::recv
mqtt-pump-queue src/mqtt_subscriber.rs session
mqtt-request src/mqtt_publisher.rs session
mqtt-request src/service.rs Service::announce
mqtt-request src/service.rs publish_retained
mqtt-workers src/service.rs Service::mqtt_session
persistence-load src/persistence.rs Persistence::load
persistence-load src/service.rs Service::start
persistence-lock src/persistence.rs Persistence::locked
persistence-retry src/persistence.rs Persistence::write_saves
persistence-wake src/persistence.rs Persistence::write_saves
persistence-write src/persistence.rs Persistence::write_saves
persistence-write-stalled src/persistence.rs Persistence::write_saves
service-stop src/service.rs Service::stop_within
stop-signal src/main.rs StopSignals::recv
stop-signal src/main.rs recv_or_never
stop-signal src/main.rs run
task-shutdown src/service.rs Service::mqtt_session
task-shutdown src/service.rs Service::stop_within
writer-end src/service.rs Service::stop_within
```
