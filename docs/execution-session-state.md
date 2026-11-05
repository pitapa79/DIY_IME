# 空Sessionと初回の計画有効化

Issue #170。P1のready-plan APIに続く、P2のRust Core接続。[P1受け入れ](execution-p1-handoff.md)はその時点の記録として残す。

`ExecutionService::open_empty()` は、同じHost・Clock・制御所有者のもとに、稼働プランも準備済みプランもないSessionを作る。ready-plan用の `open_ready(plan, version)` は維持する。旧Engine・session protocol 2・標準Registry・Graph/Packetは変更しない。

空Sessionのrevisionは0、active planはなし、prepared/rollbackはfalse、latest input revisionはなし、delivery generationは1。`prepare(plan)` はコンパイル済みプランを保持してrevisionだけを増やし、仕事を開始しない。再準備では前の準備済みプランを解放し、最後のプランを保持する。`activate()` は同じepochの最初の稼働プランをversion 1として有効化してrevisionを増やす。空の旧プランを復帰履歴として作らず、generationは初期値1を使う。以降の有効化・復帰は従来のLedgerへ接続する。

準備なしのactivateと、復帰先なしのrollbackはfalseを返し状態を変えない。空Sessionではstart/invalidate/plan_versionはno-active-plan、get_startはstart-not-seen、同じepochのget_run/cancelはrun-unknown。異なるepochのRunKeyは先にsession-conflictで拒否する。失敗したstartは受理済みstart keyやrun IDを消費しない。発行用 `next_start_key` のカウンターは受理最大値とは別のまま。

`ExecutionController::control_state()` はrevision・active_plan_version・has_prepared・can_rollback・latest_input_revision・delivery_generationを一つの制御要求で読む。Ledger/Pumpにも同じ読取専用ビューを設けた。読取り自体はPumpの時計前進・worker採用・Node観測を起動しない。独立した完了・期限・制御時計の更新は、従来どおり制御所有者が要求とは別に処理する。

`SessionControlState` はRustの型であり、wireのstateではない。JSON用Serializeは実装しない。protocol 3のpreparedToken、revisionの十進文字列化、statusのグラフ、要求/応答の原子的スナップショットは通信層の後続タスクに残す。

空Sessionもready Sessionと同じ4枠を使う。最後のController cloneがなくなると準備中のプランとSession枠を解放する。プラン有効化だけでは実行資源を予約せず、実仕事の受理は既存の原子的予約へ渡す。古いSessionの物理仕事が残れば、新しい空Sessionを有効化しても同じHostのbusy判定に従う。古い仕事の取消受付や完了通知だけで枠を回収せず、producerの実終了とstart callbackの復帰を待つ。

revision枯渇は準備の置換・初回有効化より前に検査する。有効化途中の失敗時も準備済みArcをSessionへ残す。公開のready-plan入口は従来どおりrevision 0から始まり、既存利用者の計画番号を変えない。

検査は [service単体](../crates/runtime/src/execution_control/service.rs) の実Arc寿命・revision枯渇と、[共有サービス検査](../tests/execution_service.rs) の初期状態/不変性・初回実行・後続の切替と復帰・4枠・取消済み旧producerとの競合・読取りによる再観測なしを使う。正式fixtureと期待値を変更しない。wire/HTTP/UI/TSF/MSVCはこの検査の対象外。

戻す場合は新しい受理を停止し、この変更で開始した実仕事の終了を確認してから対応PRをrevertする。既存 `open_ready` のみを使う呼出側には移行を要求しない。
