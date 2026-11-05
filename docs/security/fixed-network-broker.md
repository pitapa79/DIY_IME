# Fixed Host HTTP primitive

Issue #292; prerequisite for the network/secret-use integration in [the Wasm Node split](https://github.com/pitapa79/DIY_IME2/issues/257).

This Rust primitive itself does not create a guest capability, IPC operation or user grant. [The Wasm integration](wasm-network-broker.md) binds it to the Core physical job, current permission lease and explicit network-use/secret-use consent. Unknown capabilities and missing providers still fail closed.

## Fixed destinations and captured policy

Trusted launcher code builds `NetworkBindings` from a strict map of logical names to `HttpEndpointConfig`. A binding selects host, port, HTTPS, method, complete path/query, content type, request/response/header limits and positive resolve/connect/send/receive/cooperative time budgets. The guest cannot choose a URL, method, headers, proxy, credentials or another binding configuration. Hostnames use ASCII DNS labels; numeric IP literals must be canonical. Unencrypted HTTP requires an explicit launcher flag and the exact loopback literal `127.0.0.1` or `::1`. IPv6 destinations and the WinHTTP connection argument use brackets.

Optional authentication selects a fixed vault logical ID, header and prefix. Header names exclude routing, framing, cookie and proxy headers; CR/LF, control characters and non-ASCII header values are refused. GET bodies are refused. Configuration and authentication errors do not return their input values.

`capture` reads the current immutable encrypted vault entry without decrypting it or doing network IO. The opaque, non-deserializable `NetworkPlan` contains the normalized fixed configuration and encrypted reference. Its policy digest includes the binding, full configuration, canonical vault location, selected logical ID and random version. Rotation, deletion, policy changes and a different vault location affect new captures. An existing capture does not itself confer permission: the trusted integration must invalidate its lease when the corresponding session is closed or secret is changed.

`description` exposes only binding, method, fixed destination, policy digest and optional secret ID/version. It exposes no credential, encrypted bytes or native vault path. A destination path/query is launcher configuration and must not contain a secret. This primitive accepts a Rust authority-check callback supplied by trusted Host code; a guest or JSON request cannot provide that callback or deserialize a plan. It is not yet an end-user authorization API.

## HTTP and operation ownership

A single thread owns synchronous WinHTTP calls and all associated handles. Each exchange uses fresh no-proxy handles with global pooling disabled, zero configured failed-connection retries, no redirects, cookies, ambient authentication or keep-alive reuse. The request explicitly disables authentication in addition to the high autologon policy, because that policy alone does not protect IP/localhost targets. The broker does not select client certificates, consult credential stores or retry a client certificate challenge.

HTTPS uses system certificate validation and explicitly selected TLS 1.2/1.3 with protocol fallback disabled. There is no validation bypass or plaintext fallback. Proxy/PAC/WPAD support is not provided. HTTP error statuses remain bounded status/body responses; response headers, cookies and authentication challenges are not exported.

The callback runs before and after OS operations, before decryption and after handle closure. Per-phase OS timeouts and a cooperative wall budget are checked around calls. A blocked synchronous OS operation can finish after the cooperative budget or a cancellation signal. Its owner remains reserved until that operation returns and every handle closes. No other thread closes a synchronous WinHTTP handle; cancellation never claims an early physical release. A failed handle close retains ownership and retries closure. The future Core adapter must retain its own physical job through this whole call, including errors.

Response reads use a bounded scratch buffer and one additional byte to detect overflow. Declared oversize, streaming oversize, truncated responses, cancellation and budget expiry return no partial body. Request bytes stay borrowed through request closure. Owned temporary response/header/plaintext buffers are cleared with volatile writes on release; Windows crypto/HTTP internals and memory paging are outside this guarantee. Successful returned bodies belong to the caller and its input/result handling policy.

The selected secret is decrypted only after a live authority check, injected in the fixed header, and never supplied to a guest as a credential or header. A literal credential reflected in a response body is refused. This is not a guarantee against an authorized remote recipient transforming or encoding data it receives. Approval must therefore name the destination and secret use separately from permission to read source input.

## Verification

Twelve actual Windows loopback tests cover fixed verb/path/body, Host-only authentication, IPv4/IPv6, strict configuration, request/response/header bounds, truncated bodies, redirects, fresh-call cookie absence, refusal of automatic authentication, literal-secret echo refusal, invalid authentication values, current secret rotation/deletion, vault-location identity and policy changes. Held body reads prove both cancellation and cooperative expiry retain the operation until OS IO returns, then discard late data.

The HTTPS tests verify TLS wire bytes and reject a working self-signed endpoint with WinHTTP certificate error 12175 before HTTP application bytes are sent. The fixture first proves its TLS server with a client pinned to its exact synthetic certificate in memory. Windows SChannel requires a persisted private key here, so the fixture creates a unique test-only named CNG key and deletes/verifies deletion in `finally`. It never adds certificates to a trust store, selects an existing key or uses real user/API credentials. All network destinations in tests are local.

Primary API references: [WinHttpCloseHandle](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpclosehandle), [WinHTTP option flags](https://learn.microsoft.com/en-us/windows/win32/winhttp/option-flags), [WinHttpSetTimeouts](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpsettimeouts) and [WinHttpReadData](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpreaddata), checked on 2026-11-02.