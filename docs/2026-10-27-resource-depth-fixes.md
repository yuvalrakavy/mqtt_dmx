# Resource-depth review corrections

The prerequisite depth review froze engine `04decdc` and BridgeKit `53b7a4c`.
It found two S2 production defects: delayed driver observations surviving exact
recovery replacement, and decoded JSON scratch allocated before player admission.
The runtime correction is maintained on its separate recovery branch.

## Engine preflight correction

A valid 42,356-byte configuration expands 512 parallel fades over 32 channels.
The former `serde_json::to_value(effect)` preflight created a representation costing
18,819,377 admission units before refusing the player, above the 16MiB aggregate
policy. These are the policy's accounting units, not measured heap usage or RSS.

Player admission now checks runtime and ownership first, then counts the remaining
serialization against the residual slot budget without constructing a JSON tree.
Snapshot admission uses the same bounded counter before canonical serialization;
completion-reserved snapshots also pass admission before serialization. The counter
keeps no representation, stops traversal on refusal, and counts formatted strings
without allocating an intermediate string. Map keys carry a conservative scalar
header in addition to the previous member charge.

Verification: 124/124 workspace tests passed, followed by the retained wire-path
regression passing independently; host workspace clippy with warnings denied passed.
The counter's guard-removal control failed as expected and was restored. The original
allocation reproduction is not retained as an assertion about the obsolete path;
the retained tests instead exercise bounded traversal and unchanged live state on
refusal. Confirming review of frozen fix commit `5c3adc0` found no further issues.
The reviewer also checked paired runtime fix `bbf8d83`; no independent engine suite
rerun or different model-family coverage is claimed. Integration evidence remains
pending.

## Recovery selection

Six tests exercise startup, provisional fallback comparison against its immutable
saved base, and authoritative reconnect selection using real installation cuts.
The pure selector never installs or restarts a player. Service wiring remains open.
