# Running-execution continuity preparation

`Installation::preserve_running_from` prepares a selected, fully validated broker
installation while retaining the live player position only for matching running
executions. It compares physical ownership, history/authority/generation, execution,
selection, parameters, level and the normalized player image. The selected broker
cut keeps its operation ordering metadata. Completed/stopped images are always
restored with their held output; they never borrow a live timeline.

Seven hardware-free tests pass: matching position and next frame, different
execution refusal, terminal output preservation, local completion promotion,
multiple-completion ordering, brightness mismatch and stopped-local exclusion.
The original two controls and four completion-promotion controls fail for their
named assertions and are restored. This is a preparation primitive;
the managed MQTT service does not call it yet, and full owner integration review
remains pending.

Owner clarification: matching locally completed playback keeps its final output
even when a newer broker cut still says Running. The proof compares physical
ownership, execution identity, history/authority/generation, selection, parameters,
level, immutable effect and original planes. Only the live player's held/status
fields are normalized for comparison, then its actual final player is preserved.
The selected broker cut retains its configuration and request ordering metadata.
All qualifying completions produce one admitted snapshot above BOTH revision bases;
restoring that terminal image never promotes it again. A stopped local execution
does not qualify. The principal regression was observed RED before implementation.

After the complete change, the workspace passes 132/132 tests, host all-target
clippy with warnings denied, and the Raspberry Pi armv7 compile check. The new
four-control run took 10.9 seconds; all mutations were restored. No MQTT service
or end-to-end acceptance is claimed by these engine checks.
