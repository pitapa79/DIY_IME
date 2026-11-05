# Installation evidence and runtime authority

Issue #288; prerequisite for official bundles and service-capability provisioning. Installing a Node must not require the user to select its dictionaries or services first, and it must not produce execution authority.

`PackageVerifier::verify_installation` captures the exact immutable package and validates describe/default config in an actual isolated Worker, using the same module pin, asset capture, wire/schema, definition comparison, cancellation and true-ending rules as runtime verification. It returns opaque `VerifiedInstallation`, which exposes only package inspection. It has no Node, controller, grant or resource accessor and no Deserialize or conversion to VerifiedPackage. A compile-fail example checks that an installation proof cannot yield a WasmNode.

The installation verifier recognizes already-supported `dictionary.read` declarations without requiring a configured resource root or reading a dictionary body. Unsupported capability IDs are still refused. A declaration never enables an ambient broker. Exact graph config validation and dictionary reference/policy checks remain runtime work.

Catalog ImportPlan stores only these installation proofs. Existing batch bounds, duplicate/stale review checks, version confirmations, atomic commit and corrupt-object refusal remain unchanged. CLI/GUI import still reports executionPermissionGranted=false. An installed catalog entry is an inventory object, not an executable permit.

Actual graph preparation reads the installed immutable package and calls runtime `verify_configs` again. This requires the real fixed Host providers, exact validated configs and normal PermissionPlan capture. With no configured dictionary provider, runtime preparation fails even though installation succeeded. With a provider, execute/input-flow/resourceRead consent and private snapshot preparation are still required. Nothing bypasses the existing sealed Session or actual Worker/IO ownership rules.

The real package/catalog probe checks installation and CLI commit of a dictionary-declaring package with no provider, then refuses runtime proof creation. The dictionary Host probe and actual HTTP import also install without providers, while an unconfigured execution Host rejects graph preparation. Configured execution then runs the existing complete dictionary consent/snapshot/withdrawal regressions. Definition mismatch, invalid config, unknown capability, cancellation and catalog regressions continue to use actual isolation.
