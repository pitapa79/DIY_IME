# 実グラフの入力解決・共有需要（P1-3c）

[Issue #138](https://github.com/pitapa79/DIY_IME2/issues/138)、親工程[#133](https://github.com/pitapa79/DIY_IME2/issues/133)。[worker/観測](execution-workers.md)と[結果投影](execution-results.md)を、実ExecutionPlanの依存関係へ接続する部品である。新しい製品ノードは作らず、旧Engine/session v2/Graph/Packet/標準Registryを変更しない。

## ホストが駆動する手順

1. RunLedgerで受理した完全なRunKeyから`graph_execution`を作る。計画・入力の所有スナップショットと要求祖先の範囲はそのrun専用であり、別runの同じ入力を自動共有しない。
2. `ready_nodes`が安定した計画順の準備可能ノードを返す。通常/遅延ノードは全入力解決後だけ、制御ノードはPendingを観測できる時点から候補になる。読取りでは開始しない。
3. 通常/遅延ノードは`start_work`で一度だけ所有Invocationを取り、ホストがWorkerDispatcherへ渡す。発行されたjobIdを`bind_job`でそのノードに束縛し、`complete_work`で完全なRunKey/jobIdに照合して結果を取り込む。ここでのActiveはホストが仕事を所有した意味で、物理スレッドの稼働数ではない。
4. 制御ノードは`start_observer`と`observe`で駆動する。GraphExecutionが接続元からPending/Resolvedを組み立てる。制御方針はControlNodeが決め、Frameの未解決状態を通常Inputsへ混ぜない。時計とeventSeqは信頼したホストが一つの時計・発行器から供給する。この部品自身は起床タイマーを作らない。
5. 入力が解決すると、必要な場合だけ次の通常仕事を準備する。全target解決後は`finish(..., Completed)`でLedgerへ確定する。previewやreadyの取得を完成と数えない。

ホストはグラフが所有するobserverをGraphExecution経由で観測し、同じobserverを別の経路でも実行しない。job/observerの登録・完了配送は同じ制御所有者が直列化する。外部クライアントにこれらのRust APIや完了注入を公開しない。

WorkerCompletionの構築フィールドも非公開にする。外部RustコードはDispatcherが発行した通知の参照・消費・再送だけを行え、IDをコピーして通知を組み立てられない。再送された真正な通知でも、消費済みjobを再解決しない。既存の部品検査は同じ期待値を読み取りAPIへ移し、構築不能のcompile-fail検査も追加する。

## 入力・失敗・復旧

接続元のDataはArcの同じPacketを共有する。Errorは実際の接続に沿ったhopを加え、Dataに付随する復旧記録も次のInvocationへ継承する。制御観測でDataを通す場合も、復旧由来を失わないよう観測の実行スナップショットへ記録を渡す。

通常/遅延ノードでは全入力が揃うまで待つ。受信設定のないErrorがあればBlocked、そうでなく必須NoOutputがあればSkippedとし、コールバックを実行しない。Error→必須NoOutputの既存優先順位を維持する。制御ノードは既存ControlObserverの同じゲートを使う。

開始後に新しく準備できた仕事の枠が確保できない場合など、ホストは`fail_work`でNodeErrorを解決できる。この操作自体は容量や待機方針を決めず、物理仕事を終了したことにしない。既に物理仕事がある仕事期限等では、ホストが別に取消を要求し、真正な終了を待つ。

## 共有需要の解放

各targetから未解決の依存を逆にたどる。解決済みノードの先は、まだ待っている別の利用先があるときだけ需要が残る。制御ノードが先に終わっても、別の制御ノードや直接の出力targetが共有元を待っていれば、その共有元は継続する。

最後の需要がなくなったPending/Activeノードをunneededにし、ActiveだったものだけReleasedNodeとしてホストへ返す。対応するjobId、または制御ノードの識別子を使い、ホストがキューの破棄・協調取消・observerの解放を行う。Resolvedの履歴は消さない。

ReleasedNodeは物理枠の返却通知ではない。遅延仕事が非協調なら、取消を要求してもworker/permitが予約を保持する。不要になったobserverはホストが所有ハンドルを破棄する。後着の値や重複jobはグラフを再解決せず、真正な物理完了の回収はDispatcherが担当する。

明示取消の`finish`も、論理終端とReleasedNodeを返す。まだ実行中の仕事を強制停止しない。グラフの破棄だけで物理仕事が止まるとは扱わない。

## 検査と未接続範囲

本書はP1-3c時点の部品の記録。初期予約とFIFOは[入場・待機工程](execution-dispatch.md)、期限/観測/失効の統合は[明示時計の制御所有者](execution-pump.md)へ進んでいる。ネイティブ時計の自動起床サービスは引き続き未接続。

`tests/execution_graph.rs`は実Registry/worker/遅延permit/制御観測を組み合わせるホスト駆動検査。入力待ち、一度だけの開始、準備順、共有Packet、Error/NoOutput優先、復旧履歴、Pending観測、最後の共有参照、直接target、不要observer、取消、誤キー/誤job/重複、同じ入力の独立runを確認する。sleepで競合順を推測せず、チャネルで実仕事の生存を確認する。

このGraph単体は自動駆動サービスではない。後続Pumpで初期予約/共有FIFO/期限順序/観測/履歴付き失効/物理カウントを接続したが、ネイティブ時計と共有サービス寿命は#133に残る。特に低水準RunLedgerの開始を、そのままv3の完成した入場判定として公開しない。54fixture全件の適合、P2通信、P3 UIも未完了。

戻し方は新経路の受理停止・生存仕事の終了確認後に本PRをrevertする。旧経路は引き続き利用できる。
