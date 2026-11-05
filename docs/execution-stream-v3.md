# v3の行I/Oと独立CLI Host

Issue #176。[method所有者](execution-session-v3.md)を [stream.rs](../crates/runtime/src/execution_control/stream.rs) でJSON Linesへ接続した。既存の `diy-ime2 session` はprotocol 2のまま。明示的な `diy-ime2 session-v3 <resource-policy-file>` だけが別のv3 Hostを起動する。エディター・HTTPはまだ接続していない。

## 起動と資源方針

```text
diy-ime2 session-v3 C:\path\host-policy.json
```

policy fileはUTF-8 JSONで、[正式契約](execution-control-contract-v0.2.md) §7の13項目すべてを指定する。標準入力は要求ストリームなので、policyの `-` 指定は受け付けない。不完全・不正なpolicyはHost作成前に拒否し、標準出力へプロトコル外の診断を混ぜない。診断は標準エラーへ出す。試験fixtureのprofileを製品の既定値にはしない。

HostのresourceDomainはOS乱数で作る不透明な値で、helloが実policyとともに返す。別の明示Hostは別domainであり、同じSessionの再接続先として自動的に差し替えない。このstandalone CLIをエディターのSessionごとにspawnする実装は行わない。複数Sessionと再接続が共有する永続Host/brokerは次工程である。

## 行と失敗の境界

`stream::serve(service, registry, reader, writer)` は、既存共有サービス内で一つのSessionを作る。1フレームの読取りは1 MiBに改行2 byte分を加えた量までで、LF/CRLFを取り除いてcodecへ渡す。EOF前の改行なしの最終フレームも処理する。JSON不正やmethod不正は1エラー返信になり、次の正常な行を処理できる。

過大な行はrequest-limitを1回返して、そのSessionを閉じる。残りのbyteや後続行を新しい要求として実行しない。返信は [codec](execution-protocol.md) で8 MiB以内の完全フレームを作ってから、1改行とともに書いてflushする。書込/flush失敗は結果不明であり、同じ要求を自動再送しない。

EOF・読取/書込失敗・内部故障はそのSessionを閉じる。`serve` は借りた共有Host自体を停止せず、ほかのSessionや生存仕事の資源所有を壊さない。外側のプロセス所有者は、Hostとサービスを実仕事終了まで保持する責任を負う。

## standalone Hostの終了

`stream::serve_host(policy, registry, reader, writer)` はCLIのプロセス所有者用である。最後にHostDrainがactive run・待機仕事・observer・物理仕事の解放を待つ。正常EOFだけでなくI/Oエラーとunwindでも、Sessionの破棄後に待つ。完了permitの受付や取消応答だけでは待機を終了せず、実producerの終了と開始callbackの復帰をCoreが確認するまで保持する。

HostDrainは制御所有者の外から資源カウンターを読むだけで、Nodeの再観測や結果採用を行わない。10 msごとに確認し、強制終了期限を勝手に設定しない。協調しない仕事が終わらなければ、CLIも待機を継続する。任意コードを強制停止できる保証やworker隔離を追加するものではない。独立CLIの入力が閉じられ、全実仕事が終了した後に、利用不能になったそのプロセスの保持結果をTTLまで維持するためだけの待機はしない。

## 検査と残工程

[stream検査](../tests/execution_stream.rs) は実workerをゲートで保持し、EOF/読取失敗後もrootが終了しないこと、permit完了後もcallbackが戻るまで待つこと、書込失敗後の新Sessionが同Hostでbusyになることを確認する。独立Hostについては書込・flush失敗後も、permit完了だけでは終了せず開始callbackの復帰を待って元のI/Oエラーを返すことを検査する。1 MiBちょうどのCRLF、過大行の後続要求拒否、改行なし最終要求、各返信flushも検査する。

[実CLI検査](../tests/execution_cli_v3.rs) は本物の `diy-ime2` を起動し、hello、prepare/activate、既存input.text、開始応答の破棄とget-start、u64最大startKey、get-run、重複再送、status、終端結果へのcancel、異常形式、policy必須、旧v2の版拒否を照合する。旧 `session` の実プロセス検査も残る。

B03の照会・重複排除には実CLIの証拠が増えたが、正式fixture全54件の実CLI/HTTP照合は未完了である。B07の実再接続共有Host、B08のv2新能力拒否を含む全ケースと、HTTP/UIの接続、TSF/MSVCを完了扱いにしない。次は同じ資源domainを保つ共有Host接続と、切断・再接続時の実プロセス検査を進める。

戻すには新しいv3 Hostへの受理を止め、生存仕事の終了を確認して対応PRをrevertする。旧v2・保存Graph/Packet・ノード・辞書・依存は変更していない。
