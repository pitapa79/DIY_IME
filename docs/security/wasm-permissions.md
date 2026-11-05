# Verified Wasm permission subjects and sealed Sessions

Issue #276; follows [GUI import](wasm-package-gui.md). This is the Host authority library. The next execution/GUI integration must use it; the existing import dialog does not yet create execution grants.

## Immutable approval evidence

PermissionPlan::capture accepts a graph and per-instance VerifiedPackage proofs. It builds its own Registry from the four canonical Host system adapters and those exact Wasm implementations; no caller-composed Registry or same-kind Native/deferred replacement is accepted. Every processing Node must have an isolated verified proof. Proofs sharing a kind must have identical captured implementation/authority; only their already-validated exact configs can be combined. Unknown/native processing implementations cannot label themselves trusted in a graph. Core validates exact prepared configs, ports/types, required inputs and cycles. PackageVerifier::verify_configs permits exact graph configs to be validated on the Host preparation thread before Core compilation. No guest is called from the Core actor's synchronous preparation path.

Execute subject SHA256 covers the actual captured Worker/DLL/module identity, package authorization subject, complete Node definition and a permission schema domain. Package labels and SemVer alone do not grant or change it. The existing package subject includes sorted capability declarations. The [dictionary broker](wasm-dictionary-broker.md) supports explicit dictionary.read bindings and resourceRead subjects; unsupported capabilities still fail closed.

For each Host source output port that can reach an external Node, an inputFlow subject includes that endpoint, destination instance, all nodes/definitions/effective configs and implementations on potentially reaching paths, and their edges. Branches are treated conservatively; actual NoOutput/conditions do not authorize a narrower flow. Fan-in/fan-out and transitive paths are covered without enumerating exponentially many individual paths. Adjacency lists and cached backward reach sets avoid repeated whole-edge scans. capture_with_cancel checks the preparation token between stages; Core compilation remains its existing synchronous operation.

Changing implementation, definition, requested capabilities, relevant config or route yields a new subject. Unrelated branches and graph array ordering do not. Every implementation is approved once per selected lifetime even with multiple instances; each source-port/destination route is separate. Keystrokes, runtime input revisions, Packet content and run IDs are absent from these subjects. Fixed graph config is fingerprinted, but its plaintext is not saved in the permission book.

ApprovalSubject and PermissionPlan have private constructors and no Deserialize/string-to-authority API. Serialized review IDs are display/selection identifiers, not executable permits.

## Host lifecycle

The trusted launcher selects/owns the permission data directory, outside guest-visible caches, with no guest write access. PermissionBook::open requires that directory to exist, canonicalizes it and rejects reparse/storage children and holds a share-mode-zero Windows handle on permissions.lock for its whole lifetime. A second live Host cannot cache another copy of the saved grants.

open_session consumes the exact compiled snapshot from PermissionPlan and creates a private ready Core controller. PendingPermissionSession exposes review requests and missing IDs, never that raw controller. approve accepts only IDs present in that immutable Host-held review, either ThisSession or Saved; unknown/duplicate/empty selections are refused. UI handlers must derive the pending Session from their own current Host state, never a browser-supplied raw controller/epoch.

activate requires every execute and input-flow subject, uses the controller's actual epoch/version, grants the exact targets and immediately seals the sole controller. PermissionSession exposes only GuardedController, which cannot prepare or switch to a different graph. There is no API to take an unrelated ExecutionPlan and combine it with an approved proof. Dropping the owner, revocation or protection closes retained controller clones/results. A graph change needs a new immutable Session; unchanged saved subjects can be reused without another prompt.

ThisSession is memory-only and removed on pending/active Session close. Saved stores only a strict formatVersion=1 and sorted SHA256 IDs in permissions.json. It excludes Session/run keys, input/config/Packet text and secret values. An exclusive owner plus synced temporary file/same-directory atomic replacement prevents a partial saved grant. Selection limits are Host policy; the book file size is an explicit argument.

A failed save creates no new persistent or in-memory saved authority. revoke_id removes the saved and all Session-only grants in memory immediately and invalidates affected live bindings before writing the book. If durable revocation fails, its error must be shown: live authority is gone, but the previous on-disk grant may still exist across restart. Do not claim successful durable revocation in that case. Book directories and ancestor ACLs are trusted Host deployment state; this is not protection from another process already holding the user's full filesystem authority.

Manual OR automatic protection invalidates pending generations and live bindings on every transition. Turning it off cannot resurrect an old pending/active Session or retained result. Saved choices remain for a newly created Session. Explicit reapproval after revocation also uses a fresh Host-held Session.

Withdrawal invalidates visibility and requests real cancellation immediately. Core/adapter continues to own physical execution until the original Worker and IO actually end. PermissionSession drop is not an assertion that the OS process has already exited; callers use the existing Host usage/termination observation before reporting physical release.

## Evidence and remaining work

Real-file unit tests cover exclusive ownership, Session-only absence from disk, saved reopen, malformed/unknown/duplicate/oversized book refusal, actual sharing-lock save failure, immediate revocation despite failed durable writing, and protection/close generations.

wasm_permission_probe uses actual AppContainer Wasm and Core Sessions. It checks default deny and forged selections, execute/input-flow approval, same-code instance deduplication, order/unrelated branch stability, source/config/code changes, metadata stability, unsupported added capabilities, ThisSession expiry, saved reopen/reuse, protected pending and retained Sessions, real infinite guest withdrawal after OS process start, true exit/slot release and recovery. scripts/check-wasm-v3.ps1 runs it alongside all prior regressions.

GUI consent controls and external Registry execution, typed dictionary/network/API-secret brokers, official packages/bundle cutover, and full IME integration are not completed by this library PR. Existing legacy Registry/Editor behavior and formal contracts remain unchanged.
## Canonical system-only plans (#282)

A graph containing only input.text, demo.packet, e2e.input and e2e.output has no external execution or input-flow subjects. PermissionPlan still compiles it using its own canonical Registry and no caller-supplied implementation. The permission layer can internally seal that exact private controller without an external grant target. The public arbitrary-controller ExecutionGate.seal entry continues refusing an empty grant set; this is not a Native-processing fallback or a general permission bypass.

Each permission Binding directly owns a sealed-controller clone and closes it on invalidation, in addition to revoking its external job targets. Thus a graph with zero external targets still loses retained controllers/results on manual/automatic protection or Session-owner closure. Unprotect never resurrects the old Session. Failed sealing revokes any already-created job grants before returning. No synthetic permission subject is minted or persisted for trusted system Nodes.

The unit test compiles/runs a real input.text graph, refuses an unverified processing kind and public empty-target sealing, and checks protection/unprotect and retained clones after owner drop. The production Host integration also prepares with zero approval requests, executes the system output, observes zero guest Worker processes and rejects old payloads after protection/unprotect. Formal Node fixtures and runtime SDK remain unchanged.
