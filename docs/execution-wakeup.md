# 完了通知の起床と一通知ずつの採用（P1-3f）

[Issue #144](https://github.com/pitapa79/DIY_IME2/issues/144)、親[#133](https://github.com/pitapa79/DIY_IME2/issues/133)。[DispatchQueue](execution-dispatch.md)とWorkerDispatcherに起床ハンドルと`poll_one`を追加する。旧Engine/session v2・製品ノード・Registry・契約fixtureを変更しない。

## 起床と採用

`wake_signal()`は同じ一つのCompletionWakeを返す。通知はMutex/Condvarとpending一ビットで併合する。通知件数や結果データの別キューを増やさず、待機の直前・最中に到着した通知も保持する。`wait(None)`は無期限、`wait(Some(duration))`はネイティブ単調時計による待機。Notified/TimedOut/Closedを返し、ネイティブ時計に収まらない待機期限はinvalid-deadlineで状態変更前に拒否する。表示用ミリ秒へ丸めない。

これは単一の制御所有者が待つためのハンドルで、cloneしても通知件数を複製しない。複数の待機者へ各イベントを配信するAPIではない。ハンドルはHost/計画/Packet/物理leaseを保持せず、任意の利用者コードをworkerから呼ばない。待機中にCoreの結果採用・観測・取消・物理枠返却は行わない。dispatcher破棄でClosedへ移し、すべての待機者を起こす。旧permitが残ってもClosedから復活しない。

`poll_one()`は真正な公開イベントを一件だけ取り込む。内部のstartReturned成功等、公開結果のない通知は読み飛ばせるが、次の公開イベントを先取りしない。後続イベントは元のFIFOに残る。従来の`poll()`はpoll_oneを空まで繰り返す互換の一括操作。Controlが最初の到着から結果を決める場合、制御所有者は一件をGraphへ適用して再観測してから次を採用できる。

通知はイベント数と一致せず、内部通知だけでも起床する。所有者は起床ごとに期限を先に処理し、poll_oneをNoneまで駆動してから再び待つ。Noneを確認する途中で新通知が来てもpendingが残り、待機へ移る競合で失われない。先にpollして通知だけが残った場合、一度の余分なNotifiedは許す。結果の採用・次仕事のdispatch・再観測はホストが担当する。

## 物理所有

workerの実行用leaseは終了通知を送る前に解放し、dispatcher自身が真正な通知を採用するまでのleaseを保持する。遅延permitも、送信側が持つInvocation/leaseを解放してから通知を送る。この順序により、物理終了を採用した直後にホストがFIFOの次仕事を検討しても、送信側の一時所有で空き枠が見えない競合を避ける。start関数がまだ動作中なら、その所有とdispatcher側の所有は継続する。

起床だけで取消が完了したことにはしない。取消後の後着結果は採用せず、真正な終了だけで回収する。dispatcherを破棄しても生存するworker/permitの物理所有はHost上限へ数え続ける。

## 検査・残工程・戻し方

Core単体検査は事前通知1000件の併合、待機との競合200回、無期限待機のclose、close優先、タイムアウト後の再通知、期限加算不能を扱う。`tests/execution_wakeup.rs`は実Registry/worker/DispatchQueueで、起床だけではGraphが進まないこと、一通知ずつのFIFO採用、論理完了後のstart返却による再起床、取消後の終了回収、dispatcher破棄と旧permitの所有を検査する。

この部品だけでは自動起床するControllerは完成しない。[明示時計の制御所有者](execution-pump.md)で期限/observer/履歴/物理状態を接続したが、ネイティブ時計とController交換をまたぐサービス所有は残る。v3通信・UI・54fixture全件・TSF/MSVCは未検証。ネイティブRustコードの任意CPU/一時ヒープを強制停止する機能も追加しない。

戻す場合は新受理を止め、生存仕事終了を確認して本PRをrevertする。同期経路は維持する。
