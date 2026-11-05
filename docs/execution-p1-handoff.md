# P1 Coreの受け入れとP2への引継ぎ

2026-10-30（Asia/Tokyo）、[Issue #168](https://github.com/pitapa79/DIY_IME2/issues/168)。対象は親[#128](https://github.com/pitapa79/DIY_IME2/issues/128)と[#133](https://github.com/pitapa79/DIY_IME2/issues/133)のRust Core。CLI/HTTPのprotocol 3、エディターの非同期表示、Windows IMEへの接続は後続工程である。

Coreの検査済みfeatureは`d164d293d41070263062d690525a9cbd95f3a0eb`（[PR #167](https://github.com/pitapa79/DIY_IME2/pull/167)）、統合mainは`fe8e40d90965bb095cccf60f1480a1e513248ec8`。この引継ぎはそのmainを基準にする。P1の完了は以下のCore範囲を指し、非同期IME製品全体の完成を意味しない。

## 現在使える入口

Rust組込みでは、Registryへ通常Node・ErrorAwareNode・DeferredNode・ControlNodeを登録し、GraphをExecutionPlanへコンパイルして使う。ExecutionHostへ全13項目のResourcePolicyを明示し、ExecutionServiceを取得して、open_ready(plan, version)からExecutionControllerを作る。開始・状態取得・取消・入力失効・計画のprepare/activate/rollbackを扱える。

これは稼働計画のあるCore入口である。初期no-active-plan、最初のprepare/activate、通信のprepared token、v3 state包絡はP2で接続する。標準Registryへ新しい製品ノードを登録したわけではなく、テストのfirst-data/recovery等を製品の固定選択規則として提供しない。

低水準のExecutionPumpは明示時計または共有ExecutionClockで駆動できる。共有自動サービスは一つの制御所有者がPumpを持ち、worker通知・期限・時計通知で進める。状態取得を駆動の代用にしない。旧Engine、既存CLI、session protocol 2、現在のエディターは従来の同期経路を使う。

## 受け入れ条件と証拠

| Coreの保証 | 実装・検査の入口 |
|---|---|
| 所有Invocation、借用参照を延命しないworker適合、通常Node/遅延adapter/Controlの別境界 | [worker](../crates/runtime/src/execution_control/worker.rs)、[実worker検査](../tests/execution_workers.rs) |
| 全入力待機、Error優先、必須NoOutput、明示receive、原因・復旧・依存の妥当性 | [Graph検査](../tests/execution_graph.rs)、[結果検査](../tests/execution_results.rs)、[同期エラー境界](error-input-contract-v0.1.md) |
| targetsの祖先だけ実行、複数需要の共有、最後の参照が消えた仕事だけ取消、run間の実行を共有しない | [Graph検査](../tests/execution_graph.rs)、[Pump検査](../tests/execution_pump.rs) |
| Pendingを文章候補へ混ぜないControlFrame、入力/Timerの観測、Wait予約の置換、変化なしの再観測をしない | [observer検査](../tests/execution_observers.rs)、[Pump検査](../tests/execution_pump.rs) |
| runは原子的な受理予約、jobは物理handoff、Waitは判断採用を起点に測定 | [時計と採用](execution-clocked-pump.md)、[時計検査](../tests/execution_clocked.rs)、[準備/採用の検査](../tests/execution_staged.rs) |
| 期限を飛ばさず、同時刻はqueue→run→job→observer→grace。表示msの切捨てを期限比較へ再利用しない | [Pump検査](../tests/execution_pump.rs)、[仕事期限検査](../tests/execution_job_deadlines.rs)、[直接照合表](execution-conformance.md) |
| 協調取消と実終了を分離。開始前取消はNodeを呼ばず真正な終了を通知。開始済みの非協調仕事はstalledでも所有を保持 | [worker](../crates/runtime/src/execution_control/worker.rs)、[仕事期限検査](../tests/execution_job_deadlines.rs)、[サービス検査](../tests/execution_service.rs) |
| 同じHostのFIFO・有界資源・原子的な初期予約。busyは旧run/入力/保持結果を変えず、後続busyはNodeErrorへ解決 | [dispatch検査](../tests/execution_dispatch.rs)、[資源検査](../tests/execution_resources.rs) |
| 完全キーと私有authority、旧通知/重複通知の拒否、真正な旧仕事だけ回収、terminal結果の不変 | [私有認証の検査](../crates/runtime/src/execution_control/worker.rs)、[台帳検査](../tests/execution_ledger.rs)、[結果検査](../tests/execution_results.rs) |
| 型検証済みpayloadでの開始重複排除、保持TTL/古い結果の追出し、retired/unknown/goneの区別、結果上限の明示失敗 | [台帳検査](../tests/execution_ledger.rs)、[結果検査](../tests/execution_results.rs)、[直接照合表](execution-conformance.md) |
| 入力・配送世代と計画を分離、固定runの入力更新からの独立、prepareは不活性、activate/rollbackで旧runを復活させない | [台帳検査](../tests/execution_ledger.rs)、[Pump検査](../tests/execution_pump.rs)、[サービス検査](../tests/execution_service.rs) |
| Controller交換後も同じHost/時計/完了受信先/使用枠を保持、最後のcloneでSession close、4 Session/16未完了要求、OS乱数epoch | [共有サービス](execution-service.md)、[サービス検査](../tests/execution_service.rs)、[所有・上限・故障の単体検査](../crates/runtime/src/execution_control/service.rs) |

変換・辞書・品質評価・採用候補の意味はNode側に置く。新方式にも一律の処理時間上限やLLM個数制限を追加しない。run/job/queueの無期限設定は有界な資源枠の中で選べる。

## 正式fixtureと検査結果

[全54件の対応表](../tests/execution_coverage.json)と[照合の説明](execution-conformance.md)を参照。公開Core入口43件と私有境界8件、計51件を正本JSONのイベント・全expectedから直接検査する。私有6件は1単体テストで全件実行し、開始前取消1件はsync/deferredの2配置、時計ジャンプ1件は一度の0→30msで処理する。個別テスト件数と正式シナリオ数を混用しない。

未接続のB03（応答消失後の照会）、B07（実切断/再接続）、B08（v2からの非暗黙移行）はP2へ残す。既存B系のCore投影や型検査が通っていても、実CLI/HTTPの適合には数えない。P2では残3件に加え、重複開始・保持切れ・ID境界・形式異常等も実通信で再検証する。

上記feature SHAで`cargo test --locked`全395件、`cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo build --locked --bin diy-ime2`が成功した。環境はRust/Cargo 1.90.0、Windows x64 GNU、実行中の別exeを上書きしない専用target。UI/ブリッジを変更していないため今回npmを再実行していない。MSVC/TSF/IME登録・正式な性能目標達成は未検証。独立AIレビューやCI通過としては数えない。

再実行の入口:

```powershell
cargo test --locked
cargo test --locked --test execution_fixtures
cargo test --locked --lib formal_private_fixture_cases
cargo test --locked --lib formal_prestart_fixture
cargo test --locked --lib formal_run_deadline_and_grace_fixture
```

正本JSONの採用時のimplementationStatus属性は変更していない。現在の照合状況は対応表で追跡し、契約データを読み込めたことを実装の合格に置き換えない。

## P2以降への引継ぎ

1. 初期no-active-planのSessionと最初のprepare/activateを接続する。epoch、state.revision、planVersion、prepared token、rollback、入力世代を混用しない。
2. 別protocol 3のJSON Linesと明示CLIモードを実装する。u64識別子は十進文字列のまま保持し、要求1 MiB・応答8 MiB・4 Session/16未完了要求を維持する。旧session/v2の既定・宣言・#45の設定を変更しない。
3. ホストの全ResourcePolicyとresourceDomainをhelloへ出す。構成した値を隠れた固定値へ変換しない。接続交換・子プロセス交換で旧仕事の所有を消さず、真正な終了前に新しい私有予算で回避しない。
4. start/get-start/get-run/cancel/invalidate-inputを実CLI/HTTPへ接続する。受理応答は完了ではない。接続監視はrun/job期限と別で、切断/監視不能をNodeErrorへ偽装しない。開始を自動再送・自動補償しない。
5. P3でUIへ実行中/terminal、復旧と原因、取消要求/stalled/実終了、保持切れと接続故障を表示する。ローカル入力編集直後から旧結果を失効し、ケーブル表示にも完全キー照合を使う。

新規製品ノードの開発停止を維持する。P4の時刻・選択ノード、辞書の正式採用、任意dataTypeの公開、外部プラグインABI、隔離ホスト、Windows TSFは別工程である。辞書の形式・検索・読込方針を本体の必須仕様へ固定しない。

## 限界と戻し方

任意NodeのCPU/一時ヒープ、非協調callbackの強制停止、副作用の巻き戻しは保証しない。Controlのobserveは信頼された短い処理が前提である。Core制御所有者の内部故障は接続故障として閉じ、自動再起動・自動再送で隠さない。ready-plan Core APIの完成を、公開プラグインABIや実IMEの完成として扱わない。

この引継ぎ文書はrevertで戻せる。Core機能を戻す場合は新受理を止め、生存仕事の終了を確認し、対象PRを依存の逆順でrevertする。旧workerの結果を捨てただけで終了確認済みにしない。各段階のIssue/PR、feature/merge SHA、検査、意図、制限は親Issueの記録と各専用文書に残している。
