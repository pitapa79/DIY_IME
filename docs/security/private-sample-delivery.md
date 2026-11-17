# Private retained sample delivery (#127)

Parent #109. Verified starting main: 1cd762e540faaaf5b69e43cd6d411ce7ac8a05af.
Explicit dependency: PR126 / 1158bee75c7ed7160e14bc25b60bf3b8a6322ad4.
Dedicated codex/127-private-sample-delivery worktree. Level 2 internal performance
change. No public request, default switch, Node/SDK, permissions, isolation,
currentness, physical-ending guarantee, resource/fuel limit or CI gate changes.

## Problem and change

The private retained timing experiment captures a bounded sample ring. Previously
every Runner status snapshot cloned/materialized it, including ordinary Run,
GetRun, CancelRun and InvalidateInput replies. The preserved #125 compiled cold
trial's final explicit snapshot re-encoded to approximately 479184 UTF-8 bytes,
478315 of them callSamples. These are snapshot sizes, not a capture of earlier
wire replies. The observed 55.407ms physical-end-to-publication interval includes
polling, request handling, serialization and flush; it does not isolate their
individual costs.

Ordinary replies now skip retrieving this private ring entirely. Explicit
Operation::Status still snapshots it without draining it. Experimental metadata
(ownership, readiness, group sizes, process identities, generations, startup
failures and compiled generations) stays present. Fixed/default replies keep the
same fields and never expose the experimental ring. Sample capture, finite
reservation, drops, lifecycle evidence and analyzers are unchanged. Skipping
sample retrieval does not skip result authorization or physical accounting.

## Verification and measurement

The private benchmark records actual received JSON-line UTF-8 byte counts,
maximum reply sizes and replies containing samples, per request method. Counts
exclude the line delimiter and transport framing; they are not reconstructed
wire-size estimates. The aggregate stores no inputs, outputs or sample contents.
Old and new runtimes can use the same observer. An explicit private check flag
requires ordinary replies to omit samples; it is enabled by the existing CI
collector. No new public Host field or request is introduced.
For controlled old/new comparisons, the private observe-only flag performs the
same repeated explicit retrievals on both versions without rejecting the old
reply layout. Ordinary-reply sample counts remain visible for comparison. This
avoids attributing unequal verification traffic to the implementation change.

Outside each measured input/trace window, two settled explicit status reads must
have identical settled call samples and call drop counts. Lifetime phases may
advance during original-owner cleanup, but earlier phases must remain present
unless their removal is accounted for by the phase ring's drop counter.
When timing is enabled, every executed
linear stage must still have samples, and the existing same-call analyzer must
pair the latest successful input completely. Timing OFF and S0/default must not
fabricate sample buffers. The fixed official Host checker also asserts its exact
six retained-owner fields on successful status/state replies. All correctness,
trace decoding/dropped-zero and original final-cleanup checks remain in place.

Local script tests: 43 passed, including the existing input-failure, fair polling,
same-call, lifecycle and benchmark checks and six new reply-evidence checks.
Log SHA256: 98cc643ac39a235a803f812e43555b27fb7105e4d4789499c075c949383c19a2.
Both modified actual-Host scripts pass Node syntax checks; the collector passes
Python syntax validation; Rust fmt also passed. These script checks do not prove the new native Host
behavior or its performance.

Historical native comparisons are preserved below. Complete final-source Rust
and Windows CI remain pending. Preserve old runtime30ee1caf66b9db82ab9b100c1007d00a164d394c
and the isolated comparison build0996064b932f768b70a822ea7acea5628a324f82
separately. Only execution_host.rs differs under src between those builds;
Manager/Worker/DLL/module are fixed. Both precede queue fix128, so they do not
measure the final combined Host. All12 local trials passed correctness,
trace/dropped0, repeated sample retrieval and original final-cleanup checks:
668 attempted inputs,240 measured warm results (eight first results excluded),
60 correct latest cold-paced results and79176 decoded trace events. Exact
per-trial counts, distributions, setup, CPU/RAM, byte counts and raw-file hashes
are in [local native evidence](evidence/private-sample-delivery-native-local-20261117.json).
Timing-ON warm medians were old112.211/138.969ms and new60.677/65.572ms;
OFF medians remained around59–64ms. ON GetRun maximum JSON lines changed from
about375KB to5.6KB, with private samples absent from every new ordinary reply.
P95/P99 did not consistently improve: new warm max1285.901ms and cold20ms latest
max49990.656ms remain major response issues. The local volume was constrained;
lossless compression of closed evidence overlapped the first cold trial, after
all eight warm trials. Do not describe this as an otherwise idle environment.

The timing CI segment now builds its matched control from the exact current
Rust/Cargo sources with private-sample-replies-baseline. That Cargo feature
restores only the previous private-sample retrieval on ordinary experimental
owner replies. Distribution builds omit it; fixed/default replies, sample
capture, authorization, limits and physical ownership are unchanged. Using the
same current source avoids confusing later Rust improvements with the delivery
change, and avoids permanently restricting future changes to one Rust file.
The collector records source/feature/binary provenance, checks source hashes
before and after compilation, and verifies the current bundle. Both builds use
the same release profile/source and registry path remapping. Manager, Worker,
DLL, Node runtime and module are identical current bundle files for both Hosts.

OFF and ON each use old/new/new/old warm30, followed by four ON cold-paced5
trials, once each in the original300s owned Job/330s outer observation bound.
All correctness/trace/cleanup checks and diagnostic traffic remain identical.
Control executable, build log, sources/flags/hashes, raw results and failures,
and per-Job CPU/RAM/ending evidence remain CI artifacts. New ordinary replies
must omit callSamples; both explicit ON status routes must retain them. A failed
case stops without retry. This matched-control revision requires fresh native
CI and replaces no existing segment or safety gate.

Historical CI44082259984 compared baselinec2760ad15011c9c508f24ce17a15c7578d13b528
with candidate8a4803cd464e09715b6406b4ba436f37f8d3fa48. All nine segments and the
aggregate passed, including twelve once-only native comparisons. Whole20-node
warm ON medians were old32.320/33.348ms versus new17.607/17.663ms; OFF was around
17.5-17.6ms. New cold20ms latest medians16.523/17.146ms had maxima674.283/708.336ms,
with five latest samples per cadence per trial. Full artifact14328903005 SHA256:
977a699c50a917db9324db1da7e816edfb8edf3af8afb1d6f00f6aea04af0e76.
These results do not verify later source or the matched-control revision.

Do not infer semantic Node cost from guest SDK/API spans, subtract independent
percentiles, or divide a 20-stage response into a per-node percentile. A smaller
reply alone is not a performance pass, S1 qualification or Goal109 completion.
