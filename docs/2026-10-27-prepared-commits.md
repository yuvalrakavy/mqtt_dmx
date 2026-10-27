# Off-loop preparation and frame-boundary commit

The service can capture `Installation::preparation`, run its synchronous compiler
work on its single preparation worker, and commit the result at a frame boundary.
The live installation keeps ticking while the candidate is prepared. The API itself
owns no tasks or queues; the managed service must enforce that one-worker population.

The original deadline accompanies both preparation and final commit. A process-local
edit identity rejects a candidate after an accepted request or a restored replacement,
including identical persisted generations. Ticks and natural completions retain that
identity. Duplicate intent keeps its original receipt and the latest live snapshot.

Final commit rebases unchanged arrays on current live progress. New Desired execution
uses commit-time planes; Restart returns to its original start. Stop freezes the current frame; brightness preserves a
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
for their named assertions and are restored.

The mutation reviewer found two test gaps, with no production defect identified:
unconditional live-array copying could discard remapped ownership/generation, and
whole-array continuity copying could discard newer broker ordering metadata. The
original 16-test suite stayed green under both mutants. Two retained public-API
tests now detect them; the strengthened suite passes 18/18 independently and in the
author tree. Both new author-run controls fail as expected (6.1 seconds total) and
are restored. Configuration edit-identity rotation was also independently mutated
and detected by an existing test. Depth review found no further issues in the
frozen core delta; it did not run timing/RSS, MQTT or hardware checks.

Subsequent author integration review caught an S2 semantic regression in the new
prepared path: it used current planes for explicit Restart, whereas the existing
engine and its retained regression require original starting planes. The original
preparation fixture incorrectly expected the new-effect semantics for Restart.
That fixture now uses a real parameter-changing Desired operation; a separate
Restart regression was observed RED (128 instead of original 0). The correction
selects current held planes for Desired and original planes for Restart. Both
branches have separate guard-removal controls (both RED in 5.4 seconds, restored).
The full workspace now passes 144/144, all-target host clippy and armv7 compilation
against the pinned reviewed BridgeKit source. Confirming review is pending.
This does not establish managed-service scheduling or MQTT acceptance.
