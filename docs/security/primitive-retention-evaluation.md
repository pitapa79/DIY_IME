# Reviewed primitive retention evaluation

Refs #104 and #95. Evaluation authorized by the maintainer on 2026-11-15 JST, through measurement and a technical adopt/revise/reject recommendation. Default adoption, official Node adoption and a Level3 product security-policy merge remain separate decisions. No performance result is claimed yet.

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
