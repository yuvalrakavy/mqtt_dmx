# Managed DMX service review

P2 integrates the existing reviewed engine with one managed owner, bounded proxy
requests, an off-loop preparation job, Art-Net output and explicit fallback storage.
The SDK pin is `7d33a53407adb9557fbc4253c6ad7756fb1a6079`. Store recovery backups and
adoption/transfer commands were removed; exact combined memory accounting is deferred.

The bounded mutation review of that SDK pin removed the post-validation staging
deadline check and synchronous command-capability validation separately. Both
existing targeted tests failed; both passed again after restoration. No surviving
mutant was found within this two-control scope.

The depth review inspected bridge `bcdc568` and the SDK integration delta. It found
one blocking defect: local initialization could already be accepted when its later
runtime installation expired, leaving lighting Authoritative but runtime admission
RecoveryOnly. The runtime had no remaining installation timer to repair that split.
An isolated SDK probe reproduced it and verified that autonomous reseeding on the
original connection converges; a newer replay refuses the old capability.

The owner now retains the original installation connection. A command-triggered
acknowledgement failure preserves lighting and the failed/unknown command result,
then reseeds accepted state autonomously on that same connection. It never borrows
a newer replay. The retained owner/runtime regression drives the real runtime on a
current-thread test executor and checks both convergence and a newer replay refusal.
The two new guard-removal controls both fail at their intended assertions and restore
automatically through the repository control runner.

Verification after correction: DMX workspace 151/151 passes against the published
SDK pin. SDK 312/312 and unchanged compatibility 176/176 passed for the pinned SDK;
host all-target lint and Raspberry Pi armv7 compilation passed before the owner-only
correction. The correction receives its own lint and confirming review. Review uses
GPT-family agents only; no cross-family or physical-hardware verification is claimed.

The confirming reader found no further blocking issue in the correction. Its
independent test attempt could not open the shared build-cache lock; the author
ran the real regression, both controls and full 151-test suite successfully.
All-target lint also passes after correction. P3 Store integration is next.
