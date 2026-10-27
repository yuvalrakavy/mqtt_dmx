# DMX engine and recovery memory admission

The 16 MiB policy covers accounted engine/recovery contents. It is independent of
the SDK's incoming, request-mailbox and terminal-answer limits, and is not an RSS
or allocator measurement. One managed owner and its one explicit fallback worker
are the population covered by this reservation. The adapter must preserve those
existing ownership/concurrency limits; these components do not implement the adapter.

The former encoded-size multiplier did not distinguish many small decoded fields
from a long string, and recovery compiled/restored players before checking memory.
Admission now counts decoded JSON fields, keys, strings and collection capacity,
typed authored-map storage, embedded playback-document expansion and compiled
runtime nodes. Every fade reserves its future initial-plane map, including later
Sequence children. Arithmetic saturates rather than wrapping.

| Reservation | Population / source | Maximum |
|---|---|---|
| Owner preparations | Live installation, prepared candidate, captured checkpoint, completion-reserved checkpoint, JSON expansion, validation replacement/compiler scratch; `Installation::{initialize,configure,desired,command,restore}`, `Snapshot::{encode,encode_for_commit}` | Six decoded/compiled slots |
| Concurrent fallback validation | One `SaveWorker::submit` job; decoded checkpoint, JSON expansion, restored replacement and scratch checkpoint/player/plan during validation | Five additional slots |
| Recovery carriers and scratch | Active, in-flight, replaceable pending, bootstrap, fallback-save documents; owner and fallback JSON/file codec scratch | Sixteen 256 KiB units (4 MiB) |
| Total | Eleven slots of 1,143,901 accounted bytes plus 4 MiB carrier reserve | 16 MiB, with one byte unassigned |

Each preparation must fit its slot, so an older large live installation and a
smaller new candidate are safe without assuming equal sizes. A configuration or
execution that cannot fit is rejected; no identities or executions are evicted.
The fixed reservation deliberately leaves headroom for simultaneous immutable
copies rather than relying on their usual sharing through Arc.

| Enforcement enumeration | Boundary |
|---|---|
| `rg -n 'reserve_player' mqtt_dmx/src/{engine,memory}.rs` | After effect/ownership validation, before allocating a runtime and its plane maps |
| `rg -n 'memory::reserve' mqtt_dmx/src/recovery.rs` | Before snapshot validation reconstructs a compiled installation, and for worst-case completion before commit |
| `rg -n 'encode_for_commit' mqtt_dmx/src/installation.rs` | Initialization, restore, configuration, playback/command candidates and completion documents |
| `rg -n 'Semaphore::new\|Snapshot::decode' mqtt_dmx/src/fallback_worker.rs` | One fallback validation/save at a time, independently bounded and cancellable as previously documented |

The encoded configuration/playback/snapshot caps, depth, node, reference and
physical ownership limits remain independent. Codec parsing is still temporary
allocation before these accounted checks; the wire/depth caps bound those inputs.
In-process callers must not retain additional unbounded histories of snapshots or
create extra independent owners/workers under this single-owner reservation.

The new dense-map regression first failed without the change. It uses a valid
configuration below 128 KiB, proves initialization/update/recovery refusal, and
checks that a running animation advances after the refused update. An invalid
checkpoint with the same dense configuration distinguishes preflight from a later
reconstruction error. A player with valid node/reference counts but excessive
expanded runtime storage is also refused, while a subsequent small player works.

The existing completion-wire fixture now uses 256 channels and escaped padding,
so carrier growth reaches its ceiling before decoded memory. Its guard-removal
control remains independently sensitive. New guard-control and final suite results
are recorded in the project tracker and committed control ledgers.
