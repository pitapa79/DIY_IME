# 非同期完了・期限・協調取消・通信 v0.2

状態: **契約採用。組込みRust向けP1 Core、P2 CLI/共有Host/HTTP、P3明示UIを接続済み。受入全体は継続中**。契約採用は2026-10-29（Asia/Tokyo）、Issue #126。2026-10-30の[受け入れと引継ぎ](execution-p1-handoff.md)に、検査SHA・対応表・未完了範囲を記録する。現在の画面接続と実ブラウザー検査は[画面モデル](execution-ui-model.md)と[受入記録](execution-browser-acceptance.md)を参照。
ユーザーの「進めて」に基づき、統合担当が[接続計画](execution-control-rollout.md) P0を具体化した。[提案](execution-control-proposal-v0.1.md)のA/T/Bについて、この文書の範囲を採用する。kindの追加・ノード開発停止の解除・本番機能の実装完了を意味しない。
開始main: `0148a62a6e57661a02cd0edf16265c05b4175534`。マージ後の完全SHAをP1/P2の開始基準として記録する。
正本となる[受け入れシナリオ](../tests/contracts/execution_control.json)は実装が満たす期待値である。[照合表](execution-conformance.md)では51/54件を別APIのCoreへ直接実行し、残3件をP2へ残す。現在の同期Engineや実CLI/HTTPが全54件を通過したという意味ではない。

## 1. 版と採用範囲

| 境界 | 今回の決定 | 維持するもの |
|---|---|---|
| A 完了 | 所有スナップショット＋完了permit、通常ノードのworker適合、別の制御観測境界 | 既存Node/Inputs/Outputs/OutputValueと同期Engine API |
| T 時間 | Coreの単調時計、任意のrun/仕事期限、協調取消、終了確認までの資源保持 | 重い構成・無期限待機の選択可能性 |
| B 通信 | 別protocol **3**、start-run/get-run/cancel-run/get-start/invalidate-inputのpolling | `session` protocol 2、旧宣言・旧run応答・#45設定 |

[同期エラー契約](error-input-contract-v0.1.md)、[実装基盤](error-input-foundation.md)、Packet v1、保存Graph v1、既存demo/e2e、辞書保留は変更しない。
v0.2のJSONをv2へ送り込まない。v2が新境界の計画をunsupported-capabilityで拒否する現在の意味を維持する。新方式は将来の別CLIモード `session-v3` で明示起動し、未実装の現行CLIへ送らない。旧 `session` の既定を変えない。
push/ストリーミング、任意dataTypeの公開、外部プラグインABI、worker隔離ホスト、製品の経過時間・期限選択ノードは今回採用しない。後続の個別契約・Issueで扱う。

## 2. P1が実装する所有・呼出境界

Rust側の新規 `ExecutionController` は稼働計画の不変スナップショット、入力、資源予約、仕事・runの状態を所有する。既存Engineを丸ごと別スレッドで待つだけの実装を完了としない。

| 操作 | 契約 |
|---|---|
| start_run | plan/入力/targets/期限/容量を検証・予約してからRunKeyを発行。acceptedは完了ではない |
| get_run | 既に確定したスナップショットの読取り。時刻の前進・採用・期限延長・state.revision増加を行わない |
| cancel_run | RunKeyだけを対象に論理取消。繰返しは同じ終端状態。物理終了の保証と分離 |
| invalidate_input | 入力の配送世代を進め、旧入力の非終端runを論理取消。計画・prepared・state.revisionは変えない |
| advance_clock / submit_completion | Core/ホスト専用。制御イベントを線形化する入口。ブラウザーから時計・完了通知を受け付けない |

通常Nodeは全接続入力の解決後に一度だけ呼ぶ。Error→Blocked、必須NoOutput→Skipped、明示receiveの扱いは同期契約と同じ。重いexecuteで制御ループを塞がないため、v3では通常Nodeも有界workerへ適合できる。自動的に全ノードを並列実行する契約ではなく、依存順を維持する。
`OwnedInvocation` は設定・入力PacketのArc・RuntimeInput・run/plan/node識別を所有する。worker内で必要なRunContextを借用し、既存の借用参照をunsafeで延命しない。計画・登録ノード・準備資源は最後の所有仕事が終わるまで保持する。[準備資源提案](prepared-node-resources-proposal.md)全体の採用とは別である。
明示した遅延アダプターは `CompletionPermit` と取消tokenを受け取り、終了を一度だけ通知する。permitはCoreが特定の仕事へ発行する不透明な能力で、外部のID一致だけを信用して生成しない。開始関数自身も制御ループ上で長時間待たせない。失敗・panicは通常のノード失敗として全出力を解決する。

### targetsと実行範囲

v3のtargetsは存在する出力端点の非空・重複なし集合。**targetsの祖先だけを需要として実行する**。無関係な枝を起動しない。これは全ノードを評価する旧runおよび#46の同期run_with_targetsとは明示的に異なる新方式である。
同じ仕事を複数の需要が参照する場合は一度だけ起動し、参照が一つでも残る間は一方の選択で取り消さない。未開始の不要仕事はキューから除き、実行中の最後の参照がなくなったら協調取消を要求する。ノード内部・外部の副作用を推測して巻き戻さない。

## 3. Pendingと制御観測

通常InputsへPendingを追加しない。新しい `ControlFrame` だけが、宣言したデータ入力の `Pending | Resolved(Data/NoOutput/Error)`、起床理由、eventSeq、単調経過時間を観測する。未接続の任意入力は最初からResolved(NoOutput)。制御情報を文章Candidateへ変装しない。
制御ノードの `observe` は短い・非ブロッキングの処理を前提にする。信頼できない関数の停止を保証しない。返せる判断は次の二つ。

- `Wait { wakeAfterMs: null | PositiveMs }`: 解決を保留。nullは入力/取消等の次イベントまで待つ。正値は次回観測を登録。0や過去時刻は拒否する。同じ観測者のタイマーは置換し、毎回蓄積しない。
- `Finish(ErrorAwareExecution)`: 全出力をData/NoOutput/Errorで確定。#46のPacket/復旧/依存検査に加え、PendingをData依存や原因として申告しない。未選択の保留入力の需要を終了する。

Coreは判断の適法性・世代・配送・共有参照だけを管理する。速い候補を採る、品質を比較する、期限時に原文へ戻す等の意味は制御ノード側に置く。今回のfixtureのfirst-data等は検査用方針で、内蔵の固定採用規則ではない。
最初の需要発生、入力の新規解決、登録タイマー、取消/失効が観測契機。同じ変化なしのFrameを繰り返してbusy loopを作らない。状態取得だけではobserveを呼ばない。通常の純粋同期ノードへ時計を暗黙注入しない。
新宣言はprotocol 3専用の `executionKind: sync | deferred | control` と、通常NodeDefinition、errorInputsを含む別ラッパーで提供する。既存NodeDefinitionや保存Graphへ新フィールドを足さない。実データポートは当面text-packet@1を維持する。経過時間をポート値として出すノードの型は、別契約が必要。

## 4. 時計・期限・競合の確定規則

`PositiveMs` は1〜9,007,199,254,740,991の整数。0/負数/小数/非有限値/文字列を拒否する。nullは各節で明示した無期限または観測予約なしの場合だけ許す。省略したrun予算、ノード仕事予算はnull。値域が大きいからといってOS/Node.jsの単一タイマーへ直接渡さず、残時間を検査して分割待機する。時刻加算の表現範囲を超える予算は、仕事開始前にinvalid-deadlineとして拒否する。

| 期限 | 起点と意味 | 到達時 |
|---|---|---|
| runBudgetMs | 検証・予約が成功したaccepted時刻。以後のキュー待ちも含む | runをexpiredで論理終端、未開始中止・実行中取消要求 |
| jobBudgetsの各ノード | 仕事をrunningへ移した時刻。省略/nullは無期限 | その仕事だけdeadline-exceededのErrorへ解決。後続receiveが復旧できる |
| ホストqueueWaitMs | キュー投入時刻。null可。仕事期限とは別 | 起動せずqueue-expiredのErrorへ解決 |
| 観測wakeAfterMs | Wait判断がCoreで確定した時刻 | Timer Frameを一度配送。待機/採用の意味は制御側 |
| 接続監視 | ホストの送信/制御応答監視 | 接続故障・結果不明。runのexpiredと混同しない |

jobBudgetsの対象はsync/deferredの仕事だけ。controlのobserveには物理仕事予算を付けず、指定はinvalid-deadlineで拒否する。制御側の待機はrun予算とwakeAfterMsで扱う。run期限の加算は受理前、仕事期限の加算は実際の起動前に検査する。既に受理したrunの後続仕事で加算できなくなった場合は、起動せずinvalid-deadlineのノードErrorへ解決する。
Core内部は単調時計の精度で比較する。wireのelapsedMsは切捨て表示値で、比較に再利用しない。fixtureは整数の仮想msを使う。仕事期限は親run期限を延ばせず、親期限が先ならrunを終端する。
**期限に達するか超えたら、当該時刻の完了配送より先に期限を処理する。** 期限前にCoreが検証・記録した完了だけが期限内で、workerの自己申告時刻で遡らない。時計が飛んでも途中の期限を順に処理する。同時の内部期限はキュー失効→run期限→仕事期限→観測→取消猶予の順。runが終端した後は同時の観測で結果を採用し直さない。
期限以外はCoreが発行する厳密増加eventSeqの順に線形化する。**同じ表示msでも、既に確定した完了の後の取消はalready-terminal**。先に取消を受理したなら後の完了を採用しない。
これは元提案の「同時刻の取消を優先」を具体化する際の変更である。将来届く取消を予知したり、確定済み結果を巻き戻したりしないため、未確定のイベント間だけ線形化順を使う。fixtureは両方の順序を別ケースにする。

## 5. 論理状態・物理状態・終端報告

runはaccepted→running→terminal。terminal理由はcompleted / cancelled / expired / failed。completedは全targetが解決した意味で、値が使えるとは限らない。受理直後の取消も可能。終端は一度だけ確定し、後のイベントで再開しない。

| runの報告 | delivery |
|---|---|
| 全target解決、一つ以上Error | failed |
| Errorなし、一つ以上NoOutput | no-output |
| 全Data、復旧由来あり/なし | recovered / complete |
| 明示取消・入力/計画失効 | cancelled |
| run期限到達 | expired |
| 予約した結果予算超過・Core内の報告不能等 | failed（理由コードを保持、部分結果を最終成功にしない） |

v3報告はRunKey、targets、delivery、ノード/ポートの状態、原因、復旧記録、仕事の残存数を持つ専用形状。途中・枝取消を含む結果を従来RunReportのsucceededとして捏造しない。旧RunReport/TargetedRunReportとそのJSONはそのまま維持する。
元診断と#46の復旧記録を保持し、v3の原因参照にはsessionEpochと配送先runを付ける。#46のCauseIdだけを永続・分散IDと解釈しない。v3の外部クライアントが原因や完了を注入するAPIは設けない。
物理仕事はqueued / running / cancel-requested / stalled / finished。取消要求だけではfinishedにせず、真正な完了または「物理的に終了した」という確認でのみ枠を一度返す。単なる取消受信ACKは終了確認ではない。取消猶予到達はstalledの観測であり、枠を解放しない。
期限・取消後の真正な完了は、資源回収にだけ利用する。未知permit、別epoch/plan/run/input/job/配送世代、重複通知は採用にも他仕事の回収にも使わない。旧仕事自身の正しい旧キーは回収に有効。終了後の再通知で二度枠を返さない。

## 6. 世代と計画・入力の変更

`RunKey = {sessionEpoch, planVersion, runId, inputRevision?, deliveryGeneration}`。仕事にはjobIdを追加。sessionEpochはホストが各新セッションに生成する128bit乱数の小文字16進32桁で、再利用しない。Coreのconstructorへ信頼したホストが渡し、形式を検査する。クライアントにepochの指定権限はなく、helloで現在値を知る。fixtureの固定値は検査専用。
runId/eventSeq/jobId/deliveryGenerationはCoreが発行し、同一セッションで再利用しない。内部のrun ID採番は#46のプロセス内一意性を維持する。計画版・整数の枯渇は受理前にid-exhaustedで拒否し、0へ巻き戻さない。workerが受け取ったpermitは計画・入力の所有スナップショットへ束縛する。
入力ありrunは既存RuntimeInput（revision・本文・mode）を使用。同じrevisionの別本文/modeはinput-conflict。固定グラフはinputを省略し、RunKeyもinputRevisionを省略する。nullで省略を代用しない。
ホストは各入力revisionを単調増加で発行する。新revisionのstart-run受理時、またはinvalidate-inputの受理時に入力配送世代を進め、旧入力runを論理取消。invalidate-inputは新revisionだけを宣言し、次のstart-runでその本文/modeを束縛する。いったん束縛済みの同revisionは同スナップショットだけを許す。旧revisionの新規開始はstale-inputで拒否。
UIはローカル入力編集だけでも旧結果を即座に表示対象外にする。Coreは通知前の編集を知るとは仮定しない。新しいrunの受理が失敗した場合、暗黙に旧runを取り消さず、必要ならinvalidate-input/cancel-runを明示する。
get-run/cancel-runは保持中の過去runの完全なキーにも応答できる。キーは保存したrunへ照合し、古いという理由で現在の別runへ付け替えない。get-runのcurrentは現計画/入力世代との一致を表し、成功やUIが最後に要求したrunとの一致を意味しない。取消/期限の結果も現在の入力に対する状態として表示できるが、Dataを採用してはならない。UIはcurrentに加えて自分の最新要求のRunKey・入力本文/modeを照合する。同じ入力の再実行でも別runであり、一方の成功で他方のrunを上書きしない。固定グラフのrunは入力更新だけでは失効しない。配送世代が古い仕事でも、元のrunに束縛された真正なpermitの終了は資源回収に使える。
prepareだけでは稼働runを失効させない。activate/rollbackは安全な制御境界で旧runを論理取消し、配送世代・planVersionを進める。旧仕事の終了まで旧planを保持する。rollbackでも古い結果を再有効化しない。新run受理・結果照会はpreparedやstate.revisionを増やさない。

## 7. 有界資源と設定

新しいControllerの生成には、ホストが次の `ResourcePolicy` を**全項目明示**する。予算の所有域は `ExecutionHost` とし、同じ域のセッション/Controllerを交換しても旧仕事の使用枠を消さない。Controllerより長く生きる仕事は、その域の所有leaseを保持する。別プロセスを使うホストも、旧子プロセスの終了確認前にそのプロセス枠を返さない。
契約内の試験profileを製品既定値として転記しない。P2ホストは構成した値と資源域ID（resourceDomain）をhelloで公開し、上書き不可の隠れた数値へ変換しない。未設定/不正profileでは新方式を有効化しない。v2の起動・設定は変更しない。

| 項目 | 設定域・カウント対象 |
|---|---|
| maxActiveRuns / maxPhysicalJobs / maxObservers | 正の安全整数。未終端run / 全世代の実行中・取消待ち・stalled仕事 / 有効な観測登録 |
| maxQueuedJobs | 0以上の安全整数。未起動の仕事。0は待機キューなし |
| maxQueuedInputBytes | 正の安全整数。待機に保持する入力/設定の正規化JSON bytes。共有Arcは所有単位で一度数える |
| maxRetainedRuns / maxRetainedBytes | 正の安全整数。保持する終端スナップショットとその正規化JSON bytes |
| maxResultBytesPerRun | 正の安全整数。受理時に予約する結果予算。版3の最小終端エラー報告が収まらないprofileは拒否 |
| retentionMs / cancelGraceMs | PositiveMs。論理終端からの結果保持期間 / 取消要求からstalled観測まで |
| maxDedupeEntries / maxTombstones | 正の安全整数 / 0以上の安全整数。受理済み開始のpayload対応 / 取得不可の簡略記録 |
| queueWaitMs | nullまたはPositiveMs。無期限のキュー待ちも件数・bytes上限の内側で許す |

入場判定と予約は原子的。容量不足はbusyで、run/job/開始キーを受理せず、計画・既存run・入力世代を変えない。仕事を際限なく起動してから拒否しない。結果予算と開始payload対応の保持枠も受理時に確保する。個々のtargetsとキーを含む最小終端要約が結果予算/フレームに収まらない開始はinvalid-result-budgetとして受理前に拒否する。実行中の結果予約＋保持中の実bytesを上限へ数える。完了時に余剰予約を返し、終端保持中は実bytesを数える。
開始時に既知の初期仕事群を予約できなければ、開始全体をbusyで拒否する。受理後に新たに準備できた仕事についても、起動/キュー予約を原子的に検査する。後から枠・bytesを予約できない仕事は、起動せずresource-busyのノードErrorとして解決する。既に受理したrunを「未受理だった」と巻き戻さず、他の所有仕事を壊さない。
同時に準備できた仕事は計画の安定したトポロジカル順（独立ノードの同順位はGraphノード順）で予約し、待機キューはFIFO。完了は§4の到着・線形化順で扱う。Coreが候補の品質を推測して仕事の優先度を変えない。
結果予算を超えるPacket/診断はresult-limitの終端要約へ置換し、途中Dataを成功にしない。workerが終了した事実は回収に使う。結果を既に取得済みのrunも保持期限まで残せる。保持期限切れ、または終端記録の容量を空ける必要があれば、古い終端から削除してtombstoneへ移す。実行中runや物理仕事の所有記録を保持キャッシュとして追い出さない。
一時的なworker内割当やCPU量まで上のJSON予算で制限できるとは主張しない。任意の非協調コードを止める必要があるなら別の隔離ホスト契約が必要。上限を大きくする選択は許すが、ホストが予約を引き受けられなければ起動時/受理前に拒否する。

## 8. protocol 3の要求とpolling

1要求1応答、JSON Lines、要求1 MiB・応答1フレーム8 MiBの境界を維持。ホストの既存4セッション/16未完了要求の境界も勝手に変えず、workerやノードの個数とは区別する。包絡idと全u64識別子（planVersion/runId/jobId/eventSeq/startKey/deliveryGeneration）は**先頭ゼロなしの十進文字列**、1〜18446744073709551615。state.revisionは初期0を許す同形式。JS Numberへ丸めない。input.revision、整数msとbyte設定は既存の安全整数範囲でJSON number。Graph/Packet/version値はJSON number。
unknown field、不正型、明示nullと省略の誤用、不正ID/予算を要求の仕事開始前に拒否する。別protocolの包絡をmethodとして解釈しない。helloでprotocol:3、sessionEpoch、capabilities（execution-control-v0.2）、ResourcePolicyと現在計画状態を返す。

| method | 必須・任意入力 | 意味 |
|---|---|---|
| hello | 追加なし | 現セッションの能力・profile・状態 |
| definitions / validate / prepare / activate / rollback / status | v3用の版/識別子と従来Graph等 | v3宣言を提供。新能力のない環境はprepare拒否。切替は§6 |
| start-run | startKey、expectedPlanVersion、targets。任意input、runBudgetMs、jobBudgets | jobBudgetsは `{nodeId,budgetMs}` の重複なし配列。未知node拒否。未指定は全null |
| get-start | startKey | 応答を失った開始の照会。新しい仕事は起動しない |
| get-run | RunKey | running/terminalスナップショットと次回pollの推奨間隔を返す。推奨値は処理期限ではない |
| cancel-run | RunKey | 論理取消を応答。物理残存数を別表示 |
| invalidate-input | expectedPlanVersion、revision | 本文を推測せず旧入力配送を失効 |

```json
{"protocol":3,"id":"12","request":{"method":"start-run","startKey":"7","expectedPlanVersion":"3","targets":[{"node":"out","port":"out"}],"input":{"revision":8,"text":"kya","mode":"compose"},"runBudgetMs":null,"jobBudgets":[]}}
```

```json
{"protocol":3,"id":"12","ok":true,"data":{"state":"accepted","replayed":false,"key":{"sessionEpoch":"0123456789abcdef0123456789abcdef","planVersion":"3","runId":"41","inputRevision":8,"deliveryGeneration":"2"}},"state":{"revision":"4","activePlanVersion":"3","preparedToken":null,"canRollback":false,"latestInputRevision":8,"deliveryGeneration":"2"}}
```

例のacceptedは即時の受理情報で、get-runの結果ではない。get-runは同じkeyとstate、eventSeq、elapsedMs、残存仕事数を返し、terminalだけにresult（reason/delivery/結果要約）を付ける。この版では採用前のDataを外部へ公開しない。内部ControlFrameのPendingを、最終結果のNoOutputとして表示しない。protocol 3のstateは従来v2のstateと別の型として実装し、既存UIへ混在させない。
返信包絡は `{protocol:3,id,ok,data?,error?,state}`。ok=trueのdata、ok=falseのerror（code/message）は相互排他。包絡として読めないフレームだけid=nullを許す。stateは上の例の6フィールドを必須とし、activePlanVersion/preparedToken/latestInputRevisionはない場合nullとする。
helloのdataはsessionEpoch/capabilities/resourceDomain/resourcePolicy、statusのdataはactiveGraph/preparedGraph/preparedOrder（ないグラフはnull、順序は空配列）。resourceDomainはホストが管理する空でない不透明な文字列で、同じ予算所有域を識別する。definitionsのdataは、version:2/executionKind/definition/errorInputsを持つ宣言ラッパーの配列。validateはvalid/order、prepareはtoken、activate/rollback/invalidate-inputは空オブジェクトをdataとし、状態は包絡stateへ反映する。
get-run/cancel-runのdataはkey/state/eventSeq/elapsedMs/current/physical/pollAfterMs、terminalだけにresultを追加。physicalはrunning/cancelRequested/stalled/queuedの4つの互いに重ならないカウント（0以上の安全整数）。elapsedMsは論理終端後は固定、physicalは後着終了により減り得るがresultを変更しない。pollAfterMsは0以上の安全整数で、terminalでは0。
resultはreason/delivery/targets/ports/nodes/causes/recoveriesを必須とし、報告不能のfailedだけにerror（code/message）を追加。targetsは開始時のEndpoint配列。portsは `{endpoint,value}` の配列で、valueはstatus=data+packet / no-output / error+causes / discarded+reason（cancelled/expired/unneeded）。nodesはnodeId/status/diagnosticの配列でstatusはexecuted/skipped/failed/blocked/discarded、診断がなければnull。論理的に不要/取消となったノードと物理仕事の状態を別にし、対象範囲外の枝を報告する必要はない。
causesの各項目はid/diagnostic/pathsで、idはsessionEpoch/runId/planVersion/nodeId、diagnosticは同期Diagnosticの識別子をv3表現へ変換したもの。recoveriesはnodeId/input/output/causes（上の原因ID配列）。result-limit等の最小要約はports/nodes/causes/recoveriesを空にできるが、実在しないDataを報告しない。通常のterminalにはpendingを残さず、不要な保留ポートはdiscardedとする。
ポーリング要求を長いworkerの後ろへ直列に詰めない。Core制御ループはworkerを待たず状態・取消へ応答する。接続監視の応答期限と処理予算は別設定。有限接続監視で応答がなく切断したら結果不明、健全なget-run応答が続く長時間仕事をrun期限の代わりに切らない。

## 9. 開始キー、再取得、故障

startKeyはホストが各セッションで単調増加発行する。新しい開始は過去の受理済み最大キーより大きい値だけを受け付ける。受理した最大値をセッション終了まで保持する。無効要求/busyは受理・最大値更新をしない。受理応答の消失でもキーは受理済みである。epoch/キーの形式を検査後、保持済み/retiredキーの照合を先に行い、新しいキーだけを現在のplan/入力/容量で受理検査する。
受理済み対応が保持されている間、同じキー・同じ正規化payloadのstart-runは同じRunKeyを返し、追加起動しない。payload比較は包絡idを除いた型検証済みの全開始項目（省略の既定適用後）。JSONオブジェクトのキー順では不一致にしない。同じキーの違うpayloadはstart-conflict。要求のplan/入力世代が後で変わっても、保存済みキーの再送を新規受理として再評価しない。
start-runのdata.state=acceptedは受理証であり、現在の進行状態ではない。同じ受理証の再送ではreplayed=true、初回はfalse。同一RunKeyの現在状態はget-runから読む。get-startの成功dataはkeyだけを返す。終端済みrunの重複受理証でUIを実行中へ戻さない。
開始payload対応は実行中と終端結果保持中に保持し、終端結果を削除した時点で対応も削除する。最大受理キーは削除しない。最大受理キー以下はstart-key-retiredとして再起動を拒否する。この規則で、無制限のpayload履歴を保存せず古い開始キーの再利用を防ぐ。最大値以下だが未受理だった欠番も再利用不可で、retiredは「そのキーで必ず実行された」という証明ではない。
get-startは保持中なら同じRunKey、retiredならstart-key-retired、最大値より先ならstart-not-seenを返す。not-seenは、遅延中の元要求が将来届かない保証ではない。ホストは自動再送せず、明示的な再試行も同じセッション/同じキー/同じpayloadでのみ重複排除を利用できる。
終端結果が消えtombstoneがあるget-runはresult-gone、tombstoneも削除済みならrun-unknown。後者を「実行されなかった」と表示しない。どちらも再起動しない。別epochはsession-conflict。接続切断後に新epochで同じstartKeyを使っても以前の実行を再取得できない。外部副作用を持つ操作の再実行は利用者・ノードの責務で、Core/ブリッジが自動補償・自動リプレイしない。
同じrunIdが保持中でも、残りのRunKeyが記録と違えばrun-key-conflictで拒否する。既知の別runへ付け替えない。tombstoneは古いものからmaxTombstones以内で保持し、0なら保存しない。保持記録がなければ、それ以上の実行履歴を推測しない。
プロセスEOF/不正フレーム/監視不能は結果不明の接続故障。run内のNodeErrorとして復旧ノードへ偽装配送しない。新接続に計画や未完了runを自動復元しない。

主な拒否コードは、構文/包絡のprotocol-json、版違いのprotocol-version、要求の型/未知フィールド/不正IDのinvalid-request、予算/加算不正のinvalid-deadline、targets不正のinvalid-targets、設定不正のinvalid-resource-policy、受理前busy、受理後の仕事resource-busy。単調時計の後退はinvalid-clockとして状態を変えない。その他のコードは各節で定めたとおり。ignored-completion/stalled/run-expired等のfixture内noticeはCore観測記録であり、通信エラー応答ではない。

## 10. P1/P2/P3の検査・移行

[fixture](../tests/contracts/execution_control.json)のscenario-v2は実Graph/通信JSONとは別の検査記述。test-onlyの仕事・制御方針・仮想時計を各担当のharnessへ適合させる。fixtureを解析できたことを動作検証完了と呼ばない。
initialはテストの事前配置（稼働計画・入力世代・profile・workload）、eventsはその後の操作。run別名r1等とjob名を、そのテストでCoreが発行した実キー/permitへ束縛する。workloadsのsingleは単独仕事、raceはfirst-data制御、waitBothは通常の同期合流、fallbackは主経路Errorだけを復旧、sharedは二つのtargetの共有参照、observerは明示待機、fanoutはseedからa/bへの同期分岐合流をテストダブルで作る。
同atMsは配列順をeventSeq順に対応させる。allowClockBackwardsAttemptは意図した後退拒否テストだけ、clockDomain=checked-add-overflow-test/nativeRemainingMsは加算限界のある注入時計だけに使う。deferWorkerStartはworker開始をテスト側で止める配置で、製品設定ではない。configureはconstructor拒否の検査、repeat-startのsamePayloadは最初の型検証済み要求と同一/変更した再送を生成する指定。completeのkeyOverride/sessionEpochは、真正でない通知の拒否検査に使う。
expectedに記載した項目はすべて照合し、追加の実装情報があっても期待値を省略しない。physicalJobsはrunning/cancel-requested/stalledの合計、queuedJobsは未開始、acceptedRunsは重複受理証を除く累積受理数、startedJobs/reclaimedJobsは物理開始/終了回収回数。errors/causeCodesは観測した異なるコードの集合、その他の配列は記載順。planRevisionUnchangedはplanVersion/state.revision/preparedが不変の意味で、入力配送世代の不変を意味しない。
P1は開始/期限/取消/共有参照/真正な回収/世代/容量を実Controllerへ照合し、実workerの停止ゲートと所有寿命も確認する。P2は開始重複排除・保持切れ・再接続・旧v2・整数境界を実CLI/HTTPで確認する。P3は旧入力/計画/sessionの表示拒否、暫定表示、復旧と診断、取消と物理停止の区別を実UIで確認する。各担当は全体のRustゲート、CLI build、必要なeditor test/buildを実行する。
同時競合の初期規則、必要なprofile、polling方式は本書で確定した。Rustメソッドの細かな構文・内部キュー実装・ホストのprofile読込手段は担当が選べるが、上の意味とwire形状を変更する場合は契約変更Issueを先に作る。実験の数字や[性能目標](performance-goals.md)を機能の強制制限へ転用しない。
ロールバックはP1の新受理を止め、所有仕事の終了/隔離先の停止を確認し、利用計画を退避してから段階ごとにrevertする。v2を残す。本書/fixtureだけの段階なら2ファイルのrevertで戻せる。ノードの追加、辞書の入手、OS設定/IME登録はこの工程に含めない。
