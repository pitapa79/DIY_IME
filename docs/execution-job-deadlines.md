# 仕事予算の起動時検査と期限（P1-3e）

[Issue #142](https://github.com/pitapa79/DIY_IME2/issues/142)、親工程[#133](https://github.com/pitapa79/DIY_IME2/issues/133)。[初期予約とFIFO](execution-dispatch.md)に仕事予算の保持と執行を追加する。[正式契約 v0.2](execution-control-contract-v0.2.md)の部分実装であり、自動起床するExecutionController・通信v3は引き続き未接続。

## 起点と結果

受理済みjobBudgetsをGraphへ保持し、初期仕事、後から準備された仕事、FIFO待機を経る仕事のいずれにも引き継ぐ。未指定・nullは無期限。Controlの仕事予算は既存の受理検査で拒否する。Ledgerから直接作るGraphも同じ予算を持つ。

予算は待機時間を含めず、物理枠を取得してworkerへ渡す時点から測る。起動直前にDurationの加算を検査し、加算不能ならアダプターを呼ばず、受理済みノードのinvalid-deadlineへ解決する。失敗した仕事の予約は解放する。queueWaitMsの受理前検査とは起点・失敗段階が異なる。

ホストは単調な時計を供給し、`expire_jobs(now)`を真正な完了通知の採用より先に呼ぶ。期限ちょうどでdeadline-exceededを一度返し、協調取消を要求する。通知をGraphへ適用すれば通常のNodeError伝播となり、errorInputsを宣言した後続Controlで復旧できる。期限以降に到着する旧結果は採用せず、物理終了の回収に使う。

取消・期限失効で物理枠を返さない。cancel-requested/stalledも上限へ数え、真正な終了通知だけが枠を解放する。明示取消、採用済みの論理完了、start失敗の後に同じ仕事期限を再発行しない。論理完了してもstart関数が戻らなければ物理所有は残る。

## 統合する側の責任

`next_queue_deadline`・`next_job_deadline`は起床候補の読取りであり、実行はしない。`next_deadline`はその最小値を返す。ホストは期限を時刻順に処理し、同時刻には待機失効→run期限→仕事期限→observer→取消猶予の順序を守る。run終端時には残仕事の取消とobserver破棄を接続する。現在の低水準APIをばらばらに呼ぶだけでは、この全体順序や自動起床は保証されない。

この段階は明示時計による期限部品である。後続の[制御所有者](execution-pump.md)で全期限順序・eventSeq・物理カウント・履歴付き失効を接続したが、ネイティブ時計と共有Controllerサービスの寿命は親工程に残る。輸送v3、UI、54件の宣言的fixture全適合、TSF/MSVCは未検証。製品ノード・標準Registry・Engine/session v2・既存Graph/Packet/fixtureは変更しない。

## 検査と戻し方

`tests/execution_job_deadlines.rs`は実Registryと遅延permitで、期限境界、通知未採用と採用済みの区別、FIFO待機を除いた起点、後続仕事への引継ぎ、Control復旧、初期/待機/後続の加算不能、明示取消とstalled、start失敗の後の所有を検査する。既存全体テストとfmt/clippy/CLIビルドも統合前に行う。

戻す場合は新受理を止め、生存仕事の終了を確認して本PRをrevertする。既存の同期実行経路を維持する。
