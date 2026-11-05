# 要求祖先の範囲と非同期終端結果（P1-3b）

[Issue #136](https://github.com/pitapa79/DIY_IME2/issues/136)、親工程[#133](https://github.com/pitapa79/DIY_IME2/issues/133)。[worker/観測の部品](execution-workers.md)に続き、[採用契約v0.2](execution-control-contract-v0.2.md)の専用結果を作る境界を実装する。ExecutionController全体・通信v3・UIの完成を意味しない。

`RunLedger::result_collector`は受理済みの完全なRunKeyに照合し、`ExecutionResults`を作る。targetsを起点に実ExecutionPlanの接続を逆にたどり、祖先だけを計画順で抽出する。共有祖先は一度だけ含む。`required_nodes`は対象範囲の一覧であり、自動dispatchや準備できた仕事の一覧ではない。依存順の実行・共有需要の動的参照・FIFO・枠の一括予約は#133に残る。

ホストは真正なworker/制御観測のNodeExecutionを`record`へ渡す。対象外・二重・破棄済み・終端後の記録を拒否し、出力宣言、Packet、元診断・原因・復旧記録が属するrun/計画を確認する。これは信頼したRustホストの境界であり、外部クライアントへNodeExecutionを注入するAPIは追加しない。経路の生成や入力の復旧履歴の継承はスケジューラーの役割で、収集部品は渡された元の経路と記録を保持する。

## 結果の確定

`preview`は読取り専用で、runを終端させない。`finish_collected`がLedgerへの確定点である。全targetが解決する前のcompletedはrun-pendingで拒否し、Ledgerを変更しない。deliveryはtargetだけからError→NoOutput→復旧由来Data→通常Dataの順に決める。無関係な枝の失敗から配送失敗を推測しない。

結果はreason/delivery/targets/ports/nodes/causes/recoveriesを持ち、旧succeededを追加しない。ノード診断のrunId/planVersionと原因IDは十進文字列、原因参照はsessionEpochで修飾する。変換対象を型付き診断・原因・復旧記録に限定し、Packetの本文や読み、sourceSpan、スコアを書き換えない。既存RunReportとTargetedRunReportは変更しない。

`finish_collected(..., Cancelled)`は観測済みの失敗・復旧履歴を残し、採用しなかった全ポートをdiscarded/cancelledとする。未解決ポートをNoOutputに偽装しない。完成時に残った未解決の祖先はdiscarded/unneededとする。これは全target解決によって不要になった範囲の表示であり、仕事の物理停止を証明しない。

`advance_clock_with_results`は既存の単調時計処理を維持し、期限が来たcollectorの観測済み履歴を含めてexpiredを確定する。previewで成功結果を作れていても、Coreが確定する前に期限へ達した場合は期限が勝つ。elapsedMsは論理期限の値で固定する。通常のadvance_clockは既存と同じ最小終端要約を使うため、履歴を持つホストは生存するcollectorを明示して渡す。

確定後はLedgerが実際に保持した結果をcollectorへ反映する。結果bytes超過でresult-limitの最小要約へ置換したときも、成功previewを残さない。記録済みNodeExecutionと入力Packetの参照を外し、後着結果は終端を書き換えない。PacketがRustの検証には通ってもJSONの深さ境界を越える場合は、report-failedの最小失敗要約とする。Rust内で扱えるPacketへ新しい深さ制限を追加しない。

## 検査と残作業

本書はP1-3b時点の記録。実際の依存入力・復旧履歴・共有需要をworker/観測へ接続する部分は[次のグラフ部品](execution-graph.md)へ進んでいる。自動駆動サービスと一括予約・FIFO・期限起床は引き続き未接続。

実Registryと実workerの出力を収集するテストで、対象祖先・共有元・部分結果の拒否・配送優先・復旧・取消・期限・異なるrun・結果予算・深いPacketを検査する。既存同期Engineが付けた複数のエラー経路の保持と、JS安全整数を越えるIDの変換も個別に確認する。正式fixtureの期待値は変更せず、54件の全適合とは数えない。

この部品は仕事を開始/取消せず、時計の起床も行わない。結果のexpired/cancelledと物理仕事の終了は別であり、Controllerが需要参照と真正な完了permitに基づいて処理する。入力/計画の失効時にcollectorを用いて履歴付き終端へ接続する処理、期限順序全体、サービス時計、キュー、一括予約、P2/P3は未接続である。

戻す場合は新しい経路の受理を止め、生存仕事の終了を確認してこの部品のPRをrevertする。旧Engine/session v2の経路を維持する。
