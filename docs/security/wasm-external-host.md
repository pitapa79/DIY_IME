# Installed pure-Wasm execution Host

Issue #278, following [permission subjects](wasm-permissions.md). This production Rust Host loads installed packages, performs actual isolated validation for every used kind/config, creates the canonical four-system-plus-verified-Wasm Registry and runs only a fully consented sealed Core Session. The HTTP/GUI consent/execution panel is the next integration step. Existing legacy/v2/v3 paths remain unchanged.

diy-ime2-external-host takes exactly one trusted configuration file path. The browser/package cannot supply roots, worker executables, DLLs or policy. Configuration has packageManager (the same nested fixed manager configuration documented in wasm-package-gui.md), permissionRoot (a Host-owned directory inaccessible to guests), resourcePolicy (all 13 existing ResourcePolicy fields), positive requestBytes and responseBytes >= 1024. Optional permissionBytes defaults to 16 MiB. Optional secretRoot selects the [encrypted Host secret vault](host-secret-vault.md); no configured root means no secret management storage. The transport JSON length must fit u32. Unknown configuration fields are rejected. Request and response sizes and execution budgets are Host-selected policy.

The Host shares ManagerConfig's immutable asset capture/init, without importing the package manager's operations. Trusted Worker executable capture is bounded at 128 MiB (the complete debug binary grew to 64.08 MiB); DLL capture stays 256 MiB and module/import limits remain selected policy. The DLL pin, AppContainer token/SID, Job/handle/environment restrictions and actual process ending checks are unchanged. Workers refuse execution without OS isolation. No Native fallback exists.

## Framing and operations

stdin uses u32 JSON length/u64 payload length (little-endian), then strict JSON {id,operation}. This Host requires payload length zero; CODE is installed through the separate package manager. stdout has correlated JSON lines {id,ok,data|error}. IDs are bounded opaque ASCII strings. Malformed framing/JSON/IDs closes the Host cooperatively. A parseable correlated request with invalid fields is refused without losing the Host. No stderr is relayed as structured output; parent bridges must drain/discard raw stderr.

method variants (all strict struct variants):

- definitions, status, saved.
- prepare {graph}: closes the previous pending/active Session and waits for actual old physical/queued/active work to end before validation. All needed packages are loaded from fixed content-addressed catalog objects, kind/version checked, configs grouped and live-validated, then captured by PermissionPlan. No RuntimeInput is part of graph preparation. A random Host reviewId, actual Core epoch, requests and missing IDs are returned.
- approve {reviewId,ids,save}: only current immutable review IDs, with save=false for ThisSession or true for Saved.
- activate {reviewId}: all permissions must match before the sole controller is sealed. There is no raw controller or graph-switch command.
- run {request}: the existing strict StartRequest, preserving decimal start keys, expected plan version, typed targets, budgets and optional RuntimeInput.
- getRun/cancelRun {key}, getStart {startKey}, invalidateInput {expectedPlanVersion,revision}.
- closeSession: cancels/drops its owners and waits for genuine physical ending.
- cancel {requestId}: requests cancellation of a current preparation/Host operation. Acknowledgment is not physical ending.
- protection {enabled}: manual protection only. No browser field can turn off automatic OS protection; that trusted future input adapter is not implemented here.
- revoke {id}: immediate authority/result withdrawal through PermissionBook, including durable-write error reporting.
- close or EOF: no new work, cancel preparation, protect/drop all Sessions, wait for actual work/IO ending, then exit. close is acknowledged by true process exit, not an early synthetic stopped flag.

Only one ordinary operation owns the preparation/response slot; busy requests are refused. A separate reader handles cancellation/protection/revocation/EOF during preparation. The bounded channel never holds the publication-generation mutex across a blocking send.

## Result and context boundaries

Core creates the actual epoch/version and owns start-key replay, querying, cancellation, input invalidation and physical resource release. Status contains phase/protected/epoch/Core WireState, resource usage and actual captured Worker process counts; it contains no RuntimeInput text or Packet. Core WireState serializes large counters/identifiers through its existing strict wire contract.

Stale retained getRun/cancelRun payloads have result suppressed when current is false. Protection/revocation/cancel generations are checked again under the stdout writer lock. Once a protection/revocation acknowledgment is published, a response from the older generation cannot follow it with input/result data. The UI must additionally clear already-delivered results and filter epoch/graph/input revisions.

Used package authorization subjects are reconciled against the current catalog before ordinary operations. Changed/missing implementations or an unreadable catalog invalidate their pending/active Session; new inputs or old payload queries cannot use that binding. Unrelated package changes and metadata-only changes do not change the authorization subject. An existing untrusted producer still owns its actual physical slot until ending. A subsequent prepare waits for that end, and new implementation subjects need fresh consent.

Saved approvals contain fingerprints only, as documented by PermissionBook. Runtime input travels only through memory and stdin/stdout pipes. The Host does not write it to configuration, package files, temporary request files or status responses. A graph may explicitly include the trusted e2e.output debug adapter, whose stderr is discarded by a connected bridge.

## Verification and remaining work

scripts/check-external-host.mjs drives the actual binaries with synthetic data: install a package, strict/path/authority rejection and recovery, no legacy processing fallback, deny before consent, ThisSession/Saved/restart, real data flow, start replay/query, input invalidation/stale suppression, wrong epoch, real infinite execution and protection, publication race, revocation, old-work drain before new prepare, implementation update while running, validation cancellation, recovery and EOF. Fixtures add validateHold only for this cancellation case; formal Node contracts are unchanged. scripts/check-wasm-v3.ps1 runs the test with the existing real-isolation regressions.

This is the execution backend. The next GUI/HTTP layer must own its fixed configuration, review/Session association and protection controls, clear delivered buffers, keep result-unknown start keys queryable and wait for true Host ending on close. The [dictionary snapshot broker](wasm-dictionary-broker.md) adds explicit resource consent and isolated preparation. Network/API-secret brokers, official package production/bundling/cutover and performance/IME integration remain later work. A crashed Host is not automatically restarted with old context.