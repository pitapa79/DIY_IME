# Wasm network and secret-use integration

Issue #294 connects [the fixed Host HTTP primitive](fixed-network-broker.md) to actual isolated guest execution, Core operation ownership, permission review and the GUI. Processing Nodes, the immutable Core SDK and the meaning of existing packet fixtures are unchanged.

## Launcher configuration and package claims

The execution Host accepts optional, strict `network: {bindings,limits}` in its fixed configuration file. `bindings` uses `HttpEndpointConfig` from the HTTP primitive. `limits` contains positive `maxCalls`, `maxRequestBytes`, `maxResponseBytes`, `maxContinuationBytes` and `budgetMs`, covering a single physical Node invocation across all its continuation hops. The browser and graph cannot replace this configuration. With no provider, a package requesting network/secret use is refused during runtime preparation.

For example, a launcher may select:

```json
{
  "network": {
    "bindings": {
      "api": {
        "host": "service.example",
        "port": 443,
        "method": "POST",
        "path": "/fixed-service",
        "contentType": "application/octet-stream",
        "requestBytes": 4096,
        "responseBytes": 8192,
        "headerBytes": 8192,
        "timeouts": {
          "resolveMs": 3000,
          "connectMs": 5000,
          "sendMs": 5000,
          "receiveMs": 15000,
          "budgetMs": 20000
        },
        "authentication": {
          "secret": "service",
          "header": "Authorization",
          "prefix": "Bearer "
        }
      }
    },
    "limits": {
      "maxCalls": 2,
      "maxRequestBytes": 8192,
      "maxResponseBytes": 16384,
      "maxContinuationBytes": 4096,
      "budgetMs": 60000
    }
  }
}
```

This is an illustrative fixed configuration, not a real API address. HTTPS defaults to true. The trusted launcher still supplies the separate package manager, permission root, optional secret root, resource policy and IPC limits. Proxy/redirect/ambient-credential/TLS rules remain those of the primitive.

A manifest declares `network.request` for each selected logical binding. If that endpoint uses authentication, the manifest must also declare `secret.use` for the same binding. Extra secret use on an unauthenticated endpoint, a secret with no matching network binding, unknown capabilities and unavailable bindings are refused. A guest call cannot create a missing declaration. A package with no network capability may still execute ordinary pure operations; an emitted call then fails without network IO.

Installation evidence recognizes supported declarations without opening the vault, capturing a runtime provider, reading dictionary contents or doing HTTP. It remains non-executable. Runtime verification captures the current encrypted vault record and exact provider policy on each new graph preparation, then independently validates the guest definition and exact configurations in isolation. Same-kind proofs may merge only when their package/runtime/authority/network policy and provider identity match.

## Separate consent

The permission plan adds `networkUse` and `secretUse` to existing execute, source-port input-flow and dictionary-read requests. Their subjects cover actual implementation, exact Node configuration, normalized fixed endpoint policy, aggregate limits and selected vault identity/version. Review shows the fixed method/destination, invocation limits and optional secret logical ID/random version. It exports no native root, credential, ciphertext, authentication header or private request body. The destination path/query is launcher configuration and must not contain credentials.

Every request must be granted before activation. Once/ThisSession and saved grants use the existing PermissionBook; saved storage contains subject hashes only. Changing code, configuration, destination, limits, vault location or credential version requires new matching subjects. A changed source route independently requires a new input-flow grant. UI checkboxes expose communication and credential use separately and start unchecked. Selecting communication does not silently grant secret use or start execution.

Credential management retains its existing semantics: mutation closes the old Session and waits for all physical work before writing the new record. The next preparation captures its current version. An old immutable capture has no independent authority after its lease/controller is invalidated.

## Additive guest-v1 continuation

No guest OS/network import or WASI service is added. A successful execution can return either its existing final `data` outputs or this exact alternative envelope:

```json
{
  "apiVersion": 1,
  "ok": true,
  "call": {
    "binding": "api",
    "body": [104, 105],
    "continuation": {"step": 1}
  }
}
```

`body` is a byte array. `continuation` must be present and may be any bounded JSON value, including null. It is untrusted guest state, not a Host permission or handle. The Host refuses mixed call/output/error envelopes, unknown fields, invalid byte values and duplicate keys anywhere in the original JSON, including escaped-equivalent keys. A normal output port named `call` remains ordinary output data and cannot request HTTP. Describe, validate and resource prepare do not accept the call envelope.

After the requesting Worker and its pipe threads have actually ended and its handles/cache/profile are closed, the adapter checks the current lease/cancellation/budget, selects a binding only from the verified Node proof and performs one bounded exchange. It never retries or automatically resends. A later hop is refused before dispatch if its request would exceed the aggregate request/call allowance. Remaining aggregate response allowance tightens the response read limit for that hop; it is not checked only after reading the complete body.

A fresh isolated Worker receives the same node identity and an operation containing exactly `method:"resume"`, exact `config`, the opaque guest `continuation`, and `response:{status,body}`. If the Node has approved dictionary snapshots, its own immutable resource projection is also supplied. It receives no original input map, full RuntimeInput, input revision, RunKey, lease, permissions, native paths, headers or credential. Guest state needed after the call belongs in the bounded continuation. Existing pure guests and their exported ABI functions continue unchanged; a guest that never emits a call never receives a resume. Additive Node-repository helpers are subsequent SDK work.

## Physical ownership and withdrawal

The adapter retains the same Core physical job, CompletionPermit, invocation and permission lease across initial Worker, HTTP and every fresh Worker. It does not publish an intermediate response or release a slot between hops. Policy/cancellation and cooperative budget checks surround Worker calls, HTTP operations, resume construction and final publication. Final enqueue remains linearized with the gate check.

Cancellation, changed input, protection, revoked consent and closed sessions immediately withdraw permission to publish late data. An already running synchronous OS call remains owned until it returns and its handles close. A blocked call may outlive the selected cooperative wall budget. A request already accepted or sent can have remote side effects; cancelling does not retract those effects. The cancellation acknowledgment is not a physical-ending receipt. No subsequent hop is dispatched after withdrawal is observed, and no old response restores a Session or result. Secret rotation and EOF wait for true Core/IO ending.

The primitive's bounds, TLS checks, Host-only credential injection, literal-echo refusal and memory-clearing limits still apply. A remote recipient approved to receive a credential can transform data; this is not a secrecy guarantee against that recipient.

## Verification

### Required Windows features (#318)

The broker requires every configured WinHTTP security option to succeed. An option rejected with `ERROR_WINHTTP_INVALID_OPTION` (12009) returns `network-platform`, with only the numeric option ID and OS error in a fixed diagnostic. Other HTTP failures remain `network-io`. Cancellation/permission withdrawal is checked again before either diagnostic can be returned. No option is skipped and no request is sent after a required setting fails.

On the tested Windows 10 22H2 installation (build 19045), a session-only probe reproduced OS 12009 for `WINHTTP_OPTION_DISABLE_GLOBAL_POOLING` (195). Separate sessions accepted `WINHTTP_OPTION_DISABLE_SECURE_PROTOCOL_FALLBACK` (144) and `WINHTTP_OPTION_FAILED_CONNECTION_RETRIES` (162, zero retries). The probe sent no requests and closed all three session handles. The existing HTTP/TLS tests on this installation stop before the server-side assertions.

Public Windows Server 2025 runner jobs passed the real Wasm ownership and policy checks and the Editor regression, including the network path. The runner results establish coverage for that environment. [Windows CI 42010526169](https://github.com/pitapa79/DIY_IME/actions/runs/42010526169) and [Windows CI 42013397003](https://github.com/pitapa79/DIY_IME/actions/runs/42013397003).

Request-level keep-alive control and session-level cross-session pooling are separate WinHTTP options. The documented connection GUID matching mechanism requires build 22000 or later, so it provides no documented replacement for the rejected option on build 19045. Any replacement must establish the same connection isolation before sending a request. [WinHTTP option flags](https://learn.microsoft.com/en-us/windows/win32/winhttp/option-flags) and [connection GUID requirements](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/ns-winhttp-winhttp_match_connection_guid).

[Microsoft's option reference](https://learn.microsoft.com/en-us/windows/win32/winhttp/option-flags#winhttp_option_disable_global_pooling) recommends disabling cross-session pooling; it separately documents disabling keep-alive. The latter is not assumed to establish equivalent cross-session isolation. No verified fallback or minimum Windows build is adopted here; those remain open in #318. Real-OS tests cover refusal of an unknown required option without a connection and preservation of withdrawal precedence. They do not replace the HTTP/TLS/credential/redirect/cookie/physical-ending suites on a platform supporting all required features.

The actual Wasm fixture covers one and multiple hops, opaque continuation, exact response projection, invalid/mixed/duplicate replies, invalid bytes/missing continuation, undeclared bindings, continuation and per-call/aggregate bounds. A real loopback server and production framed Host verify partial and saved consent, declaration/provider/vault refusal, unauthenticated communication, current credential version, Session-only expiry and source-route reapproval. Unknown-capability installation regression uses `network.unknown` because `network.request` is now deliberately supported for installation; unknown capabilities remain refused.

Held response bodies demonstrate zero live Workers but one retained Core physical job during HTTP. Cancel, input invalidation, protection, secret-use revocation, credential rotation and EOF retain the slot/owner until release, suppress late delivery and avoid another dispatch. Rotation/EOF acknowledgments wait for the operation to finish. The existing cancel record remains a current terminal cancellation when appropriate; input invalidation instead makes the old record stale. Tests assert the respective public result semantics, rather than treating a cancellation record as successful data.

The typed HTTP/client/Host test exercises real network/secret review, partial rejection, approved execution, saved reuse and a held response during credential mutation. DOM/API tests cover independent choices, fixed destination/version/limits display, and strict rejection of private or mixed-scope metadata. The CI Wasm suite runs the production network probe after dictionary integration. All tests use synthetic keys and loopback services. Browser visual inspection remains unavailable under the documented environment restriction.
