# v3画面接続（P3-1〜3）

Issue #194で型/API/表示ガード、#196でSession所有hook、#198 / [PR #199](https://github.com/pitapa79/DIY_IME2/pull/199)で明示実行パネルを接続した。既定v2と保存Graph/Packet/入力バッファ形式を維持し、明示選択したときだけ共有Hostのv3 Sessionを開く。Core/server/Node/Registry/辞書/正式fixture/依存・lockはこの画面接続では変更しない。

## 分離した境界

- [executionModel.ts](../editor/src/executionModel.ts)：v3のepoch、完全RunKey、6項目WireState、受理証、snapshot。u64は最大値まで正規十進文字列とし、比較時はBigIntを使う。Numberへ変換しない。入力revision・ms・物理数は安全整数として検査する。受理証は成功結果ではない。
- [executionApi.ts](../editor/src/executionApi.ts)：明示v3 HTTPのSession開閉/status、prepare/activate/rollback、invalidate-input、start/get-start/get-run/cancel-run。応答形と要求対象のepoch/plan/完全キーを検査する。definitions/validateも同じv3経路へ接続する。
- [executionView.ts](../editor/src/executionView.ts)：最新ローカル要求token、入力snapshot、epoch/plan/完全キー/配送世代を照合する純粋な状態関数。通信、poll timer、Nodeの採用方針を持たない。

terminal resultのports/value、nodes、causes、recoveriesはunknownの境界で保持する。旧v2 Report/Diagnosticへ型castせず、意味の解釈と構造検査はv3専用executionPayload/ExecutionResultsへ分離する。復旧のdeliveryと原因/復旧記録はそのまま残し、失敗を消して成功へ変換しない。ここでData表示可能と判断されても、payloadを検証せず画面に渡せるという意味ではない。

## 画面所有者の呼出順

1. 型付きAPIで得た最新stateとSession epoch、現在のローカル入力からExecutionContextを作る。固定入力グラフはinput:nullとし、文章入力を使うrunだけRuntimeInputを渡す。
2. executionIntentで入力本文/mode/revisionの不変snapshotを捕捉し、beginExecutionで最新要求にする。同じ本文の再実行にも別のobject tokenを使う。開始キーは別途呼出所有者が新規発行し、意図的な照会以外で再利用しない。
3. startの応答を受けたらupdateExecutionContextでそのWireStateを反映してからacceptExecutionへ渡す。初回start自体が入力配送世代を進めるため、送信前の世代との単純一致を要求しない。受理時に完全キーを束縛し、その後は完全キーと現在の入力配送世代を比較する。
4. get-run/cancel-runの応答も最新stateを反映してobserveExecutionへ渡す。eventSeqの逆行、phaseの後退、別キー/旧tokenを拒否する。terminalの結果とelapsedMsは保存し、真正な物理カウンター・current・eventSeqの更新だけを反映する。
5. 入力のローカル編集は、Core通知を待たずにupdateExecutionContextへ渡す。本文/mode/revisionのどれかが違えば旧表示を消す。prepare-onlyは稼働runを保ち、activate/rollback/新epochは旧表示を消す。固定runは入力更新による配送世代変更だけでは失効しない。

executionDataVisibleはcurrent・最新token・完全キー・ローカル入力を照合し、completed + complete/recoveredだけを許可する。cancelled/expired/failedや古いrunのDataは表示しない。一度current:falseを観測した同じrunを遅れたtrueで復活させない。

executionPhysicalStoppedは、実snapshotに未終了/待機仕事があればfalse、terminalで全仕事数0ならtrue、接続故障/保持切れ/非終端の一時的な0ならundefinedを返す。取消要求や受理だけでは停止済みと扱わない。UIの壁時計からCoreのelapsedMsや期限を合成しない。

## 故障と明示照会

ExecutionApiErrorのstateは実Core拒否に付いた検証済みWireStateだけ。事前abortは送信せずresultUnknown:false。送信後のネットワーク/abort、不正JSON/応答、bridge故障はresultUnknown:trueで、偽stateやNodeErrorを作らない。busy等のCore拒否は通信故障と区別する。危険な数値予算をJSONのnull（無期限）へ暗黙変換する前に拒否する。

APIは開始/計画/Sessionを一切自動再送しない。結果不明ではloseExecutionでDataを消し、旧キーとtokenを残す。同一epochが実際に生きていれば、明示get-startで最新stateとキーを取得し、recoverExecutionへ渡してからget-runを再照会できる。recoverExecutionは仕事を開始せず、Dataもまだ表示しない。保持切れのforgottenや別epochからは復旧させない。HTTP送信後の切断ではbridgeがSessionを閉じるため、この同一epoch照会が常に可能とは保証しない。新epochでの未発見を旧仕事の未実行証明にしない。

## 検査・限界・戻し方

[状態検査](../editor/tests/execution-view.test.ts)は整数境界、ローカル編集、初回入力世代更新、固定run、prepare/activate/rollback/epoch、全キーの各欄、終端/取消/期限/物理更新、復旧記録、保持切れ/故障/明示照会を確認する。[API検査](../editor/tests/execution-api.test.ts)は無再送、事前abortと送信後故障、異常応答/旧v2 state、予算の暗黙無期限化拒否を検査する。実loopback HTTP→製品bridge→実Coreでは入力の結果、get-start、計画切替/rollback、再接続と旧キー拒否を通す。実開始応答bodyを読み捨てて不正JSONとして渡す試行でも開始は1回とし、get-startで復旧する。

P3-3では以下の実画面と検査を追加した。正式54件のwire照合、P3全体、TSF、Windows 10、MSVCの完了とは扱わない。戻す場合は新受理を止め、自Session closeと真正な仕事終了を確認して担当PRをrevertする。

## Sessionの所有とページ寿命（P3-2）

Issue #196の[useExecutionSession](../editor/src/useExecutionSession.ts)は既定disabledで、明示enabledの間だけSessionを開く。接続ごとのownerを使い、再接続・遅延open・StrictModeのcleanup後に返った旧Sessionを自分だけでcloseする。old ownerの応答を新ownerへ渡さず、同じownerのWireStateもrevision/deliveryGenerationのBigInt比較で逆行させない。getSessionで即時の最新stateを取得できる。

callは変更系（prepare/activate/rollback/start/invalidate）1件、照会系（status/get-start/get-run）1件、取消1件の3laneに分ける。同じlaneの二重要求は送信前busyとし、poll中でも取消や計画操作が単一busyの後ろで詰まらない。開始処理が受理済みでも、そのHTTP応答待ちをUIが全操作の停止と扱わない。これは通常Coreが即答するmethod要求の制限で、runが終端するまで全UIをbusyにするものではない。

既知Core拒否は実stateを反映して接続を保持する。通信故障では自分のSessionを失効しcloseを1回だけ送り、Hostや他Sessionをkillしない。失敗した開始/計画/接続を再送・自動復元しない。明示connectは新しい空Sessionを取得するだけで、計画や入力を準備/実行しない。close完了や故障を物理仕事の停止に読み替えない。

pagehide/unmount/disabledは自分のownerを廃棄する。pageshow.persistedは旧接続を復活させず、明示再接続を求める。通常入力編集でHTTPをabortする処理は追加しない。30秒の読取専用status監視は変更/照会が進行中ならスキップし、故障後は停止する。runの処理期限を新設しない。

[React/jsdom検査](../editor/tests/execution-session.test.tsx)は実hookをマウントして寿命を確認する。輸送は型付きAPIを注入したmockで、実ブラウザー・実Hostのページ破棄確認とは区別する。型付きAPI/実HTTPの別検査を、このhookの実UI完成の証拠へ読み替えない。P3-3のExecutionPanelからAppへ接続した。注入clientはhookの寿命中安定した参照を使う。


## 明示パネルと表示境界（P3-3）

[ExecutionPanel](../editor/src/ExecutionPanel.tsx)は準備・切替・復帰、targets、任意run期限、開始/照会/取消、保持切れ・故障・明示再接続を担当する。旧v2の実行操作/自動実行/結果と混在させない。v3宣言を受け取るまでキャンバスを再構築せず、未知のポートで接続を描かない。準備だけでは表示中の稼働runを失効させず、切替・復帰・別epochは旧表示を消す。復帰後はさらに以前の計画を復活させない。

実行追跡と表示状態の両方がadvanceExecutionSnapshotを使い、遅れたpollで取消後のterminalをrunningへ戻さない。終端の結果・Core経過時間を保存し、真正な物理数・currentを更新する。物理数が0になるまで照会するが、run/jobへ既定期限を付けない。古いepochの準備・照会・入力失効完了は新Sessionへ反映しない。

[executionPayload](../editor/src/executionPayload.ts)は版2宣言ラッパーとPacket/結果全体を検査する。重複kind/version、出力先重複、異なるrunのcause、壊れた候補木・Unicodeを拒否する。結果は部分的に採用せず、原因・復旧参照も検証する。[ExecutionResults](../editor/src/ExecutionResults.tsx)は完成したDataだけを表示し、空文字列・NoOutputを別に扱う。診断と復旧の情報を旧v2 Reportへ偽変換しない。

同じ200msの最新1件自動実行hookを利用し、計画/epoch/入力を照合する。入力編集の即時非表示とCoreへのinvalidateを分離し、失敗した開始/計画/接続を自動再送しない。固定runは実行時入力編集から独立する。

[パネル検査](../editor/tests/execution-panel.test.tsx)13件、[payload検査](../editor/tests/execution-payload.test.ts)3件、[モード検査](../editor/tests/execution-mode.test.tsx)3件を追加した。React/jsdomでは遅延応答、取消後の物理終了、明示期限/stalled、復旧と元診断、固定run、準備/切替/復帰、最新1件・busy無再送、未設定時の無フォールバックを確認する。実HTTP/API検査のdefinitions/validateは実Coreを使い、UI輸送mockとは区別する。

Windows 11の実ブラウザーでは明示policyの共有Hostで7ノードの出力、実用ローマ字変換、編集直後の旧Data非表示、自動実行、入力確定、ケーブル/空文字列、再接続と同期復帰を確認した。修正後の再検査は警告・エラー0件。ページ破棄の全ブラウザー条件、取消/stalled/復旧の実ブラウザーでの全組合せ、独立レビュー/CIは未検証。


[Issue #200の実ブラウザー検査](execution-browser-acceptance.md)で、取消/stalled、producer終了とcallback復帰の分離、通常pagehideのSession closeと共有Host保持を確認した。実BFCache、期限/復旧の実画面と全寿命条件は未完。検査Registryへの切替時の未知handle警告も記録している。


Issue #202ではNodeDisplayStatusをv2 ReportのNodeResultから分離し、v3のdiscardedを除外せず「採用対象外」としてキャンバスへ渡す。未実行・失敗・物理終了へ読み替えない。実Node描画と取消snapshotの投影、グラフ編集後のstatus消去を検査し、実Host/ブラウザーでもstalled1から終了0になっても採用対象外が保たれることを確認した。
