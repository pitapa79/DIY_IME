# 実時計の起点と採用前再確認（P1-3i）

[Issue #150](https://github.com/pitapa79/DIY_IME2/issues/150)、親[#133](https://github.com/pitapa79/DIY_IME2/issues/133)。[明示時計のPump](execution-pump.md)へ、[準備と採用](execution-staged-adoption.md)を使った時計再確認を接続する。これは常駐サービスの中核であり、バックグラウンドスレッド・通信v3・UIは別工程。

## 時計と入口

ExecutionClockはArcで共有する具体型。native(offset)はInstantの単調経過時間、controlled(now)は注入時計を使う。時計読取りのために任意のNode/利用者callbackを呼ばない。controlledのadvance_toは後退を拒否し、前進したときだけ登録済みWakeへ通知する。登録はWeakであり、ClockからHost/計画/物理leaseを所有しない。nativeの読取り加算不能はinvalid-clock、callerによるnativeの前進も拒否する。

ExecutionPump::with_clockとtickはこの時計を使う。get_run/get_start等は引き続き既存スナップショットの読取りだけ。Clockが進んだ通知だけではGraphは進まず、ホストがtickを呼ぶ必要がある。旧new/advance_toの明示時計経路も維持する。clock付きPumpへ任意時刻をadvance_toで注入せず、注入時計そのものを動かす。

## 三つの起点

- run予算: 要求の検証後、全初期資源の予約を原子的に確定した時刻。
- job予算: workerへ物理所有を渡した時刻。待機からの移行ではleaseを物理枠へ置換する瞬間。
- ControlのWait: callbackと検証を済ませ、判断を正式に採用した時刻。

run受理には内部AdmissionClockを使う。予約の使用枠をロックした後、実時刻・既存の最小期限・予算加算を、counter/lease/cache変更前に検査する。既存期限が来ていればclock-retryで全無変更とし、Pumpが期限を処理してから同じ開始を再試行する。結果保持の追出しも予約成功前には行わない。検証中に過ぎた旧runを、新入力の開始でcancelledへ上書きしない。未公開IDに欠番はあり得る。

待機→実行の置換でも、待機期限/run期限を同じ予約境界で確認する。期限到来なら待機の所有を変えずに時計処理へ戻る。初期/後続の直接handoffは実時刻で仕事予算を加算し、すでに自身のrun期限に達していればadapterを呼ばない。その内部通知を採用する前にPumpがrun失効を処理する。仕事開始時刻へQueueの処理済み時計を飛ばし、途中の期限を失うこともしない。

## 検証後の再確認と観測順

worker通知を一件準備し、Graphの出力検証まで終えてから時計を読み直す。期限・取消・失効を先に処理し、同じQueueへcommit_eventする。まだ論理結果が残る場合だけ準備済みGraph結果を記録する。物理終了の確認と論理結果の採用は別に扱う。

Controlもprepareの後に時計を再確認する。準備中のobserverハンドルはmapから一時的に外し、同じobserverを二重に呼ばない。期限が来たら判断を一旦保持し、到来したTimer/Inputとその下流の観測を先に駆動する。準備済みのFrameが入力解決で古くなった場合は、ハンドルを戻して新しいInput Frameを配送する。これにより、先に確定した結果を下流が観測する前に、別の準備済み結果を採用してしまうことを防ぐ。

判断は明示的な待機列で処理し、時計再確認のたびにpolicy callbackを再帰的に積まない。Nodeごとの意味・選択規則はCoreへ固定しない。初期Startedや変化なしのFrameを無限に繰り返さず、Wait予約も一つだけ置換する。runが終端/不要になったら保留中ハンドルも解放する。最後のtargetの解決を記録したら、次の時計処理へ移る前に論理終端を確定する。

期限は時刻順に検出し、同時刻はqueue→run→job→observer→graceの順序を保つ。実時計では、別のcallback中に5msのTimerが来ても、実際にそのpolicyを呼べるのが10msならFrameのelapsedは10msであり、5msに実行したように戻さない。検証・採用中に来たrun/job期限を先に反映し、遅いDataで成功へ戻さない。旧明示時計の仮想ジャンプは、ホストが期限時刻を一段ずつ供給する既存の意味を維持する。

## 保持と次工程

結果の保持TTLもnext_deadlineへ含める。実仕事がなくても保持期限で起きられる。SessionのLedgerがなくてもHost側の保持時計を前進でき、同じHostの保持時計を巻き戻さない。状態取得をTTL処理や時計駆動の代用にしない。

このPumpだけでは自動サービスではない。後続の[共有実行サービス](execution-service.md)が、同じPump/Host/ClockをController交換後も保持する共有所有者、worker/Clock/タイマーによる独立起床、Session close・4 Session/16未完了要求・安全なepoch生成を接続する。Nodeの任意CPU/一時ヒープ、非協調callbackの強制停止は保証しない。54件の宣言的fixture全適合、通信v3、UI、TSF/MSVCは未完了。製品ノード・標準Registry・旧Engine/session v2・Graph/Packet/fixtureは変更しない。

## 検査と戻し方

`tests/execution_clocked.rs`は同じclock付き経路へ注入時計を使い、callback中の時計前進、run失効、仕事失効後の復旧、Waitの採用起点、Timerの先行と下流選択、同じ走査の後方にある期限、128 observerの非再帰処理、純粋な状態読取り、複数Session、保持TTLを確認する。Core単体検査は予約preflight失敗時のlease/counter/cache不変、実予約時刻、加算不能、handoff起点、失効後の未起動を扱う。全Rust検査とfmt/clippy/CLIも行う。

戻す場合は新受理を止め、生存仕事の終了を確認して本PRをrevertする。既存同期経路を維持する。
