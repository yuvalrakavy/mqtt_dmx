# Running-execution continuity preparation

`Installation::preserve_running_from` prepares a selected, fully validated broker
installation while retaining the live player position only for matching running
executions. It compares physical ownership, history/authority/generation, execution,
selection, parameters, level and the normalized player image. The selected broker
cut keeps its operation ordering metadata. Completed/stopped images are always
restored with their held output; they never borrow a live timeline.

Three hardware-free tests pass: matching position and next frame, different
execution refusal, and terminal output preservation. Both guard-removal controls
fail for their named assertion and are restored. This is a preparation primitive;
the managed MQTT service does not call it yet, and full owner integration review
remains pending.

One design case is awaiting owner clarification: fallback playback locally completes,
then a newer broker cut describes that same execution as running. The design's
matching-status rule and separate completion-promotion rule need a precise priority.
No automatic local-completion promotion is implemented by this primitive.
