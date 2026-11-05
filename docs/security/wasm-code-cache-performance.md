# Immutable Wasm code files and release measurements

The Host reuses captured code files, while every invocation still creates a fresh AppContainer profile/SID, process, Job and private input/output pipes. There is no persistent guest, shared input/context, native fallback, AOT deserialization or Main-process Wasm compilation. The Worker still verifies the full pinned runtime DLL hash and compiles its module with the same fuel/memory policy on each call.

The factory shares only its exact captured Worker executable and pinned DLL. Each package image separately retains its captured `node.wasm`; another package's invocation receives no read grant to that module cache. All paths remain trusted Host/Worker arguments and are absent from the guest context. Runtime/code/config identities and approval fences are unchanged.

Each generated cache contains only these code files. Open read handles deny write/delete sharing for the entire cache lifetime, preventing later rewrite, replacement or deletion. Existing caches from another run are never loaded. A modified/corrupt installation object still goes through the existing hash and isolated proof checks. There is no on-disk executable-artifact cache trusted by filename, guest metadata or a receipt.

A short mutex serializes ACL changes. Each invocation grants its unique SID read/execute access; it revokes only that SID after actual process exit and pipe draining. Other live invocations keep their own grants. Permission-operation failures poison the snapshot and refuse further use. The same owned call retains code references, the Job, profile and physical accounting until ending is confirmed; cancellation does not release an unconfirmed owner.

Retained code files share a 512 MiB budget per captured Host factory, across its package images. Allocation checks exact byte lengths before creating files. A cache releases its reservation only after removing its fixed known files and directory. Failed final cleanup stays charged for that Host lifetime; it does not recursively remove unknown entries. Process crashes may leave public code staging directories, as with the previous per-call staging; neither invocation payloads nor secrets are written there. This is a bound on live Host-owned snapshots, not a global disk quota across independent/crashed Hosts.

Measurements use the same official module: 1,024,375 bytes, SHA256 `105c0ceb2854dba27a747eae5faa6e393bb99a88ce5ab3048a468fa9add88134`. They include actual AppContainer execution and confirmed ending, on the development Windows machine. These are prototype observations, not an IME latency guarantee.

| Eight describe/default-validation calls | Elapsed |
| --- | ---: |
| Original per-call code copies | 4.426 s |
| Reuse of DLL file alone | 3.959 s |
| Captured code files before read-lock hardening | 1.589 s |
| Final retained/read-locked code files, sequential | 1.837 s |
| Final same eight calls, two concurrent callers | 1.517 s |

The sequential run's four preparations took approximately 938/292/295/311 ms, each making two fresh calls; all eight physical owners had ended at return. The concurrent check likewise ended all eight owners. Initial diagnostic Workers measured about 0.39 s for the first open of a newly staged DLL, versus about 0.05 ms for later opens of the same file across distinct profiles. DLL hashing itself took about 9 ms, and module compilation about 80–90 ms. These observations do not establish which OS component caused the first-open delay. No antivirus or isolation settings were changed. Compiler optimization policy remains unchanged.

Reproduce phase measurements with `cargo build --locked --release --example wasm_release_benchmark`, then run the example with `WORKER DLL WORKSPACE MODULE text.stringify` (optionally `--parallel`). Use the existing `scripts/check-wasm-v3.ps1` for real capability/permission/cancellation suites and `scripts/check-windows-distribution.mjs` for the assembled application. Measurements and full-suite results must be recorded for the final candidate; visual QA remains separate.

Tests execute a native synthetic fixture inside actual AppContainers to check disjoint module access, read-only files/directories and revocation of one SID while preserving another. They also check snapshot poisoning, refusal at the shared byte limit, final reservation release and retained-file rewrite/replacement/deletion denial. Formal Node fixtures, SDK snapshot and semantic contracts are unchanged.

The assembled read-locked release application (161 shipped files, same module and graph) completed initial installation in 16.896 s and an existing-catalog restart in 346 ms; four Unicode/tie/unknown/empty runs took 624/622/611/624 ms. The previous assembled application took about 53–62 s initially and 2.1–2.35 s per run. Both checks used empty PATH, explicit/partial consent, saved reopening, true close and launcher-crash descendant termination. The subsequent constructor ownership fix changes failure cleanup only; CI validates the exact final source. The initial Windows Chrome URL-confidence limitation was followed by a successful visual/functional check in a separate in-app browser; see node-source-cutover.md for the scope. These response times remain above practical IME targets.
