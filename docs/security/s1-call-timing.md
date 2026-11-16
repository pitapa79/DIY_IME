# S1の同一呼出し計測（#113、検証中）

親Goal #109。開始時mainは`057a0e28d88103269db1e6d1268af5fd5ac88710`。
実験基盤は未マージPR #112の`6ee78f42fd3b2dc355b0ce6e22c658bcbf12ea15`に依存する。
PR #105/#103も未マージであり、通常mainへの統合・製品/default採用は含まない。

## 測定するもの

同一の通常HostのNode呼出しについて、Node全体のwall時間と、trusted Nativeが測る
`diyime_call`のwall時間を対応付ける。後者の内側でresource callback/IPCを待った時間を
別に測り、基盤の待ち時間をNode固有処理として差し引いて隠さない。
Store/Instance作成、Native側のABI準備/コピー/検証、IPC、終了・回収はspan外に残す。
現在のspanにもWasm内SDKのJSON/ABI処理は含まれる。guest時間を固有処理とは見なさず、
差分はAPI residualと呼ぶ。`goalOverheadAvailable:false`、`semanticWorkMs:null`、
`frameworkOverheadMs:null`を明示する。意味的な処理区間の分離は未完了であり、
Goal109のNode Execution Overhead達成判定には使わない。
wall時間はCPU使用時間ではなく、preemptionも含む。実際の境界と欠測を記録する。

別試行・別ランナーのpercentile同士の減算や、20段全体を20で割る計算はしない。
同一呼出しの差分から得た標本についてP50/P95/P99/最大値を求める。
guest時間や対応するNode時間が欠ける、または整合しない標本を0として補わない。

## 診断の境界

診断は内部のdefault-OFF計測。guestのJSON、Graph、Packet、SDKを拡張しない。
trusted Nativeの有限メタデータを実sequence/moduleへ結び付け、guest resultと区別する。
Wasmが返した数値や診断らしいbytesを計測値として信用しない。
入力・reply・callback・物理枠の既存上限を緩めず、追加の保持/transport分も会計する。
認可・保護・失効・取消・正答判定を診断へ依存させない。

元Workerの物理終了/IO回収、ACL/profile回収、新Worker準備/起動/readinessを
元の所有者に結び付けて記録し、取消後に何を待っているかを区別する。
診断のために回収を先送りして枠を返したり、保証を弱めたりしない。

## 初期試作と検査

`DIY_EXPERIMENT_RETAINED_TIMING=1`と既存retention実験を明示した場合だけ有効。
私有header `DIYRET02`と有限のNativeメタデータを使い、従来の`DIYRET01`は維持する。
診断はRESULTの前の独立フレームで、guest返却bytesは解釈しない。
当該callの追加256bytesを保持中に予約する。owner当たり16件call・192件phaseのringに
327680bytesを既存保持予算から前払いする。既存256KiB予算では拒否する。
欠測・不正・古いcall結合は正常な0ms値にせず、drop数も記録する。

関連単体試験は11成功・8ignored。実Workerでは20msのresource callback待ち、
診断の予約保持/解放、実行中取消と250ms期限切れを確認した。
通常Hostのcompiled単段では初回とwarm3件が正答、同一呼出しの対応付けが成功し、
Host終了後のworkspaceファイル0件と外側Jobの全子孫終了を確認した。
初期単段試作のspanにはexport検索・fuel設定も含まれた。現在はこれらを除外したが、
Wasm内SDK処理は含まれるため、意味的処理と基盤費の完全な分離ではない。

Core traceは識別子をalias化するため、現在の解析は、受入順序と依存順序・kindを検証した
既知の単純な直列グラフに限定する。並列/分岐グラフへこの対応付けを流用しない。

## 現在の観測と失敗（2026-11-16、ローカル）

同一cohortの単調時計で、元owner終了/IO、ACL、profile、joinと、次のattemptのgate、
code準備、profile、code access、Job、起動/readinessを記録する。
attempt・元PID・runを区別する観測であり、OS終了証明や認可判断には使わない。
最終closeではownerとringが破棄されるため、全phaseの保存保証はない。
最終回収は独立したJob・workspace検査で確認する。欠落は0msにしない。

Windows 10.0.19045、i5-7300U、論理4CPU、物理メモリ4179914752bytes。
現行Rustバイナリの通常Host試験（private flag有効、未マージ実験）:

- 20段mixed、診断ON、逐次/100ms/20ms各5回は成功。20ms最新5件は全正答、
  中央値102.383ms、最大956.7086ms。再生成1回のretirement→owner joinは145.157ms。
  初期20ownerを混ぜた再生成の代表値とはしない。workspace残存0、外側Job全子孫終了。
- 対応する診断OFF試験はtrace.dropped=2で失敗。比較結果を採用しない。
- readinessを待たない20段identity coldは成功、準備開始→初回結果26376.9242ms。
  次の1件127.8673ms、workspace残存0、外側Job全子孫終了。
- cold-paced最初の20ms burstの最新入力は正答、応答20300.3507ms。
  最新20Nodeは全Native計測対応。全140Node機会はpaired20/missing5/notExecuted115。
  次roundでtrace.dropped=2となり全試験は失敗。未保存の後続結果を推測しない。
  外側Job全子孫終了。成功したcold入力だけで実用化を主張しない。

全callback終了後にtraceStopし固定snapshotをページ読出しするが、欠落は解消しなかった。
欠落ゼロの検査を維持する。今後の失敗時も部分traceとround結果を保存する。
既存失敗は書き換えない。cold起動、復旧遅延および診断欠落は要修正。

fmt、全target release Clippy、実行ファイル/library harness build成功。
現行関連unitは11成功/8ignored。実Native callback予約/待ちと実行中取消/期限試験は成功。
JS解析・cadenceの8テスト、Python/YAML構文、差分検査は成功。
最新Windows CIは未実行であり、依存PR112の成功を本変更の成功に代用しない。

## 未完了

意味的処理区間の分離、診断欠落、cold起動・再生成遅延、balanced raw/compiledと
診断OFF/ON比較、十分な標本、安全回帰と最新CIを検証する。
一般のHost審査資格、main統合、全グラフ/変更境界/長時間運用、Goal109完了は別に残る。
