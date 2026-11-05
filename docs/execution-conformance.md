# 正式受け入れ例と実装の照合（P1-3q）

[Issue #166](https://github.com/pitapa79/DIY_IME2/issues/166)、親[#133](https://github.com/pitapa79/DIY_IME2/issues/133)。正本は[契約v0.2](execution-control-contract-v0.2.md)と[54件のfixture](../tests/contracts/execution_control.json)。期待値を変更せず、[機械可読な対応表](../tests/execution_coverage.json)で全IDを列挙する。

## 検査の意味

**公開Core入口43件（direct-core）と私有境界8件（direct-private-core）、計51/54件を直接検査する。このCore分類で残る3件はrelated-onlyであり、Coreの適合済みとは数えない。** 関連テストが通ることと、正式イベント列の全expectedを通すことは区別する。表の関連テスト名は実在を検査し、正式IDの欠落/重複や、直接実行リストとの不一致も失敗にする。通信v3の完成はどのCore検査からも推測しない。

tests/execution_fixtures.rsは正式JSONからprofile、仕事名、targets、イベント時刻・入力・予算・完了Packet・expectedを読む。実Registry/ExecutionPlan/Pump/workerと不透明permitを使い、仮想時刻の進行と実workerの開始/終了待ちを分離する。実時間待ちの5秒は検査のハング検出で、run予算ではない。後片付けは自分のpermitへ実終了を通知し、自分の枠だけ回収する。

テスト専用のfirst-data/recovery/observerはノード側に置き、Coreへ固定選択規則を加えない。通常joinは一方がPendingの間に呼ばれたら失敗する。A06のwait-observerはテストpolicyのWait設定と実予約を照合する。T04のworkerReportedMsは意図的にCoreへ渡さず、実配送時刻で失効することを確認する。cancelReplyはCore snapshotの既終端性から投影し、wire応答の検査と混同しない。

未対応op/expectedはpanicとして失敗させ、無視して成功にしない。session_revisionは計画状態のrevisionを読むだけの入口として追加し、A07の取得前後を比較する。eventSeqやplanVersionをstate.revisionの代用にしない。直接実行リストを新規JSON全体への自動適合としない。

Issue #156では入力失効、prepare/activate/rollback、固定run、入力競合、重複完了、明示再送、保持切れ、時計後退の14件を追加した。次計画には旧main.outを置かず、旧開始の再送が現在のtargets検証へ誤って流れないことも確認する。時計後退の失敗では直前のsnapshot・使用枠・計画revision・時計を比較する。完了permitの最初の終了だけを数え、実際の物理枠回収まで待つ。保持結果が消えたケースでは取得失敗をそのまま期待値へ照合する。

B01/B02/B04/B05/B06/B10は、正式な通信分類ケースのCore投影を直接検査する。実CLIの応答消失や切断・再接続を再現した検査ではない。automaticResubmissionsは観測した実仕事の開始数と明示受理数の差を検査する。B03/B07/B08は後続の[正式transport検査](execution-transport-conformance.md)で別途イベント列と全expectedを照合した。Core単独の51/54分類は維持し、対応表のtransport欄で区別する。

planRevisionUnchangedは契約どおりplanVersion・state.revision・preparedをすべて比較する。session_has_pendingは準備済み計画の有無を読むだけの追加入口で、時計・観測・計画を変更しない。今回の不変ケースは初期preparedがないため、計画の有無を比較できれば十分である。準備済み計画を別内容へ置換するケースでは、その同一性も別途照合する。initial.latestInputRevisionはイベント実行前にCoreへ設定し、最初のstartが偶然同じrevisionを束縛することに依存しない。

## 通信境界の追加検査

Issue #192ではB03/B07を実HTTP、B08を検査Registry付きの実v2子プロセスで正式fixtureから直接実行する。固定epochは実発行値への束縛表で比較し、時計操作は検査Hostの外側だけへ置く。`tests/execution_coverage.json`の既存status/evidenceはP1の意味を維持し、3件だけにtransportを付加した。これは全54件の通信適合を示さない。検査手順と投影の限界は[通信照合の記録](execution-transport-conformance.md)を参照。

## 全54件の対応

| 正式ID | 状態 | 検査根拠 | 残工程/境界 |
|---|---|---|---|
| A01-normal-completion | direct-core | [a01_normal](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| A02-pending-is-not-no-output | direct-core | [a02_pending](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| A03-first-data-cancels-unneeded | direct-core | [a03_first_data](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| A04-normal-merge-waits-all | direct-core | [a04_all_inputs](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| A05-shared-job-keeps-owner | direct-core | [a05_shared_demand](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| A06-wake-without-new-input | direct-core | [a06_timer](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| A07-poll-does-not-reobserve | direct-core | [a07_pure_poll](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| T01-before-run-deadline | direct-core | [t01_before](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| T02-exact-run-deadline | direct-core | [t02_exact](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| T03-after-run-deadline | direct-core | [t03_after](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| T04-worker-timestamp-does-not-backdate | direct-core | [t04_no_backdate](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| T05-job-deadline-can-recover | direct-core | [t05_recovery](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| T06-unlimited-does-not-expire | direct-core | [t06_unlimited](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| T07-clock-jump-processes-deadline-and-grace | direct-private-core | [formal_run_deadline_and_grace_fixture](../crates/runtime/src/execution_control/pump.rs) | 一度の時計ジャンプをcfg(test)のCore遷移記録で照合。製品ログ/通信v3の完成を意味しない。 |
| C01-cancel-before-completion-same-time | direct-core | [c01_cancel_first](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| C02-complete-before-cancel-same-time | direct-core | [c02_complete_first](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| C03-repeat-cancel-no-double-release | direct-core | [c03_repeat_cancel](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| C04-ack-of-receipt-is-not-termination | direct-core | [c04_ack_is_not_end](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| G01-input-invalidation-rejects-old-result | direct-core | [g01_invalidate](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| G02-prepare-does-not-invalidate | direct-core | [g02_prepare](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| G03-activate-invalidates-but-reclaims-old-job | direct-core | [g03_activate](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| G04-rollback-never-resurrects-version | direct-core | [g04_rollback](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| G05-other-epoch-is-not-old-job-ack | direct-private-core | [formal_private_fixture_cases](../crates/runtime/src/execution_control/worker.rs) | 私有通知認証のcfg(test)検査。公開の偽造通知入口や通信v3の完成を意味しない。 |
| G06-duplicate-completion-reclaims-once | direct-core | [g06_duplicate_completion](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| G07-revision-content-conflict | direct-core | [g07_input_conflict](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| G08-fixed-run-survives-input-update | direct-core | [g08_fixed_run](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| R01-cancelled-work-keeps-capacity | direct-core | [r01_cancelled_capacity](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| R02-queue-expiry-before-same-time-slot-return | direct-core | [r02_queue_expiry](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| R03-oversized-result-is-not-success | direct-core | [r03_result_limit](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| R04-invalid-profile-refuses-new-controller | direct-core | [r04_invalid_profile](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| B01-same-start-is-idempotent | direct-core | [b01_replay](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| B02-start-key-with-changed-payload | direct-core | [b02_replay_conflict](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| B03-lost-response-can-be-queried | related-only | [input_replays_and_plan_switches_keep_the_same_service_contract](../tests/execution_service.rs) | [transport別欄](execution-transport-conformance.md)で正式イベント/全expectedを照合。Core分類は保持。 |
| B04-key-after-retention-cannot-relaunch | direct-core | [b04_retired_start](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| B05-expired-result-is-explicitly-gone | direct-core | [b05_result_gone](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| B06-tombstone-loss-does-not-prove-not-started | direct-core | [b06_result_unknown](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| B07-reconnect-rejects-old-run-key | related-only | [dropping_all_handles_then_reopening_reuses_the_old_budget_and_completion_receiver](../tests/execution_service.rs) | [transport別欄](execution-transport-conformance.md)で正式イベント/全expectedを照合。Core分類は保持。 |
| B08-v2-does-not-negotiate-v3-implicitly | related-only | [execution_declarations_are_opt_in_and_legacy_engine_and_session_reject_new_kinds](../tests/execution_workers.rs) | [transport別欄](execution-transport-conformance.md)で正式イベント/全expectedを照合。Core分類は保持。 |
| B09-u64-ids-stay-strings | direct-core | [b09_exact_ids](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| T08-invalid-budget-is-pre-admission | direct-core | [t08_invalid_budgets](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| T09-duration-overflow-is-rejected | direct-core | [t09_checked_add_limit](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| B10-replay-is-not-revalidated-against-new-plan | direct-core | [b10_replay_after_activation](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| A08-node-no-output-is-a-resolved-result | direct-core | [a08_no_output](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| C05-cancel-before-worker-start | direct-private-core | [formal_prestart_fixture_skips_both_sync_and_deferred_callbacks](../crates/runtime/src/execution_control/worker.rs) | 私有開始ゲートでsync/deferred双方を検査。通信v3の完成を意味しない。 |
| G09-mismatched-plan-cannot-reclaim | direct-private-core | [formal_private_fixture_cases](../crates/runtime/src/execution_control/worker.rs) | 私有通知認証のcfg(test)検査。公開の偽造通知入口や通信v3の完成を意味しない。 |
| G09-mismatched-run-cannot-reclaim | direct-private-core | [formal_private_fixture_cases](../crates/runtime/src/execution_control/worker.rs) | 私有通知認証のcfg(test)検査。公開の偽造通知入口や通信v3の完成を意味しない。 |
| G09-mismatched-input-cannot-reclaim | direct-private-core | [formal_private_fixture_cases](../crates/runtime/src/execution_control/worker.rs) | 私有通知認証のcfg(test)検査。公開の偽造通知入口や通信v3の完成を意味しない。 |
| G09-mismatched-delivery-cannot-reclaim | direct-private-core | [formal_private_fixture_cases](../crates/runtime/src/execution_control/worker.rs) | 私有通知認証のcfg(test)検査。公開の偽造通知入口や通信v3の完成を意味しない。 |
| G09-mismatched-job-cannot-reclaim | direct-private-core | [formal_private_fixture_cases](../crates/runtime/src/execution_control/worker.rs) | 私有通知認証のcfg(test)検査。公開の偽造通知入口や通信v3の完成を意味しない。 |
| G10-get-run-does-not-retarget-a-key | direct-core | [g10_wrong_key](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| R05-zero-queue-refuses-before-admission | direct-core | [r05_no_queue](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| R06-later-ready-work-may-fail-without-unaccepting-run | direct-core | [r06_later_fanout](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| T10-clock-backwards-does-not-change-state | direct-core | [t10_backward_clock](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |
| G11-same-input-keeps-separate-runs | direct-core | [g11_distinct_runs](../tests/execution_fixtures.rs) | 通信v3/実CLIの適合をこのCore検査だけで主張しない。 |

## 形式と加算の境界

Issue #158はR04/B09/T08/T09を追加する。R04はprofileの検証からHost/共有サービス生成までの実constructor境界で拒否し、作成・受理に進まない。B09はDecimalIdの実serde境界へJSON値を通し、最大u64を文字列として往復する。T08はRunBudgets::from_valueで予算専用JSONを型検証し、値域・型・未知field・重複nodeIdをinvalid-deadlineへそろえる。外側の通信包絡の検査やerror分類を置き換えない。errorsは契約どおり異なるコードの集合、rejectedValuesは記載順で照合する。

T09はDurationの加算限界までnativeRemainingMsだけ残す注入時刻を配置する。runの受理前にchecked-addが失敗し、枠・revision・開始キーが不変であることを確認する。製品の予算上限やOSタイマーを短く変更するものではない。

## 容量・待機・後続仕事

Issue #160はR01/R02/R03/R05/R06を追加する。受理後のrunning/queuedを区別し、待機仕事の開始を待つために時計を勝手に進めない。物理完了から後続仕事へ同じ枠を渡す場合は、新しい仕事の開始通知もsettleの根拠にする。queued/running/受理/実開始と、初期busy後の最新入力revisionを実Coreから照合する。session_latest_input_revisionは読取りだけの入口である。

R02はキュー期限と古い仕事の終了確認が同時でも、失効した仕事を起動しない。R03は指定byte数の実Packetをpermitで配送し、最小失敗報告のresult-limitをerrorsへ照合する。R06はseedからa/bを経て通常joinへ接続し、後続のresource-busyを受理取消にせず、元の原因と実開始数を確認する。errors/causeCodesは契約どおり異なるコード集合として比較する。

R06のa/bは通常の同期Nodeで、executeを実worker上の停止ゲートで待たせる。完了イベントでそのゲートへOutputsを渡し、Core自身が通常の同期実行結果として処理する。遅延permitで同期Nodeを代用しない。受理済みRunKeyはseed完了前にテスト側の対応表へ記録し、workerから届いた実run IDに束縛する。後片付けでもゲートを解放し、取消通知だけで実終了と見なさない。

## 私有完了通知の認証

Issue #162はG05とG09の5件を追加する。公開permitで別キーを装う代用はせず、worker.rsのcfg(test)から本物のprivate authorityを持つ通知のstampだけを変える。正本JSONの初期状態・入力・予算・時刻・Packet・expectedを使い、実Registry/Ledger/workerで採用と回収の両方を拒否する。対象IDはtests/execution_private_cases.jsonの6件で、1つのRust単体テストが全6件を順に実行する。公開Coreの43件と重複せず、対応表の集合照合と参照実在も検査する。

Core観測のignored-completionは、実consumeが通知を拒否した時点を検査側がLedgerの現在時刻とともに記録したもの。通信errorや製品の永続ログを新設していない。本物のproducerは未完了のまま保持し、偽造通知の照合後にだけ協調取消と真正な実終了通知を配送する。その真正な通知で初めて物理枠と保持報告の物理数が0へ戻ることも確認する。固定の不一致IDと偶然同じ不透明IDが発行された場合は、dispatch前に新しい隔離された事前配置を作り、比較対象が本当に異なる状態を保つ。製品のIDや通知入口は変更しない。

## 実呼出前の取消

Issue #164はC05を追加する。workerがcallbackの開始確認をする前に取消tokenが立っていれば、Nodeを呼ばずNotStartedの私有通知を返す。Coreは正しいstamp/private authorityと受理済み取消を確認し、実workerの終了として一度だけ回収する。開始確認後に到着した取消は従来の協調取消として扱う。実行中のNodeの強制停止は追加しない。

未使用の遅延permitのfinishedを確定し、保有lease/Invocationを通知前に外す。dispatcher自身の所有は真正な通知の採用まで残す。Mutexを外してから保有値を破棄する共通release処理を使い、論理cancelledへ偽Data/NodeErrorを追加しない。

cfg(test)の開始ゲートで実workerを待たせ、同じ正式C05をsync/deferred双方へ流す。ゲートはsender破棄でも解放される。通知の採用前に枠1、採用後に枠0/queued0/callback開始0、未使用permitのlease/Invocation解放を照合する。Native Timerの独立起床はsupplierなしのGraph、開始済みproducerの枠保持は実開始通知と注入時計による別検査で両方を維持し、OSの5ms以内の起動順を期待値へ混ぜない。

Controlの準備中に仕事期限へ到達する既存検査も、producerが必ず開始するという待ちを外す。実producerが開始済みなら取消tokenを確認し、未呼出なら真正な終了で物理枠0を確認する。deadline-exceededからのrecovered結果・Started/Inputの観測順は同じ期待値で維持し、後片付けではどちらの場合も自分の実仕事を回収する。

## 一度の時計ジャンプとCoreの観測時刻

Issue #166はT07を追加する。実開始済みのproducerを未完了で保持して0→30msを一度だけCoreへ渡す。clock_stepで本当にexpiredへ変わったrunと、flush_graceで本当にstalled数が増えた時点をcfg(test)だけで記録する。渡した処理時刻をそのまま記録し、期待値の10/15msから検査ログを合成しない。Coreの期限走査が途中時刻を飛ばした場合はnotice列が一致せず失敗する。

全expectedに加えて、真正なproducerの終了後に物理0へ戻っても、expiredの論理報告とelapsedが変わらないことを確認する。対象ID一覧と対応表も照合する。テスト用記録は通常ビルドから除かれ、製品の永続ログ/新しい観測API/時間制限を追加しない。当時残工程だったB03/B07/B08は後続#192のtransport別欄で照合したが、このCore検査の適合済みには加算しない。

## 検査と戻し方

公開43件と私有8件、計51件および対応表を検査し、Rust全体・fmt・clippy・CLIを実行する。UI/ブリッジ変更なし。新規製品ノード、標準Registry、Graph/Packet、旧Engine/session v2、正式fixtureは変更しない。本工程でP1全体/通信v3/UI/TSF/MSVCを完了扱いしない。本工程#166は検査・test-only観測と対応表のrevertで戻す。過去の#164の開始前取消の本体変更を戻す場合は、新受理を止め、生存仕事の終了を確認して該当PRをrevertする。
