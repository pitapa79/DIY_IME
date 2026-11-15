# Reviewed primitive retention evaluation

Refs #104 and #95. Evaluation authorized by the maintainer on 2026-11-15 JST, through measurement and a technical adopt/revise/reject recommendation. Default adoption, official Node adoption and a Level3 product security-policy merge remain separate decisions. Results below are experimental; no adoption or #95 performance-goal success is claimed.

Main at start: `336442b4cb63c8411b67c90736e38af5f6599395`. Explicit experimental comparison base: PR103 `7e182ab8d069580cf2a0b89dc861390c6a346f31`. This retains the prepared disposable comparator without merging other Draft PRs.

## Previous experiment preserved

The uncommitted runtime-file-copy experiment was archived with its base, tracked patch, two new files, SHA manifest and successful/failed logs before restoring PR103's worktree. It remained default OFF and was not adopted. Release check/Clippy/build and two actual Windows unit tests passed. Neutral lifecycle diagnostic0 passed45.640s. Normal Host plain-catalog safety failed28.578s at the unchanged assertion that a neutral spare must be used rather than cold launch. Its cause is unresolved. No assertion/timeout/fixture was relaxed, the diagnostic runner was not run, and no ABBA performance evidence was obtained for this method. The archive is `runtime-copy-experiment-shelved-20261115` under the local task workspace; it contains no real user input. Existing PR103 CI43598040279 completed all four segments and aggregate successfully at7e182ab, independently of that uncommitted experiment.

## Experimental routes

The Host environment variable `DIY_EXPERIMENT_REVIEWED_PRIMITIVE_RETENTION` is absent/0 by default. `separate` admits one reviewed Node per retained AppContainer. `grouped` admits a strict linear graph in groups of at most four stages. A20-stage chain therefore retains20 or5 processes respectively. Each group stays within the existing16-binding and four-module native limits; neither limit is widened. Existing exact-four-stage fixed-official retention remains the default policy and is wrapped without changing its admission.

The private allowlist requires the exact official module SHA `105c0ceb2854dba27a747eae5faa6e393bb99a88ce5ab3048a468fa9add88134`, version1 and selected kinds: `experimental.packet-passthrough`, `experimental.text-trim`, `experimental.text-case`, `experimental.text-concat`, compatibility `demo.identity`, and the pure language adapter `text.romaji-to-hiragana`. The latter is not relabeled as a primitive; it supplies the existing ja-Latn to ja port transition for actual mixed-primitive graphs. All selected manifests declare no capabilities. Runtime Node objects must have no dictionary/network/prepared-resource binding. A name, classification or guest self-report is insufficient. Normal package/definition/config validation and input-flow/execute consent still apply.

Only Host-owned preparation graph snapshots supply group order. Grouping validates one connected linear external chain and rejects branch/join instead of guessing an order from JSON. Separate retention can evaluate independent branches; grouped branch/join support is not implemented and must appear as a limitation, not a passed condition. Unused Nodes and aggregate lifetime costs require measurement.

Disposable spare inventories and retained evaluation cannot overlap. The same explicit finite physical/resource policy must be reported for every comparator. Lifetime reservations remain distinct from transient Core input callbacks. The benchmark uses22 physical slots so20 separated lifetime owners leave input capacity; this is a comparison configuration, not a new default. Insufficient capacity is rejected. Readiness and all actual PID counts must prove the intended route; cold fallback is not counted as retained execution.

## Retained and changed guarantees

- Retained: AppContainer/Job, pinned full runtime, Wasm memory boundaries, fresh Store/Instance per call, existing fuel/memory/result limits, per-Node grants and input-generation checks, explicit cancellation, and original process/IO/Job/ACL/profile cleanup before lifetime quota reuse.
- Changed: successful input completion does not destroy its process. Previous input remnants may remain in process allocations despite Store disposal. This is not input erasure or per-input physical termination.
- Grouped additionally removes the OS boundary between admitted Nodes and shares their process-fault domain. A group cancellation or runtime failure can affect the other group members. No runtime-escape protection between co-resident modules is claimed.
- The Core stays in a different process. Third-party/default eligibility and external permissions are unchanged. Any decision to extend eligibility or change user-visible protection requires the policy proposal to state those effects explicitly.

## Measurement and acceptance evidence

`scripts/benchmark-primitive-retention.mjs` uses the actual External Host, package manager, verified fixed modules, consent, Core and AppContainer route. It supports identity/mixed serial1/2/4/8/16/20, cold/steady and sequential/100ms/20ms inputs. Same-binary route order must be balanced; capture every attempted input, correct/superseded/busy/failure denominator, raw trace, client latency, lifetime status, startup/regeneration and Job CPU/peak memory. Full guest-inclusive intervals are conservative overhead upper bounds, not guest-subtracted measurements. No independent percentiles are subtracted or added.

The experiment must also exercise empty/Unicode/max input, output limits/recovery, current-input supersession, active cancellation, protection/revoke, plan/session replacement, faults and recovery, exact original process cleanup and absence of late adoption. Existing fixed-retained regression tests must continue to pass. Local WinHTTP platform failures remain explicit and are not excused by unrelated successful checks.

Final report must identify measured coverage and unverified conditions, recommend adopt/revise/reject separately for each route, and distinguish an experimental technical selection from default/product adoption. #95 is not complete merely because this subset is fast.

## Initial local functional evidence (not an adoption verdict)

Release all-target Clippy and Host/Worker/Manager builds passed. Actual normal-Host runs used the same Host SHA `cfad2366c13ff276033af2ab36976b1cf9fe04def0e1a006ece4c1bf4e9d67ab`, Worker `17ac3d3246f3e96963a555a6019d9e6f99e8d37bdbdc98b7d0cd9a177cb3d367`, Manager `a30872f5dc0f299a2de690448021d6d7aa1940f30e22ca5a3f3c5c1affa7acb9`, and pinned DLL. Each successful smoke had one initial plus three warm correct inputs; intended PID counts and all stage replies were checked, then original Workers and execution/buffer reservations settled before Host exit. The outer Job confirmed no surviving descendants. These tiny samples do not establish tail performance.

| Route / chain | Retained processes | Warm client median / maximum ms | Whole Job CPU ms | Whole Job peak commit bytes |
| --- | ---: | ---: | ---: | ---: |
| Separate /2 | 2 | 6.144 /10.661 | 10671.875 | 148824064 |
| Grouped /2 | 1 | 9.154 /11.944 | 8171.875 | 110854144 |
| Grouped /20 | 5 | 36.918 /63.295 | 17625.000 | 260538368 |

Job accounting includes driver, installation, preparation and shutdown, not just warm calls. Peak commit is not total physical RAM or resident mapped pages. Cold preparation through first result in the separate2 smoke was9299.777ms. Initial20-separated smoke timed out before all Workers became ready and submitted no inputs; cause unresolved, with failure/report retained and no successful Job accounting claimed. A subsequent system-memory probe during the grouped experiment observed4,179,914,752 total physical bytes and96% load; this is context, not causal attribution of the earlier failure.

For grouped20, all three warm graphs had complete same-Host-clock traces. Across60 Node calls, guest-inclusive ready-to-resolved median/P95/P99 was1365/3644/10722us; Host accepted-to-published median/maximum34098/59616us. These include Node computation and are not guest-subtracted numbers or20-stage adoption proof. All5 original PID identities and80 total stage replies (initial plus warm) were verified before cleanup.

The first separate2 measurement completed all calls but failed its measurement-only close assertion after incorrectly requiring every ResourceUsage field to reach zero. The existing ledger intentionally retains dedupe entries and bounded result records after Session close. The corrected meter requires zero Worker processes, physical jobs, active runs, queued jobs/input bytes and reserved result buffers, while recording retained records/bytes explicitly. It does not change the ledger, fixture, production cleanup or expiry policy. The failed report and log remain separate; subsequent successful reports are not used to relabel it as passed. Node-ID route mapping was required because same-kind prepared Nodes share a link; routes now hold weak owners keyed by actual graph Node ID, preserving separate process assignment across repeated kinds.

## Subsequent bounded scheduling and local ABBA evidence

CI43614228879 at3dcf7ed4 failed all four original segments in the Editor regression: unconditional private fields changed the exact default retained-worker JSON shape. The fix preserves the original six fields for default/fixed policy, adding CLI-only diagnostics solely for actual experimental owners. The unchanged real Editor TypeScript parser accepted a live default Rust status and still rejected the old extra-field regression. That focused check does not substitute for full latest CI.

Serializing experimental compiler startup alone did not solve separate20 readiness: `primitive-serial-start-v6` ended with an empty Session and no input submitted. Its failed Job accounting was preserved (CPU38796.875ms, peak commit338444288B, no surviving descendants). Inspection found20 idle coordinator probes could compete with the Core's existing16 pending service requests. Evaluation groups now share a mutex around each fresh control-state RPC; no cached authorization, gate increase or fail-open behavior is introduced. The default/fixed route has no such mutex. Startup permits are cancellation-aware and held through original failed-construction cleanup, released at actual readiness, with the existing10s on-demand readiness bound. Release all-target Clippy/build passed; the two startup scheduling unit tests passed before the final probe-only change.

The subsequent separate20 probe completed one initial and three warm correct inputs, with all20 original PID identities and zero surviving descendants. This supports the scheduling fix operationally; it is not proof of the sole cause of previous failures.

Four subsequent runs used identical Host SHA `83699dbed9cf5d5c8abebdf75f1da076e0083615714da3fe7bbfb6e06eaf719d`, Worker `c28383184291718c1c2a70ad330840d3065b3ae594bea712b7b57d94ac401edb`, Manager `da9286d999fd4e555babb29bf4e1ea8a7d57fd6e752beada7942a2a3be1153de`, and the pinned DLL. Order was grouped/separate/separate/grouped, each with100 warm inputs plus one initial. Every input was correct; intended lifetime process counts, stage reply counts, execution/buffer drain and outer Job descendant exit passed. Labels are `primitive-final-a` through `primitive-final-d`.

| Order / route | Client P50 / P95 / P99 / max ms | Whole Job CPU ms | Whole Job peak commit bytes |
| --- | --- | ---: | ---: |
| A grouped20 | 25.571 /41.220 /165.696 /283.135 | 18078.125 | 177668096 |
| B separate20 | 35.896 /78.050 /102.790 /233.420 | 60359.375 | 410628096 |
| C separate20 | 39.412 /133.317 /247.092 /351.132 | 52234.375 | 414289920 |
| D grouped20 | 42.454 /284.081 /542.729 /651.025 | 24125.000 | 175108096 |

CPU and commit include installation, cold preparation and shutdown. They are not warm-call CPU or unique resident RAM. These are guest-inclusive full-graph client times on this small-memory Windows machine. The later grouped run has much worse tails; no stable speedup, causal memory-pressure attribution or guest-subtracted #95 pass follows. Do not compare these binaries with the earlier initial smoke as an optimization A/B.

Technical recommendation at this point: **revise both routes; do not adopt either into product policy**. Separate20 functions after scheduling fixes but keeps20 processes and substantial lifetime/startup cost. Grouped20 reduces the observed Job commit footprint to roughly43% of separate20 here, but loses inter-Node OS isolation and has unstable tails. That tradeoff currently lacks evidence sufficient for adoption. Continue a bounded opt-in comparison and preserve the existing per-input disposable route as the baseline. #95 remains open, including real distinct modules, branch/join/unused graphs, full admission negatives and default-policy acceptance; current evidence does not complete it.

The dedicated CI comparison and safety segments add actual normal-Host evidence and preserve partial reports, sampled process memory, Job accounting, source and binary hashes. All original four segments and their gates remain required. Comparison includes short1/4-stage smokes and balanced20-stage identity/mixed routes; safety includes20-stage cancellation/protection/revoke/fault/reopen plus paced mixed inputs. These scripts enforce correctness and cleanup, not a predeclared claim of performance success. Latest CI results must be reported independently from local evidence.

## Technical decision from cancellation and paced input

The same-binary disposable20 identity comparator (`primitive-disposable-v1`, one initial plus10 warm) completed correctly with all descendants exited: client P50/P95/max1949.263/4103.410/4103.410ms. It used prepared stock-all with the same22-slot policy and other optimizations, not an intentionally disabled baseline. This is only one small trial with a different sample count from the retained ABBA. Whole Job CPU23640.625ms, peak commit253042688B; one sampled process-memory read was unavailable and remains missing. Retention eliminates a substantial observed preparation/destruction cost, but no stable speedup ratio is claimed.

Current separate20 safety (`primitive-final-safety-v7`,278.609s) passed empty/Unicode, plan/session replacement and stale-key refusal, actual active cancellation (one cancelled while active; two completed before cancellation), protection and original-owner drain, revocation/consent/recovery, checked original Worker fault and recovery, and close/reopen. Final execution/process/buffer owners and all outer Job descendants were zero. Whole Job CPU242984.375ms and peak commit440668160B include repeated preparation and cleanup. It is not a complete negative-admission or input-erasure proof.

The first mixed20 paced graph (`primitive-final-paced-v7-grouped`) was rejected at prepare with `incompatible-ports`: the test generator placed an `any`-root passthrough output before a text-only input. No input ran. The generator was corrected to leave passthrough terminal and keep trim/case text-compatible, without changing any manifest, Core validation or fixture. This failed report and its successful cleanup remain preserved.

The corrected graph has romaji-to-hiragana, alternating trim/case, and terminal passthrough, all from the same pinned official module. It is heterogeneous Node functionality, **not distinct Wasm code**. Local `primitive-final-paced-v8` runs for both routes completed35 attempted/admitted inputs in each of sequential/100ms/20ms scenarios (five independent seven-input bursts). No busy, execution failure, expiry or cancelled outcome was reported. Old result suppression and execution drain were checked by the existing shared input scenario helper.

| Route / interval | Correct / superseded / attempted | Correct response P50 / P95 ms | Last input of each burst: correct / P50 / max ms |
| --- | --- | --- | --- |
| Separate / sequential | 35 /0 /35 | 30.894 /63.632 | 5 /30.894 /41.783 |
| Separate /100ms | 35 /0 /35 | 34.686 /47.176 | 5 /36.487 /41.328 |
| Separate /20ms | 6 /29 /35 | 1513.808 /4464.070 | 5 /2842.352 /4464.070 |
| Grouped / sequential | 35 /0 /35 | 29.833 /61.075 | 5 /29.624 /44.282 |
| Grouped /100ms | 35 /0 /35 | 29.214 /61.016 | 5 /27.575 /29.317 |
| Grouped /20ms | 6 /29 /35 | 2165.371 /7653.919 | 5 /3795.424 /7653.919 |

Supersession itself is allowed and is not counted as an execution failure. The decisive problem is that the latest, unsuperseded inputs took seconds after rapid input. Original input-bearing processes are terminated on cancellation, so readiness/regeneration costs return; exact attribution of these seconds still requires trace evidence. Neither route currently meets interactive use under these20ms bursts. Physical input-callback end P95 was4541.646ms separate and7721.361ms grouped; those values are recorded independently from response latency, not subtracted across samples.

**Decision: revise the implementation and withhold product adoption of both routes.** Keep the experiment default OFF and preserve the current disposable baseline. The next technical work should address bounded recovery after supersession while preserving stale-result fences and actual termination/cleanup requirements; do not remove cancellation or grant checks to make warm numbers look better. Grouped co-residency is not justified as a product security tradeoff by the current results. Primitive eligibility based on reviewed bytes remains a useful experiment, but lowering security level by itself does not solve the measured recovery bottleneck.

This completes the technical adopt/revise/reject recommendation requested for this evaluation, not Issue95. Admission-negative coverage, branch/join/unused graphs, real distinct Wasm code, max/output limits, dictionary/network paths, full original cadence/coverage, CoreP95 and main/default acceptance remain unproved. New paced runs additionally retain each attempted input and per-burst lifetime status, require each latest input to complete correctly, and distinguish allowed busy admission from execution failure. Their success still asserts correctness/cleanup rather than a latency goal.

Current grouped20 safety (`primitive-final-safety-v8`,63.265s) also passed the replacement/cancel/protection/revoke/original-Worker-fault/reopen checks and final zero execution/process owners and outer Job descendants. Whole Job CPU58468.750ms and peak commit187846656B include repeated preparation and shutdown. The strengthened paced recorder was exercised separately in `primitive-ledger-v9` (three bursts per interval,21 inputs per scenario), preserving each attempt and per-burst status and requiring all nine latest inputs correct. This is functional validation of the recorder, not a rerun that supersedes the five-burst latency evidence.

[Local evidence summary with raw-report/Job hashes, binaries, failures, outcomes and resource accounting](evidence/primitive-retention-local-20261115.json) contains13 observed runs, including the two failed readiness/invalid-graph cases. Raw reports and logs remain in the task workspace under those exact labels. This local summary is separate from the latest full CI and its downloadable artifacts; it does not turn an incomplete/cancelled CI into a pass.
