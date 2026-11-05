# Approved dictionary snapshots for Wasm Nodes

Issue #284; builds on the [external Host](wasm-external-host.md), [permission subjects](wasm-permissions.md) and [execution GUI](wasm-execution-gui.md). The Node repository implements the guest `prepare` operation in PR #12. Dictionary parsing and lookup remain in that repository; the Host only grants, captures and transports bounded resources.

## Trusted binding configuration

The fixed package-manager configuration, including `packageManager` inside the external Host configuration, accepts optional `dictionaryBindings`:

```json
{
  "dictionaryBindings": {
    "dictionary": {
      "root": "C:/trusted-host/dictionaries",
      "referencePointer": "/path",
      "maxFileBytes": 1048576,
      "maxTotalBytes": 4194304,
      "maxFiles": 16,
      "budgetMs": 5000
    }
  }
}
```

This is a fragment of trusted deployment configuration, not a browser operation. Roots, JSON pointers and all positive limits are fixed by the launcher. The root must exist and satisfy the existing ResourceRoot checks. The package manifest declares `{"id":"dictionary.read","binding":"dictionary"}`. The pointer selects a relative filename in each exact validated Node config. Root metadata is checked on startup, while verification and review never open dictionary file bodies. Unknown capabilities, missing providers and invalid pointers/references are refused. The [installation verifier](wasm-installation-proof.md) can import supported dictionary declarations without a configured root. Runtime verification still requires the actual fixed provider and normal resource consent.

References use forward-slash relative components. Absolute paths, parent traversal, empty/dot components, backslashes, device names, alternate streams, wildcard/control characters and trailing dots/spaces are rejected. ResourceRoot additionally rejects reparse points/junctions and validates the opened file handles against the fixed root. Its sharing locks, file identity checks and per-file, aggregate, file-count and elapsed IO limits apply. The native root stays in Host memory; only a policy SHA256 containing its identity and limits enters review subjects.

## Consent and preparation

A `resourceRead` request displays its logical binding, relative reference and policy SHA256, alongside the affected graph instances. Its subject includes the exact implementation/config/resource policy. Execution and source-port input-flow approvals are also required. Filename/config, code, capability or selected binding policy changes require the corresponding new grants; runtime keystrokes do not. Relative filenames are visible in review, but saved permissions contain only subject fingerprints.

Activation checks every grant before reading. It seals and registers an invalidatable private binding, releases the permission-book lock, then captures every selected file into one bounded batch per provider. All capture completes before isolated guest preparation, so Worker startup/JIT time does not consume the file IO budget. Sharing handles remain held throughout this preparation. Each Node receives only its own config's immutable resource map, with relative references as keys. The guest receives no native root, full RuntimeInput, graph, grant token or controller.

Guest `prepare` validates the captured resources and returns strict `{valid:true,prepared:boolean}`. A failed file read or guest validation publishes no active Session. The prepared controller stays private until every resource is valid and the binding remains current. A new PermissionPlan creates fresh preparation cells even if its verified implementation proof is reused. Instances of the same implementation share the exact compiled/prepared cells for their already-validated configs.

During execution, the adapter checks its execution gate before projecting the prepared bytes into the guest request. File changes/deletion after activation cannot change the active snapshot. An explicit new preparation captures new bytes. Dictionary parsing, UTF-8/format diagnostics and conversion meaning belong to the Node implementation.

## Withdrawal and ending

Cancellation, resource-grant revocation, protection and EOF invalidate the binding and cancel any preparation Worker. File IO is synchronous: cancellation is checked around each operation and the preparation owner remains held until it returns. Guest work retains the existing AppContainer, Job, process-handle and pipe ownership checks. A control acknowledgment requests withdrawal; it is not a claim of physical ending. The ordinary preparation slot is released only after its Worker and IO have actually ended. Turning protection off does not restore old snapshots/controllers/results.

## Verification and scope

Inline unit tests cover no reads during review, missing/unsafe references, policy identity, sharing locks, immutable bytes, provider/config separation, cancellation and fresh preparation cells. `scripts/check-wasm-dictionary.mjs` drives the real manager/Host/AppContainer with resource projection fixtures, consent denial, file failures, snapshot refresh, route/resource grant separation, junction refusal, preparation cancellation/protection/revocation, infinite execution, EOF and policy changes. It is part of `scripts/check-wasm-v3.ps1`. GUI/API tests cover explicit resource selection and strict review decoding. The actual Node-repository Wasm was also tested locally through romaji conversion and dictionary lookup, including invalid UTF-8 diagnostics.

This broker grants read-only dictionary snapshots. Network/API-secret brokers, official package bundling and cutover, performance work and IME integration remain separate work. No ambient filesystem/network access or Native fallback is added.
