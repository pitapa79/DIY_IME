# S1の同一呼出し計測（#113、検証中）

親Goal #109。開始時mainは`057a0e28d88103269db1e6d1268af5fd5ac88710`。
実験基盤は未マージPR #112の`6ee78f42fd3b2dc355b0ce6e22c658bcbf12ea15`に依存する。
PR #105/#103も未マージであり、通常mainへの統合・製品/default採用は含まない。

## 測定するもの

同一の通常HostのNode呼出しについて、Node全体のwall時間と、trusted Nativeが測る
`diyime_call`のwall時間を対応付ける。後者の内側でresource callback/IPCを待った時間を
別に測り、基盤の待ち時間をNode固有処理として差し引いて隠さない。
Store/Instance作成、ABIの準備/コピー/検証、IPC、終了・回収は基盤費側に残す。
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
当該callの追加256bytesを保持中に予約し、固定長メタデータと、有限の16件ringを会計する。
欠測・不正・古いcall結合は正常な0ms値にせず、drop数も記録する。

関連単体試験は11成功・8ignored。実Workerでは20msのresource callback待ち、
診断の予約保持/解放、実行中取消と250ms期限切れを確認した。
通常Hostのcompiled単段では初回とwarm3件が正答、同一呼出しの対応付けが成功し、
Host終了後のworkspaceファイル0件と外側Jobの全子孫終了を確認した。
この初期版のguest spanにはexport検索・fuel設定などのNative側処理も含まれるため、
純粋なWasm処理やGoal109の基盤P95/P99の証明には使わない。測定境界を絞り直す。

Core traceは識別子をalias化するため、現在の解析は、受入順序と依存順序・kindを検証した
既知の単純な直列グラフに限定する。並列/分岐グラフへこの対応付けを流用しない。

## 未完了

元所有者の回収/再生成段階、guest測定境界の精密化、raw/compiledと診断OFF/ONの比較、
4段/20段、100ms/20ms・復旧、既存安全回帰と最新CIを検証する。
一般のHost審査資格、main統合、全グラフ/変更境界/長時間運用、Goal109完了は別に残る。
