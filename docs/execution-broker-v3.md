# 複数Sessionを共有するstdio broker

Issue #188。#176の独立CLIに続き、`diy-ime2 broker-v3 <resource-policy-file>` を追加する。broker（仲介処理）は一つの親プロセスと一組の標準入出力を使い、複数Sessionへの要求を同じHostへ振り分ける。Sessionごとの子プロセス、TCP待受、外部依存は追加しない。HTTPは[明示v3 API](execution-http-v3.md)へ接続し、画面はまだ接続していない。

## 所有の単位

一つのbrokerプロセスが一つの `ExecutionHost` / `ExecutionService` / 時計 / ResourcePolicyを所有する。`open-session` はそのHostに空のSessionを作り、`close-session` はそのSessionだけを閉じる。再度openしてもresourceDomainと使用枠は維持し、Sessionのepoch（生存期間を識別するOS乱数）だけを新しくする。新しいSessionへ計画・入力・開始キー・結果を自動復元しない。

Sessionは不変のRegistryをArcで共有する。Arcは同じ宣言を参照し続ける所有方法であり、Sessionごとの計画・準備token・入力世代は共有しない。公開Node/Registry/Graph/Packet契約と旧Engineは変更していない。

`broker::serve` は借りた共有serviceを使い、このbrokerのSessionだけを所有する。別brokerが持つSessionは触らない。4Session/16未完了サービス要求のCore境界を維持する。stdio要求は一つずつ処理し、仕事自体は既存workerへ渡す。通信から新たなworker数や処理期限を決めない。

独立 `serve_host` とCLIは、pipe EOF・読取/書込/flush故障で所有Sessionを閉じてから、真正なproducer終了と開始callback復帰までHostDrainで待つ。取消受付や完了permitだけで生存仕事の枠を返さず、非協調仕事に強制終了期限を設定しない。閉じるのは親のpipe単位なので、そのpipeを失えば複数の所有Sessionがすべて閉じる。親bridgeが一つのHTTP Sessionだけを失った場合は、pipeを切らず対応するclose-sessionを送る。[HTTP所有](execution-http-v3.md)ではCoreのowner-lockも使って、親再起動による予算回避を防ぐ。

## 包絡と操作

broker包絡の版は `broker:1`。内部のSession通信は既存どおり `protocol:3`。二つのidは別々の応答照合用で、どちらも先頭ゼロなしの正の十進文字列、最大u64までを保持する。brokerのidは重複排除キーではない。開始の重複排除には内部のstartKeyを使う。

```json
{"broker":1,"id":"1","request":{"method":"open-session"}}
```

open成功のdataは `sessionEpoch`、実際の `resourceDomain`、全13項目の `resourcePolicy`。policyはUTF-8ファイルへ全項目を明示する。不完全なpolicyと `-` はHost作成前に拒否する。試験profileを製品の既定値にしない。

openが返した実epochを以下のsessionEpochへ入れる。

```json
{"broker":1,"id":"2","request":{"method":"request","sessionEpoch":"0123456789abcdef0123456789abcdef","message":{"protocol":3,"id":"10","request":{"method":"hello"}}}}
```

request成功のdataはprotocol 3の**返信包絡全体**。外側ok=trueは返信の配送成功を表すため、内部data.okやterminal/resultを確認して処理結果を判断する。例えば旧版や不正methodの拒否は内側のprotocol 3エラーであり、ノードのErrorへ変換しない。start-runの受理応答を処理完了と見なさない。

```json
{"broker":1,"id":"3","request":{"method":"close-session","sessionEpoch":"0123456789abcdef0123456789abcdef"}}
```

close成功のdataは `{"closed":true}`。閉じた/所有していないepochは `broker-session-unknown`。別SessionのRunKeyを新Sessionへ送ると、内側が `session-conflict` として拒否する。新Sessionのget-startがnot-seenでも、旧epochで実行されなかったという証拠にはならない。

返信は `{broker:1,id,ok,data?またはerror?}`。読めない包絡だけid=null。外側の不正はbroker-envelope / broker-version / broker-request、共有Coreの容量不足はbusy。内側のv3診断はそのまま保つ。未知fieldや不正epoch/IDは受理前に拒否する。

## 有界I/Oと故障

JSON Linesの最大要求は外側を含むUTF-8で1 MiB。LF/CRLFを許し、EOF前の改行なし最終行も処理する。過大行は一度だけrequest-limitを返してpipeを閉じ、残部・後続行を新要求として実行しない。新旧streamで同じ読取関数を使う。

最大返信は**broker包絡を含めて8 MiB**。内部v3返信が単独で8 MiB内でも、外側を含めて超えればbroker-responseとして通信を閉じる。完全なフレームを作ってから改行とともに書いてflushする。途中まで成功返信を書き出さない。通信・Core所有者の故障で完全返信を作れなければNodeErrorを捏造しない。

開始、open、closeを自動再送しない。open応答を失ってepochを知らない場合、新規openで同じSessionを復元できない。brokerの停止・親の再起動を新しい私有予算での回避に使わず、元のHostの真正な終了を確認する。親bridgeの破棄・監視不能時の詳細な扱いは[HTTP仕様](execution-http-v3.md)を参照。

## 検証と限界

- [broker検査](../tests/execution_broker.rs)：共有domain/4Session、計画分離、他brokerのSession生存、旧キー拒否、明示close/openでの予算保持、実ゲートproducerとcallbackの所有、EOF/読取/書込/flush、サイズ境界/不正field/flush。
- [実CLI検査](../tests/execution_broker_cli.rs)：本物の一つの子プロセスで複数Session、実input.text、受理応答の破棄とget-start、u64最大値、再送重複排除、close/open後の同domain/新epoch/空計画/旧キー拒否、別Sessionの結果維持、13項目それぞれの欠落拒否。
- 内部encoder検査：外側を含め8 MiBちょうどを許し、1byte超過を拒否する。

これは正式fixture全54件のCLI/HTTP照合完了ではない。HTTP切断・再接続とbridge所有の実測は[HTTP検査](../editor/tests/execution-broker.test.ts)を参照。UI接続、外部ABI、TSF/MSVCは後続。旧session/v2と独立session-v3は維持する。戻す場合は新受理を止め、生存仕事の終了を確認して対象PRをrevertする。
