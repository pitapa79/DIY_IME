# Host-owned S1 review qualifications (#133)

Parent [#109](https://github.com/pitapa79/DIY_IME/issues/109). Implements the
[approved S1 specification](wasm-execution-classes-v1.md); technical experiment,
default OFF. Starting main1cd762e540faaaf5b69e43cd6d411ce7ac8a05af, explicit
PR130 dependency3e877b320d3b14f483fac9c65c6c1ddc6b4f069f. No public review API,
SDK change, new authority, product adoption or default activation.

## Review policy and evidence

S1 qualification is no longer decided by six kind names and one module hash.
The Host reviews a deliberately limited risk profile using actual prepared
assets and its own execution implementation:

- The private WasmNode is produced by isolated describe/config validation and
  Core definition validation, with verified package identity. Raw artifacts and
  guest-supplied classification/hash flags cannot construct a qualification.
- Inspect the actual captured Wasm import section. Allow no imports, or function
  imports named diyime_resource_v1.read; deny other modules/functions and imported
  memories/tables/globals/tags. The bounded parser checks section/name/LEB lengths
  and duplicate/trailing import sections. It does not replace Wasmtime validation
  or the isolated ABI/function-signature validation already performed by prepare.
- Require absent actual dictionary, prepared-dictionary and network broker
  bindings, including secret-backed network plans. A declaration of no capability
  alone is insufficient. The retained request receives an empty, authorized,
  per-call resource scope. No WASI or ambient OS imports are installed.
- Require finite process memory, Wasm memory, fuel, walltime and termination
  policy supported by the existing retained driver. The actual native path
  creates a fresh limited Store/Instance and destroys its callback before RESULT.
  This is the Host's risk assessment of this bounded implementation profile;
  it is not a claim to prove arbitrary guest semantics or memory wiping.

The non-serializable reviewed implementation captures the actual immutable
module/Worker/runtime/policy owner and full validated definition. A separate
qualification binds the exact validated configuration, verified package identity,
node id and sealed grant in the current Session/plan. Immutable Arc identity
avoids hashing entire modules on each input and conservatively requires fresh
review after recapture, even when the displayed hash has not changed.

## Lifetime and invalidation

Before coordinator installation, before native preparation and on each delivery,
check the qualification against the actual Node/configuration. The sealed
controller verifies current grant tokens and protection epoch under the existing
gate lock, without an extra Core polling command per input. Each resource lease
continues to linearize delivery/publication with cancellation and revocation.
A changed Session, plan, node, code/runtime/Worker/policy, definition/configuration,
package or authority cannot substitute for the qualified owner. Revoke/reapprove
or protection on/off never resurrects an old qualification or sealed controller.

Unknown/unsupported review during explicit S1 selection refuses activation.
Qualification mismatch during routing closes the retained owner and rejects the
callback before delivery; it does not automatically retry the call. Existing
legacy fallback remains subject to its original authorization and cleanup rules.
Normal default S0 and the independent fixed official route are unchanged.
S1 permission does not imply S2 permission: grouped experiments retain the prior
separate audited module/kind gate.

Existing ownership remains authoritative: lifetime600s, idle60s, input120s,
max4096 calls, finite input/reply reservations and per-call Worker limits. These
are existing technical bounds, not newly approved product recovery allowances.
Original Worker/IO/Job/ACL/profile/code owners must end before physical capacity
is returned. S1 does not promise normal-input physical termination or complete
erasure of old input from process allocations. Node-separated OS isolation and
necessary protection/revocation/cancellation termination remain required.

## Verification and remaining work

Matching tests use two valid Wasm modules whose executable i32.const differs,
not custom-section changes. They cover generic kind/version independence,
code/Worker/runtime/policy/definition/configuration/package substitution,
authority/context mismatch, revoke/reapprove and protection epochs. Tiny unit
runtime assets are matching fixtures, not evidence of production native execution.
Actual-import parser tests cover forbidden capabilities and malformed bounds.

The actual Windows checker uses verified package import, explicit consent/Core,
normal Host and AppContainer with two genuinely different executable fixture
builds and official identity in the same graph. It checks S0, S1 single/4/20 stages,
plus cancellation/protection/revocation/plan-Session replacement/fault/reopen and
original final cleanup. Cases run once under the existing300s owned Job/330s
observation bound; failures, partial reports, binary/module/source hashes and
CPU/RAM/ending evidence are artifacts. Existing CI cases and nine45min segments
remain required. Native results and final-source CI are pending.

This does not complete Goal109's full branch/parallel/unused/long-run/context
matrix, semantic-work attribution, response-time resolution or main integration.
No OS document/input-target adapter is added here. Unsupported broker-dependent
S1 profiles are refused; they remain future work, not silently accepted.
