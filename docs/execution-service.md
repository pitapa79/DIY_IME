# 共有実行サービスとControllerの寿命（P1-3j）

[Issue #152](https://github.com/pitapa79/DIY_IME2/issues/152)、親[#133](https://github.com/pitapa79/DIY_IME2/issues/133)。[実時計のPump](execution-clocked-pump.md)を一つの制御スレッドで所有し、要求のない間もworker通知・時計通知・期限によって進める。ノードの変換・選択の意味はCoreへ追加しない。

## 所有と入口

ExecutionService::new(host)は同じExecutionHostから共有する常駐サービスを取得する。with_clockは同じ経路へ注入時計を指定する。生存サービスへ別時計を渡すとclock-conflictになる。サービスが完全に休止した後も、Hostは最初の時計を保ち、再生成で時刻の起点を変えない。

open_ready(plan, version)は準備済みExecutionPlanから独立したExecutionControllerを作る。Controllerのcloneは同じSessionと開始キー発行器を共有する。start_run/get_run/get_start/cancel_run、入力失効、prepare/activate/rollback、計画version取得を固定Core要求として渡す。利用者の任意callbackを要求キュー上で実行する公開APIは作らない。next_start_keyは発行済みの最大値を進め、u64上限で失敗する。再送には発行済みキーと同じpayloadを保存して使う。Ledgerの「受理済み最大キー」と発行器は別であり、busyを受理成功と見なさない。

同じHostで4 Session、処理中を含め16未完了要求まで。境界を超える要求はbusyになり、仕事の実行枠を増やさない。HostのResourcePolicyによりworker・待機・observer・保持結果も引き続き共有する。別Hostの別domainとは独立する。Session識別子はOS乱数16 byteの小文字hexで生成する。Windowsでは[BCryptGenRandom](https://learn.microsoft.com/en-us/windows/win32/api/bcrypt/nf-bcrypt-bcryptgenrandom)のシステム既定乱数源を使い、失敗時はepoch-unavailableとして返す。時刻・固定値へ代替しない。Unixの実装は/dev/urandom、その他は未対応として失敗する。今回実機検査したのはWindows GNUである。

## 独立した進行と純粋な取得

workerの実イベントがある場合、注入時計のrevisionが変わった場合、期限が到来した場合にPumpを駆動する。単なる状態取得の通知ではControlを再観測しない。get_runは経過時間を単調時計から読み取るが、eventSeq・入力Frame・Wait予約を変更しない。取得要求より前にすでに到来した独立イベントは、通常のCoreイベントとして処理する。

native時計では最も近い期限までCondvarで待つ。巨大な待機は60秒ずつに分けるが、元の処理期限は変更しない。注入時計ではOS時間の経過で進めず、advance_toの通知を待つ。データは有界要求キューと既存の仕事所有に置き、Wakeは起床bitだけを持つ。要求が連続しても各要求の前後に独立イベントを確認する。

## closeと再接続

最後のController cloneが破棄されると、要求に残っている参照も消えた時点でSessionを非同期closeする。対象runを取消し、observerと待機仕事を解放し、Ledgerと計画を外す。保持結果はPacket/報告と保持leaseを持ち、Sessionの計画を保持しない。すでに実行中の仕事は自分の計画とleaseを実終了まで所有し、cancel応答だけで枠を返さない。Session close後の物理状態は保持報告へ直接反映する。

サービス所有ハンドルがなくても、生存Session・要求・実仕事・待機・observer・保持結果が残る間は同じ所有者を保つ。全ハンドルを破棄して直ちに同じHostへ再接続しても、生存仕事の完了受信先と使用枠を引き継ぐ。新Sessionのepochで旧結果を取得できない。保持TTLも要求なしに処理する。全資源がなくなれば制御スレッドは終了する。HostはサービスをWeakで参照し、Clockの起床先もWeakなので循環所有で常駐し続けない。

Nodeのpanic/不正出力は既存のNodeError境界で扱う。制御所有者自体の内部故障はservice-faultとして接続を閉じ、キュー内の未実行要求も返す。故障情報が確定する前の応答切断を正常closeと誤認しない。自動再起動・自動再送はしない。故障したハンドルの破棄後も旧物理仕事が残っていれば、新サービスの生成を拒否する。真の物理終了後の明示再接続だけを許す。

## 検査、限界、戻し方

実スレッドのtests/execution_service.rsでは、ポーリングなしの完了/Timer/失効/TTL、開始callback中の取消、最後のclone、再接続中の物理枠、Session上限、完全キー、取得の不変性、再送、計画切替を検査する。Core単体検査では16要求の境界、内部故障、参照循環、保持期間の所有、休止後の時計継続、キー上限、長い待機分割、OS epochを扱う。Rust全体・fmt・clippy・CLIビルドも実施する。

今回はready-plan用のCore APIであり、初期no-active-plan/最初のprepare、通信v3とエディター接続、54件の宣言的fixture全適合、TSF/MSVCは残工程。Nodeの任意CPU/一時ヒープや非協調callbackの強制停止は保証しない。Node/Registry/Graph/Packet/旧Engineとsession v2は維持する。戻す場合は新受理を止め、生存仕事の終了を確認して本PRをrevertする。
