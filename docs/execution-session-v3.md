# v3 methodと実Controllerの接続

Issue #174。[包絡codec](execution-protocol.md)の次に、[SessionV3](../crates/runtime/src/execution_control/session.rs) を実装した。これはRust内で完全な要求フレームを受けて返信フレームを作るmethod所有者であり、まだCLI/HTTPの入口ではない。旧 `session` protocol 2はそのまま残す。

SessionV3は呼出側が用意したRegistryと共有ExecutionServiceを受け取り、同サービス内の空Sessionを作る。内部Controllerは公開もclone配布もしない。`reply(&mut self, frame)` の排他的な所有者だけが、このSessionのplan/input状態とprepared token/graph履歴を変更する。worker終了・期限処理は共有Actorが独立して進めるが、revision/planVersion/prepared/latestInputRevision/deliveryGenerationを変更しないため、method操作後の状態取得まで他のmethodが割り込まない。共有Hostの予算や時計をSessionごとに作り直さない。

実装したmethod:

| method | 要求の追加フィールド | data |
|---|---|---|
| hello | なし | sessionEpoch、capabilities、実HostのresourceDomain/resourcePolicy |
| definitions | なし | version 2の宣言ラッパー配列 |
| validate | graph | valid/order |
| prepare | graph | token |
| activate | token | 空オブジェクト |
| rollback | expectedPlanVersion | 空オブジェクト |
| status | なし | activeGraph/preparedGraph/preparedOrder |
| start-run | startKey/expectedPlanVersion/targets、任意input/runBudgetMs/jobBudgets | acceptedの受理証 |
| get-start | startKey | key |
| get-run / cancel-run | key（完全なRunKey） | RunSnapshot |
| invalidate-input | expectedPlanVersion/revision | 空オブジェクト |

methodは未知フィールド・不正な型を拒否する。inputの省略は入力なし、明示nullはinvalid-runtime-input。runBudgetMsの省略/nullとjobごとのbudgetMs:nullは無期限。jobBudgetsの省略は空配列で、nullは不正。予算の値・重複・対象node、入力のrevision/本文/mode、対象ポートなどは既存のCore検査を利用する。ブラウザーから時刻・完了permit・内部コールバックを渡せるmethodは作らない。

prepareの失敗・token枯渇・stale token・plan-conflictでは前の準備/計画を残す。初回activateはversion 1で復帰履歴なし。以降のactivate/rollbackでCoreが論理取消と配送世代を更新し、method所有者は成功後だけ対応するgraph/tokenを切り替える。番号をwireへ出す際は [6フィールドのWireState](execution-protocol.md) を使い、旧v2のstateと混ぜない。

start-runで現planを先に検査してはならない。既に受理したstartKeyなら、型検証と正規化を経たpayloadをLedgerの保存済み対応へ照合し、計画切替後でも元の受理証を返す。get-start/get-runは追加起動しない。response bytesを呼出側が受け取れなかったとしても、処理は自動再送しない。get-runの結果と受理証のacceptedを混同しない。

サービス故障、状態取得不能、内部の応答変換不能、フレームサイズ超過はreplyのErrで外側の所有者へ返す。ストリーム担当はその接続を終了し、Nodeの通常失敗や成功に置き換えない。methodレベルのplan/入力/容量等の拒否は、相関idと現在のstateを持つエラー返信になる。8 MiBの返信を送れないことは、すでに行われたstartの未受理を意味しない。

[method接続の実検査](../tests/execution_session_v3.rs) は、実共有Actorと実workerを使う。ゲートでstart関数を保持しながら照会/取消が返ること、受理応答を捨てた後のget-startと重複排除、計画切替後の再送、全キー照合、同Hostでの新Sessionのbusy、入力失効、真正な完了とcallback復帰の両方までの物理枠保持、保持切れと再起動拒否、既存input.textの実結果を確認する。単体検査ではtoken枯渇の原子性も確認する。

これはまだプロセス内のフレーム検査であり、実CLI/HTTPのB03/B07/B08完了とは数えない。正式fixtureの直接Core検査数も変更しない。次はCLIの有界行I/OとHostの生存期間、別Session/再接続の共有Host接続、それからHTTPとUIを進める。製品ノード追加、辞書採用、TSF/MSVC、外部ABIは含めない。

戻すには新しい受理を停止し、Sessionを閉じても残る実仕事の終了を確認してからこのPRをrevertする。旧v2入口と保存Graph/Packet、標準Registryの意味は変更していない。
