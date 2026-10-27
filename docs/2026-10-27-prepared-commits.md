# Off-loop preparation and frame-boundary commit

The service can capture `Installation::preparation`, run its synchronous compiler
work on its single preparation worker, and commit the result at a frame boundary.
The live installation keeps ticking while the candidate is prepared. The API itself
owns no tasks or queues; the managed service must enforce that one-worker population.

The original deadline accompanies both preparation and final commit. A process-local
edit identity rejects a candidate after an accepted request or a restored replacement,
including identical persisted generations. Ticks and natural completions retain that
identity. Duplicate intent keeps its original receipt and the latest live snapshot.

Final commit rebases unchanged arrays on current live progress. Restart/new execution
uses commit-time planes; Stop freezes the current frame; brightness preserves a
completion that occurred during preparation. One accepted edit advances the latest
installation revision once. Admission and the final deadline check precede the swap;
all failure paths leave the live installation unchanged.

Installation-owned snapshots use a private encoding path retaining memory, wire,
completion-growth and revision-headroom checks without recompiling all published
definitions. Construction/admission already validates their private typed state.
External `Snapshot::encode`, `encode_for_commit` and `decode` still perform full
validation. No untrusted document may call the Installation-only encoding path.

Verification: four positive prepared-commit tests failed against the stub, then all
six initial tests passed. Later regressions cover late expiry, intervening accepted
intent and duplicate completion. All sixteen continuity/preparation tests pass;
full workspace 141/141, all-target host clippy and armv7 compile check pass. Eight
new guard-removal controls and the one re-anchored completion capture control fail
for their named assertions and are restored. Mutation/depth review is pending.
This does not establish managed-service scheduling or MQTT acceptance.
