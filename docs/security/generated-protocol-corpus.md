# 決定的な生成異常入力とtransport回復（#249）

開始main: d109f8b3eb182aa716f92fa9368597bd18bfb735。
tests/security_protocol_generated.rsは実v2 serve_session、実v3 SessionV3/streamへ直接照合する。
新しいNode ABI、fuzzer runtime、依存、正式fixture、製品のprotocol動作は追加/変更しない。

生成seedはd179202611012493（xorshift、試験用で認証/乱数品質を目的としない）。
v2が1322ケース、v3が1352ケース。内訳は正常status、全prefix切詰め、各byteの削除/12値置換、
Unicode field/UTF-8途中、深さ1/16/64/127/128/256/512、不正prepare/Graph/設定/許可自己申告、
512個の0〜256byte疑似乱数frame等。payload生成ではLFを除き、行境界は別のstream検査に分離する。
全payloadは4096byte未満で、実秘密や外部接続は使わない。

検証内容:
- 稼働計画を用意した実Sessionへ各生成ケースと正常statusを交互に送り、全応答がJSONで状態が不変。
  不正prepareが既存計画を壊さず、Probeの追加prepare/実行が発生しない。v3は共有Host usageも照合。
- 最後にv2のrun、v3のstart-run/get-runで実Probeを実行し、generated-normal-canaryの正常Dataを採用する。
  拒否だけを合格にしない。正常setupのprepare1回、最後の実行1回だけを確認。
- Unicodeを含む全不完全prefixをchunk容量1/2/7/64で受信し、次の正常行で回復する。
  空EOFは応答0、改行なしでも完全JSONなら最終要求として応答1。
- 完全応答の次のframe途中でConnectionResetを注入するとErrで止まり、部分/後続のreplyを出さない。
  その後、同じv3 Hostで別streamの正常要求が成功し、物理資源を残さない。
- 既存1MiB境界と過大lineを照合。過大suffixに正常JSONがあっても再要求として実行せず1エラーで閉じる。

## 観測した既存v2/v3差分

JSON本文を1MiBちょうどへ空白paddingした場合、LF終端はv2/v3とも受理する。
CRLFでは、v3が両終端byteを除去して受理し、v2はLFだけを除去するためCR分が上限超過となりrequest-limitを返す。
本タスクはこの既存境界を記録し、新しい外部ABIの共通規則やv2の上限緩和を採用しない。
通常サイズのCRLFと、最終改行なし完全frameの既存挙動は維持する。

## 再現と限界

```powershell
cargo test --locked --test security_protocol_generated -- --nocapture
cargo test --locked --all-targets
cargo test --locked --doc
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

通常CIで5テストを実行し、seed/corpus数はnocaptureで取得できる。
正常復旧の追加時にMockがNoOutputを返すのにcompleteを期待する試験データ誤りがあり、
Mockを意図した正常Dataに揃え、本文の一致まで確認して成功した。製品契約/正式fixtureの期待値は変更しない。

これは有限の決定corpusで、網羅的fuzz/外部Node sandbox/OS資源強制/実TSFの証明ではない。
既存execution_protocol/execution_stream/session/execution_session_v3の契約表と所有/非同期検査を維持する。
長時間coverage-guided fuzzやwriter障害などの既存資産との統合は、対象ABI/実行環境に応じて別途設計する。
戻す場合は追加testと本書だけをrevertする。
