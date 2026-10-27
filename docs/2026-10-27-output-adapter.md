# Managed-owner output adapter slice

`Installation::channels()` exposes only the compiler's fixed physical ownership.
The output adapter reads each channel's actual rendered engine value; it neither
resolves authored labels nor applies brightness again. Each 50 ms owner tick can
produce a complete 512-channel Art-Net datagram for every configured universe.
Packets use explicit opcode/protocol/length byte order and nonzero per-universe
sequence numbers. IPv4-mapped destinations use the compiler's canonical form.

`Frames::reconfigure` prepares a replacement before changing the active frames.
It returns zeroing packets for released addresses on the old mapping; the owner
must send those before sending the new mapping's frames. Retained physical
universes keep their sequence counters. A bad release or mismatched layout
preserves earlier frame data. Diagnostic unowned values remain outside recovery
and are reset by fresh configuration-frame construction.

`DatagramSender` lazily opens at most one local nonblocking UDP socket per address
family. It performs no DNS, broker or filesystem operation. Errors return to the
owner without a local retry or wait; the owner will apply the fleet's external
failure reporting policy while playback continues. Per-packet traffic is TRACE.

Nine hardware-free tests cover engine values on distinct universes, exact packet
headers, wrapping through 255 to 1, canonical addresses, selective release,
atomic refusal, layout changes, activation sequence continuity and an actual
datagram received on an owned ephemeral loopback socket. The first four tests
were observed failing against a scaffold; the sequence-activation case also
failed before its correction. Nine new controls fail at the intended assertions.
This is component evidence, not the Store/MQTT/engine/emulator acceptance chain.

The prerequisite mutation review also retained two engine memory probes: large
ownership with a small Delay and spare authored node capacity. Both were GREEN
against original production and RED when their respective charge was removed.
Author verification repeated both mutations through the new resource-review table.
Production memory code did not change. SDK-side correlation/context probes are
recorded in the tracing-init prerequisite review note.

Current tree checks: workspace 116/116, zero skipped; all-target clippy with denied
warnings; Raspberry Pi armv7 workspace check. The managed process owner, recovery
selection/installation, CLI/binary, fallback supervision and Store driver are
still being implemented. Depth review of the frozen prerequisites is next.
