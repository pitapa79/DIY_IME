# Preserve and validate the guest JSON wire

Issue #290. The production Worker must preserve the original guest reply until the trusted Parent has checked it. Parsing the reply into serde_json::Value and re-encoding it erases duplicate keys, so a later strict schema decoder cannot detect the original ambiguity.

With an expected module length (the production invocation), the Worker invokes the bounded guest ABI, releases the reply allocation, writes the exact captured reply bytes and flushes its pipe. It does not normalize, wrap, repair or add fields. The developer batch/probe display path is retained for its existing diagnostic use. No guest/Host schema or API version is changed.

Parent wire decoding first streams the original JSON through a recursive duplicate-key validator. Each object's decoded keys must be unique, including escape-equivalent names. Arrays recurse, while independent objects can reuse field names. Only the current objects' key sets are retained; values are not built in this pass. The existing message and serde recursion limits remain in force. Trailing data, malformed JSON, invalid UTF-8 and excess nesting are refused. Errors contain no guest keys or payload excerpts.

Only then do the existing strict envelope, Node definition/config/preparation and exact typed output decoders run. Nested definition/default-config/metadata fields cannot lose duplicate evidence before conversion to a generic Value. Existing unknown-field, Packet/root, NoOutput, provenance and authority checks remain. No effectful capability is enabled by this change.

Worker termination/Job/IO ownership is unchanged: Parent still owns the physical reservation until actual process and pipe ending. A raw guest response is never an exit receipt. Cancellation, revocation, protection, Session close and publication-generation checks continue to apply.

Unit cases cover nested, array-nested and escaped duplicate keys, envelope duplicates, primitive/independent-object success and the recursion limit. Real Wasm fixtures add ambiguous describe/default config, validation, resource preparation, output ports/envelope and Packet metadata. The v3 and dictionary production probes require these replies to fail, then exercise normal recovery and true ending.

The local regression also ran the new probe against the previous production Worker binary: the old Worker normalized an ambiguous definition and the new rejection assertion failed. The updated raw Worker/Parent pair is tested against the same fixture. This verifies the actual transport boundary rather than only calling the decoder in a unit test. SDK snapshots, processing Node meaning and formal acceptance fixtures remain unchanged.
