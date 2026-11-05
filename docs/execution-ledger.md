# 実行受理・run終端・保持台帳（P1-2）

Issue #131、上位 #128。[契約v0.2](execution-control-contract-v0.2.md)に沿うCore部品で、完成した非同期Controllerや通信v3ではない。

本書はP1-2時点の記録。所有入力・実worker・制御観測の接続境界は[次の部品実装](execution-workers.md)を参照する。グラフ全体のスケジューラーは引き続き別の接続工程である。

`RunLedger` は実ExecutionPlanを所有し、targets・入力・予算を検証する。受理後だけStartReceiptを公開する。既存Engineと同じrun ID採番を使い、プロセス内で衝突させない。内部番号には欠番があり得るが、busy等で未受理の番号をRunKeyとして返さず、開始キー・入力世代・計画は変えない。

- 同じ開始キーと同じ型検証済みpayloadは同じ受理証、異なるpayloadはstart-conflict。削除済みキーはwatermarkでretiredにし、再実行しない。
- get-run/get-startは読取りだけ。時刻進行・observe・計画変更を起こさない。Core/ホストがadvance_clockで時刻を進める。期限ちょうどの後着完了を採用せず、終端後の結果は不変。
- invalidate-inputは入力世代だけを進め、固定入力のrunを取り消さない。prepareは稼働runを維持し、activate/rollbackは旧runを論理取消。rollbackでも版を増やす。
- 同じ入力の複数runは別のキーと結果を持つ。currentは現在の計画/入力との一致であり、UIの最新要求や成功の代わりではない。
- plan_snapshotが返すArcは取消・計画更新を越えて所有できる。後続workerはこの所有参照を保持し、借用RunContextを延命しない。

終端結果はExecutionHostの共有保持ストアへ移す。同じ資源域の別台帳の結果も含め、古い終端から削除できる。枠の交換と新予約は原子的で、失敗した開始が古い結果だけを消すことはない。保持payloadはkey/elapsedMs/resultの正規化JSON bytesで計数する。読取り時に合成するcurrent/physical等の可変情報と、通信フレーム全体の上限は別境界。
保持ストアのArcは資源計数本体のArcと分けている。保持レコードのleaseがストア自身を所有する循環を作らず、最後の所有者が消えれば資源域も解放できる。保持期限切れはresult-gone、tombstoneも残さない場合はrun-unknown。どちらも「未実行」と解釈せず、get-startのwatermarkも消さない。

最小終端要約が予約予算へ収まらない開始は拒否する。受け取った結果が予算を超えた場合はresult-limitのfailed要約を保持し、部分Dataを成功扱いしない。完了報告は既に検証した実行結果を渡すホスト専用境界で、通信クライアントがfinishを呼ぶAPIは設けない。台帳でもtargets、Packet/root、deliveryとの整合を検査するが、原因・復旧の意味は後続の実行結果生成側が#46の検査を通して与える。

[台帳テスト](../tests/execution_ledger.rs)は期限前/同時/後、取消/完了順、開始再送/保持切れ、容量拒否の不変性、異なるControllerの共有保持、入力と計画、Arcの寿命を検査する。[資源テスト](../tests/execution_resources.rs)は別域・共有leaseの交換拒否と原子性も検査する。
正式fixtureの一部から期限等の期待値を参照しているが、54シナリオ全体を実グラフの実行へ照合したとは扱わない。テストのfinishは検証用のホスト結果であり、実ノードをworkerで実行した結果ではない。

未実装: worker/完了permit・個別仕事期限・FIFO/共有需要・ControlFrameと観測タイマーの実駆動、CLI/HTTP v3、UI、製品ノード。physicalカウントの更新も後続Dispatcherが実所有leaseに対応させて呼ぶ。台帳単体は自動でスレッドや時計を駆動しない。複数の台帳を使うホストは同じ単調時刻を各台帳へ配送する。
通信v3の最終フレーム上限・エラー形式の結合検査はP2で行う。既存Engine/v2の実行意味・正式fixture・Node APIは変更しない。戻す場合は後続から逆順に依存を外し、本段階をrevertする。
