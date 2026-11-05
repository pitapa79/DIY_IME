# protocol 3の包絡と状態

Issue #172。[正式契約v0.2](execution-control-contract-v0.2.md) §8に対応する低水準codecを [protocol.rs](../crates/runtime/src/execution_control/protocol.rs) に実装した。公開CLIの入口とエディターは引き続きprotocol 2を使う。protocol 3のmethod実行・ストリーム所有・共有Host接続・HTTP/UIは後続工程であり、本codec単体でP2の通信が完成したとは扱わない。

`decode_envelope(frame)` は行区切りを取り除いた完全なUTF-8 JSONフレームを受け取り、明示protocol 3と正規形の十進文字列idを検査する。版を先に判定し、v2のmethodや数値idをv3として解釈しない。要求が1 MiBを超えれば解析前にrequest-limitで拒否する。JSON構文・包絡フィールド・idが読めなければ応答idはなし。読めたidでの版不一致はidを保持する。

返す `request: Value` は未検証のままである。method担当は未知method/field・不正型・null/省略・入力/予算などを別途拒否してからControllerを呼ぶ必要がある。包絡が読めたmethod不正は、その要求idでエラーを返す。decoderはRegistryを持たず、プラン準備や仕事開始の副作用を起こさない。

`WireState::from_control` は [Coreの一貫した状態](execution-session-state.md) とmethod所有者が実際に保持するprepared tokenから返信stateを作る。revisionは0を許す十進文字列、plan versionとdelivery generationは既存DecimalId、入力revisionは既存の安全整数number。6フィールドを常に出し、省略可能な状態値がなければnullを出す。prepared有無とtoken有無が食い違う場合は内部状態エラーにする。tokenをrevisionから推測して生成しない。

`encode_reply(id, outcome, state)` は成功dataと失敗errorを相互排他で出す。idなしの成功は拒否し、idなしの失敗は読めない包絡への返答として呼出側が使う。8 MiBを超える書込みを検出した時点でresponse-limitを返し、部分フレームを返さない。上限はJSONエスケープ後のUTF-8バイト数であり、文字数や入力Valueの文字列長では判定しない。元Valueのサイズや任意再帰値のCPU/ヒープ上限を保証するものではない。

フレームに改行は含めない。ストリーム担当はread自体を有界にし、過大行の残りを次の要求として実行せず、encoder成功時だけフレームと改行を書いてflushする必要がある。応答不能時の接続終了と、閉じたSessionの実仕事終了までのHost所有は後続工程で扱う。methodと返信stateの原子的な組合せも、その所有者の責務になる。

[codec検査](../tests/execution_protocol.rs) はid境界、異常包絡、版の分離、1 MiB/8 MiBの境界、Unicodeとエスケープ拡大、6フィールドとnull、data/error、正式文書内の要求/応答例を実codecへ照合する。旧v2のserveもそのまま呼び、v3要求の拒否とv2 statusの継続を確認する。これは実CLI/HTTPのB03/B07/B08受け入れ完了を意味せず、正式fixtureの直接検査数は変更しない。

旧入口・ノード・辞書・依存を変更していない。戻す場合はこのPRをrevertする。外部へcodecを接続した後の変更では、その接続と生存仕事の所有を確認した上で戻す必要がある。
