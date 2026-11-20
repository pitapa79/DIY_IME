# Metadata preparation code-cache experiment (#140)

Refs #109. This is an internal, default-OFF experiment on the unmerged S1
dependency stack, not product adoption or a completed performance result.
Start main: `1cd762e540faaaf5b69e43cd6d411ce7ac8a05af`.
Dependency: PR #136, `57fb978933e949cdbb8ed80e5afa33fa8e3da6cb`.

## Selection and ownership

Both `DIY_EXPERIMENT_PREPARATION_CODE_CACHE=1` and
`DIY_EXPERIMENT_COMPILED_JIT=1` are required for the new metadata route.
Only `describe` and `validate` can select it. The default-OFF route and the
existing fixed-official source/policy cache route retain their previous call
implementation. A shared official cache cell alone cannot classify a different
module as official. Ordinary Node execution does not use this new selector.

The existing captured artifact owns the module, Worker, pinned runtime and
resource policy. Its existing compiler-only AppContainer produces the code;
the original process/IO/Job/ACL/profile cleanup must succeed before its output
is accepted. Existing output validation binds the source digest, fuel and memory
policy; immutable code files and the shared finite code budget own its lifetime.
Packages cannot supply serialized native code. A newly captured module/policy
gets a new pending cache, including when it shares the runtime files.

Each metadata request uses the prepared-JIT transport with a fresh AppContainer
Worker and fresh Store/Instance. The compiler receives no metadata input. No
Worker, Store, Instance, metadata reply or past input is cached. Default and every
distinct configured value are still validated, and package-definition equality,
permission review and qualification remain in their existing callers.

When explicitly requested, disabled/busy/unavailable compilation or cancellation
refuses before metadata guest execution; there is no raw-JIT fallback or guest
retry. The retained/recovery compiler selector keeps its existing behavior.
Per-call limits and physical ending/accounting are unchanged.

## Verification and limits

The ignored real-Windows test
`isolated_worker::adapter::metadata_preparation::tests::real_metadata_code_cache_reuses_only_code_and_refuses_unavailable_producers`
requires `DIY_RANGE_WORKER_BINARY`, `DIY_RANGE_RUNTIME_DLL`,
`DIY_METADATA_FIXTURE_WASM`, `DIY_METADATA_OFFICIAL_WASM` and the two experiment flags above. It checks a
third-party fixture even with a shared legacy cell, one compiler over repeated
describe/validate requests, exact invalid-config refusal, cancelled admission,
public preparation with default/distinct config validation, independent captured
cache state, explicit OFF control, zero active processes and final code-budget /
workspace release. It also checks that the actual pinned official code still
uses the shared legacy compiled cell while its generic JIT cache stays pending.
The fixture owns its staging parents; code-cache cleanup must leave them empty
before the fixture removes those empty directories. CI adds this canary to the
existing runtime-cache segment.

The packaged Windows Host comparison uses the same source, fixtures and binaries
in OFF → ON → ON → OFF order. Each arm runs fresh Host sessions for 1, 4 and 20
serial stages, with 20 ms, 100 ms and sequential input. The only experiment flag
changed between arms is `DIY_EXPERIMENT_PREPARATION_CODE_CACHE`.

The report keeps manager review and commit, Host prepare, approve and activate,
latency of the first burst's latest correct input, and elapsed time from the
Host prepare request to that same correct observation. The readiness endpoint is
the first positive status observed after an input burst. The first input follows
activation and trace setup. Readiness may have been reached before the status
observation. The report
preserves all input outcomes, Job CPU and memory, source and binary hashes and
final Job and workspace cleanup. A missing Job memory read keeps the arm failed.

The latest validation is pending on Windows. The comparison measures both setup
and input response because reuse may shift preparation cost from the first
input into setup. Compare the recorded endpoints within their own trials.

The serious latest-input recovery tails, broader #109 boundary/long-run coverage,
normal-path integration and the dependency audit blocker #137 remain separate
unfinished work. This experiment cannot justify completing #109 or merging with
failed required Windows CI. Existing security guarantees and product/default
adoption approval requirements are unchanged.
