# Host API secret management

Issue #286; prerequisite for the network/secret-use broker in the [Node split plan](https://github.com/pitapa79/DIY_IME2/issues/257). This change stores and manages secrets. It does not grant a guest permission to use a secret or make a network request.

## Fixed storage and encryption

The trusted `diy-ime2-external-host` configuration accepts optional `secretRoot`. The launcher chooses a directory separate from guest-visible Worker caches. With no configuration, `secrets` reports `available:false`, and writes/deletes are refused. Browsers and package manifests cannot choose this root. The Host creates it and the vault rejects a direct reparse root, nonregular lock/storage objects and a second live owner. Ancestor ACLs and deployment storage remain trusted: this does not protect against another process already holding the user's full authority.

Only current-user Windows DPAPI encryption is used, with `CRYPTPROTECT_UI_FORBIDDEN`, no prompt and no machine-wide flag. DPAPI failure has no plaintext fallback. The application domain, logical ID and randomly generated version are additional entropy. The decrypted binary envelope has a fixed domain marker, exact length and checksum; decoding validates all of them. It does not infer integrity from a particular DPAPI error code. Reopening verifies each captured encrypted entry and immediately discards its temporary plaintext. No plaintext hashes are exposed as metadata.

`secrets.json` has strict formatVersion=1 and bounded entries `{id,version,ciphertext}`. Limits are 64 entries, 4096 secret bytes, 16 KiB per encrypted entry and 8 MiB for the complete storage object. IDs are bounded lowercase ASCII names, versions are CNG-generated 128-bit identifiers, and duplicate/unknown fields, entries or invalid encrypted envelopes are refused. New content is synced to a temporary file and replaced atomically before the in-memory state is changed. A failed write/delete preserves the previous stored value and reports failure. The old execution Session has already been closed. Atomic replacement is not a power-loss durability guarantee.

Owned Rust secret values, decrypted envelopes, DPAPI output allocations and incoming framed bytes are cleared using volatile writes before release. The correlation header is decoded separately with ignored operation fields so it does not clone plaintext into a generic JSON Value. Secret values have no Debug/Serialize/Clone implementation. Browser/JavaScript garbage collection, OS crypto/HTTP internals, paging and crash-dump policy are outside this memory-clearing guarantee.

## Management operations and GUI

The fixed Host IPC and same-origin loopback HTTP lease permit only:

- `secrets {}`: `{available,entries:[{id,version}]}`.
- `secretPut {id,value}`: `{entry:{id,version},state}`.
- `secretDelete {id}`: `{deleted:true,state}`.

There is no read/export operation. Even encrypted bytes are not returned by the API. Root paths, arbitrary executable/policy fields and extra operation fields are refused. Metadata responses are strictly decoded; a response containing plaintext/ciphertext fields is rejected.

A mutation closes the old graph and waits for genuine Core/Worker/IO ending before updating storage. The browser and HTTP owner also withdraw old results before dispatch. Protection or lease changes fence late responses. Updating the same logical name produces a fresh version, including when its value happens to be unchanged. A new future secret-use subject can therefore distinguish rotation without a public fingerprint of the value. Management remains available under input protection; it does not turn protection off or reopen an old Session.

The execution panel offers an explicit metadata refresh, password-masked entry, save/replace and per-entry delete. It does not fetch saved values. Sending clears the value field immediately; protection, pagehide and disconnect also clear it. No graph/local-storage/session-storage field is added. Failed or uncertain requests are not automatically replayed; refresh metadata to inspect the result and enter a fresh value for another explicit write. The UI closes the graph on mutation and requires explicit preparation before executing again.

## Verification and remaining work

Actual DPAPI tests cover Unicode roundtrip, reopen, version rotation/delete, metadata/disk plaintext absence, exclusive ownership, sharing-lock save/delete failure and recovery, invalid IDs/values/schema/limits, corrupted ciphertext, renamed/version-substituted entries, invalid decrypted envelopes and duplicate entries. The 64-entry limit permits replacement while refusing another new entry.

`editor/tests/secrets-real.test.ts` uses real HTTP, the typed client, production Host and an actual infinite AppContainer Worker. A secret rotation drains that Worker and all physical jobs before replying, invalidates old results, survives reopen, remains manageable under protection and supports durable deletion. It checks that response/storage/permission data does not contain synthetic secrets or input. DOM/API tests cover masked explicit entry, immediate clearing and result withdrawal, protection/pagehide, no automatic replay and rejection of value-bearing metadata. Existing lease, same-origin and generation tests remain applicable.

[Network dispatch and secret-use consent](wasm-network-broker.md) now connect this vault to fixed Host-only authentication injection. Official bundle/cutover remain subsequent work. Unknown capabilities and missing runtime providers still fail closed. No real user credentials are needed or used by these tests. Browser visual QA remains subject to the environment limitation described in [the execution GUI verification](wasm-execution-gui.md).

Primary references checked on 2026-11-02: [CryptProtectData](https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptprotectdata) and [CryptUnprotectData](https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptunprotectdata). DPAPI normally binds decryption to the encrypting logon credentials/computer, with documented roaming-profile exceptions; it is not an independent account or administrator boundary.
