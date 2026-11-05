# 明示時計の制御所有者（P1-3g）

[Issue #146](https://github.com/pitapa79/DIY_IME2/issues/146)、親[#133](https://github.com/pitapa79/DIY_IME2/issues/133)。[実グラフ](execution-graph.md)、[初期予約/FIFO](execution-dispatch.md)、[仕事期限](execution-job-deadlines.md)、[起床通知](execution-wakeup.md)をExecutionPumpへ接続する。製品ノードや採用方針を追加する工程ではない。

## 所有と駆動

一つのPumpが一つのExecutionHost/DispatchQueueと複数のSession Ledger、各runのGraph/observer/タイマーを所有する。同じHostのSessionは一つのFIFO・物理上限・結果保持予算を共有する。open_sessionは信頼したホストが生成したepochと検証済み計画を渡す内部Rust入口であり、公開通信や乱数生成APIではない。重複epoch・別Hostは受理しない。

startは初期枠を一括予約し、再送なら同じ受理証だけを返す。新規なら通常仕事をworkerへ渡し、ControlへStartedを配送する。その後はグラフの依存関係をたどり、準備できた仕事、受理後のresource-busy、完了による後続仕事、不要枝の取消まで同じ所有者が扱う。対象祖先だけを実行し、共有需要の残る枝は取り消さない。acceptedの受理証は結果完成を意味しない。

`advance_to(Duration)`は信頼した単調時計所有者が呼ぶ明示駆動である。期限処理の後、poll_oneで一件を採用し、Graph/Controlを駆動してから次の公開完了を読む。先に届いたDataを使う等の判断はテスト用Controlの内部にあり、Pumpへ固定しない。get_run/get_start/plan_version/next_deadlineは読取りだけで、workerをpollしたりobserveを呼んだりしない。

## Controlと時間

各observerには「前回どの入力まで解決していたか」と、Waitで指定した一つの次回時刻だけを持つ。最初はStarted、新規解決があればInput、変化がなければ期限時にTimerを一度配送する。Inputと旧タイマーが同時なら、更新されたFrameをInputで一度観測し、そのWait判断で旧予約を置換する。前回と同じFrameを理由なしに再観測しない。前のControlが同じ駆動中に入力を解決した場合、次のControlのFrameを取り直す。

期限は次の最小時刻まで進み、時計を大きく進めても途中の時刻を飛ばさない。同時刻には次の順序で処理する。

1. FIFO待機をqueue-expiredへ解決する。ここではまだControlを呼ばない。
2. run期限を履歴付きexpiredへ終端し、残仕事を取消・observerを破棄する。
3. 生きている仕事の期限をdeadline-exceededへ解決する。
4. 生きているGraph/observerを駆動し、Wait予約を置換する。
5. 取消猶予を過ぎた実仕事をstalledとして観測する。枠は返さない。

その後に未採用の完了を一件ずつ扱う。期限時刻にチャンネルへ置かれていても、まだCoreが採用していない結果を遡って成功にしない。逆に、すでに完成したrunへ同じ表示msで取消をしても結果を巻き戻さない。表示elapsedMsの切捨て値を期限比較に使わない。時計後退は全Sessionへ変更を入れる前に拒否する。

## 終端と物理状態

run取消、入力失効、新入力の成功した開始、activate/rollbackは、手元の結果collectorをLedgerへ渡し、観測済みの原因・復旧履歴を残したまま終端する。新しい開始がbusyなら、旧run/入力/observer/仕事を変えない。prepareは仕事を失効させず、rollbackも旧runを復活させない。入力を使わない固定runは入力だけの失効から独立する。

Graphが終端したらobserverとタイマーを除去し、全残仕事を協調取消する。runごとのqueued/running/cancel-requested/stalledをLedgerへ反映する。旧結果が保持期限で消えても、実仕事の終了はDispatchQueueで回収する。保持結果がなくなったことを物理終了と見なさない。Coreが採用する通知・観測にはSessionのeventSeqを発行し、読取りでは増やさない。

## 利用上の境界

Pump自身に時計スレッドはなく、waitを呼ぶだけでは進まない。new/advance_toではホストが明示時刻を供給する。後続の[実時計のPump](execution-clocked-pump.md)はwith_clock/tickを追加し、[検証と採用の分離](execution-staged-adoption.md)を使ってcallback/検証後に実時刻を再確認する。ホストによる駆動と自動サービスの所有寿命は別である。

これは通信v3の完成したExecutionControllerではない。時計再確認は後続工程で接続したが、Controller交換時も同じPumpを生存させる共有サービス、Sessionのclose/上限/安全なepoch生成、OSタイマーの分割待機、通信v3、UI、54件の宣言的fixture全適合、TSF/MSVCは残工程である。Pumpを破棄しても旧worker/permitのHost枠は残るが、新Pumpが旧仕事の個別状態を引き継ぐ機能はない。旧仕事が生存するController交換にはこの低水準の作り直しを使わない。

## 検査と戻し方

`tests/execution_pump.rs`は実Registry/worker/遅延permit/Controlを用い、対象祖先と依存駆動、一件ずつの選択、読取り/再送の不変性、タイマー置換、時計ジャンプ、同時期限の優先、期限復旧/stalled/真正な回収、保持期限超過、新入力の受理成否、履歴付き入力失効/計画切替、Session間FIFO、取消との線形化、ミリ秒未満の精度、連鎖するControlを検査する。旧部品APIとEngine/session v2の全Rust検査も維持する。fixtureの構造検査を54件全体の適合とは数えない。

戻す場合は新受理を停止し、生存仕事の終了を確認して本PRをrevertする。Graph/Packet/NodeDefinition・既存同期API・標準Registryは変更しない。
