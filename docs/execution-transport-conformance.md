# 正式B03/B07/B08の通信照合

Issue [#192](https://github.com/pitapa79/DIY_IME2/issues/192)。正本の[正式fixture](../tests/contracts/execution_control.json)を変更せず、[検査アダプター](../editor/tests/execution-transport-fixtures.test.ts)が対象3件の初期状態・イベント順・全expectedを読む。未知のイベント、許可されないフィールド、未対応expectedは失敗させる。

## 境界と実測値

| ケース | 実行経路 | 照合する値 |
|---|---|---|
| B03 | loopback HTTP → 製品bridge → 実stdio broker/Service | 開始応答のbodyを読み取らず破棄し、get-startのキーと実producerのキーを比較。受理1・実開始1・自動再送0・物理仕事1を照合。 |
| B07 | 同じHTTP/Host内で明示Session削除 → 新規Session | 実発行の新epoch、旧キーへのsession-conflict、旧結果不明、自動再送0を照合。旧producerは取消後も解放ゲートまで生存する。 |
| B08 | 実子プロセス → 既存v2 codec/Session + 検査Registry | error-aware prepareをunsupported-capabilityで拒否。実planVersion 3を作り、エラー前後のstate全体（revision/prepared含む）不変と受理0を照合。 |

B03はクライアントの状態へ開始応答を採用しない検査であり、サーバーが送ったbytesのネットワーク消失を模擬するものではない。HTTPステータスは確認するが、body内のRunKeyは復旧に使わない。開始観測は検査producerが実OwnedInvocationから書いたキー・入力・nodeId・制御時計の時刻であり、期待値の複製ではない。get-runの物理カウンターも比較する。

B07のdisconnect/reconnectは明示的なSession削除/新規作成へ投影する。TCP切断、ブラウザー再読込、bridge/Host再起動を同一の検査として扱わない。新Sessionは同じresourceDomain/policyを持ち、active planを引き継がない。旧キーの要求は新Sessionで実行する。

B08の初期epoch/latestInputRevisionはv3側の一般的な文脈であり、v2には対応する欄がない。この2値をv2 wireへ追加せず、planVersionとplan state、能力拒否という既存v2境界を検査する。検査ノードのexecuteが呼ばれていないことも確認する。`serve_session(Session,reader,writer)`は旧serveのフレーム処理を抽出した入口であり、通常serveのRegistry・版・挙動は維持する。製品Registryへ検査ノードを登録しない。

## 時計と所有権

[検査Host](../examples/execution_transport_fixtures.rs)は実ExecutionHost/ExecutionService/Registry/workerを使い、テスト専用の制御時計とproducerを注入する。時刻はfixtureのatMsを外部ファイルで信頼された検査側から進め、実clock.nowと一致したACKを待つ。製品wireに時計操作を追加しない。実時間の5秒はハング検出用であり、run/jobの予算ではない。policy全欄・入力・予算・targetsは正式JSONから読む。

固定fixture epochはOSから実発行されたepochへの束縛表で比較する。実キーのplanVersion/inputRevision/deliveryGeneration/runIdを偽造しない。初期planVersionまで実prepare/activateを行い、v3の初期latestInputRevisionはstart前に実invalidate-inputで設定する。

検査Hostは自身の所有ファイルをcreate_newで取得し、Session終了後も物理producerが終了するまで保持する。この制御時計用の検査Hostは製品のserve_host_lockedそのものではない。製品Hostのロックと実終了までの保持、別親の再起動拒否は[HTTP所有権の既存検査](execution-http-v3.md)が担当する。片付けは自分のproducerを解放し、所有ファイルの消失を待ち、自分で作成した一時ディレクトリーだけを検証して削除する。残った他者の所有ファイルは削除しない。

## 対応表と検査手順

[対応表](../tests/execution_coverage.json)のP1分類（direct-core 43 + direct-private-core 8 = 51/54）は維持する。元のrelated-only 3件には別のtransport欄を追加し、fixture-http 2件・fixture-v2-cli 1件を記録する。専用検査はその対象集合・証拠関数の実在も確認する。Coreの51件と通信3件を足して「全54件がwire適合」とは数えない。他の51件の通信イベント全体は未照合。

```powershell
cargo build --locked --bin diy-ime2 --example execution_broker_gate --example execution_transport_fixtures
cd editor
npm test -- --maxWorkers=2
npm run build
```

全Rust検査、fmt/clippyも統合前に実施する。別CARGO_TARGET_DIRを使う場合は、ビルドしたCLIと両検査Hostを作業treeのtarget/debugへコピーしてからeditor検査を行う。`start-editor.ps1 -Check`も両Hostをビルドする。UIのv3移行、TSF、Windows 10/11双方、MSVCの検証はこの作業に含めない。
