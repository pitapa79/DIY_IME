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
このローカル試験時点では、同一ソースのWindows CIは未実行だった。

## Windows CI 44661420851の観測

計測対象は通常Hostを使った合成入力と実AppContainer Workerで、保持と診断を実験設定で有効にした。
計測レポートとパッケージのソースは `ba1845258222e58593561c90f0298a1e52e316aa`。
同じ実バイナリの事前コンパイル有無を別集計し、各条件のウォーム2試行から各30入力を集計した。
同一呼出しでNode区間からWasm API内の時間を引いた。リソース待ちはNode区間に残した。
以下は観測できた区間の値で、受付・公開をすべて含む基盤追加遅延は未判定。

| 構成 | 事前コンパイル | 対応呼出し数 | P95 (ms) | P99 (ms) | 最大 (ms) |
| --- | --- | ---: | ---: | ---: | ---: |
| 単段 | なし | 60 | 0.891 | 0.948 | 0.948 |
| 単段 | あり | 60 | 0.882 | 0.911 | 0.911 |
| 直列4段 | なし | 240 | 0.840 | 0.893 | 1.422 |
| 直列4段 | あり | 240 | 0.836 | 0.904 | 1.262 |
| 直列20段 | なし | 1200 | 0.817 | 1.084 | 2.911 |
| 直列20段 | あり | 1200 | 0.817 | 1.081 | 1.317 |

上表の計測欠落は0件。20段のコールド起動直後に20ms間隔で入力した場合、
初回のバーストの最新入力は各条件とも2/2試行で正答した。
同じNode区間が20ms以上の呼出しは、事前コンパイルなしで9/40件、ありで6/40件。
コールド条件では観測できた区間だけでも応答予算を超える事例が残る。
[同一呼出しの生データ](https://github.com/pitapa79/DIY_IME/actions/runs/44661420851/artifacts/14597515936)

同一のウォーム試行で、`run-accepted` から `core-result-ready` までの時間から、
重なりのない全Nodeの `node-started`〜`call-ended` を引いた。入力Nodeの呼出しも差し引いた。
Core結果準備から `host-response-published` までは別に計測した。各条件60入力で欠測なし。

| 構成 | Node呼出し外 P95 (ms、事前コンパイルなし／あり) | 結果準備から公開 P95 (ms、同順) |
| --- | ---: | ---: |
| 単段 | 0.677／0.658 | 0.692／0.698 |
| 直列4段 | 1.392／1.375 | 0.747／0.752 |
| 直列20段 | 4.855／4.860 | 1.113／1.167 |

呼出し外の経過時間にはスケジューリング待ちも含まれ、最初の受付イベント以前の処理は範囲外。
純粋なCore Overhead P95 < 1msの判定には、Core自身の実行時間と待ちの分離が残る。

20段で取消・異常終了の後に最新入力を投入した比較は、事前コンパイルなし→あり→あり→なしの順で実行した。
投入から正答確認まで、取消後はなし339〜346ms／あり74〜77ms、異常終了後はなし339〜345ms／あり80〜89ms。
4条件すべてで旧結果を遮断し、全子プロセスと作業ファイルを回収した。メモリ計測欠測は0件。
[取消・異常終了後の復旧](https://github.com/pitapa79/DIY_IME/actions/runs/44661420851/artifacts/14596776106)

同じCIの起動方式比較では、`OpenProcess` が失敗した3件でプロセスメモリを確認できず、集約チェックは不合格。
20段のアイドル復旧中の取消と保護は成功し、取消で観測対象のWorkerを回収、保護で旧キーを拒否して子プロセスと物理枠を解放した。
[起動方式比較](https://github.com/pitapa79/DIY_IME/actions/runs/44661420851/artifacts/14597255988)・[取消と保護](https://github.com/pitapa79/DIY_IME/actions/runs/44661420851/artifacts/14597507436)

## 復旧の再検証と残る制約

#174 / Draft #175 の HEAD `6a66299781d1026a41dccf34ea3181185ada094e` では、Windowsの必須集約を含む11ジョブが成功した。
同一バイナリで依存順を付ける前後のアイドル復旧は698.311msと473.406msだった。各1試行、両方とも最大同時起動2、
旧Workerの後継20件を最新RunKeyに対応付け、各26/26のwarm入力が正答し、メモリ欠測は0件だった。
別の取消と保護の試行でも、実起動したWorkerと最終回収を照合した。
[Windows CI](https://github.com/pitapa79/DIY_IME/actions/runs/44483499942)・[依存順なし](https://github.com/pitapa79/DIY_IME/actions/runs/44483499942/artifacts/14511680159)・[依存順あり](https://github.com/pitapa79/DIY_IME/actions/runs/44483499942/artifacts/14511471128)・[取消と保護](https://github.com/pitapa79/DIY_IME/actions/runs/44483499942/artifacts/14511916181)

Windows CI 44693112708の合成マージは `87de8df5690c466267cc2b877c6111bf39405f7c`。
同じ配布バイナリの20段で、事前コンパイルなし→あり→あり→なしの順に測定した。

| 起点から最新入力の正答まで | 事前コンパイルなし (ms) | 事前コンパイルあり (ms) |
| --- | ---: | ---: |
| 取消後の新入力の投入 | 333.1、338.6 | 74.7、76.4 |
| 異常終了後の新入力の投入 | 336.8、337.3 | 75.8、81.4 |
| 取消要求の直前 | 383.3、385.8 | 121.3、124.3 |
| 異常終了を起こすヘルパーの起動直前 | 406.1、415.6 | 140.3、150.1 |

各モード2試行。元プロセスの終了、古い結果の遮断、新入力の正答、全子孫と作業ファイルの回収を確認。
[復旧の成果物](https://github.com/pitapa79/DIY_IME/actions/runs/44693112708/artifacts/14610082923)

依存順を考慮した並行アイドル復旧では、20段の各試行で26入力を正答し、約64秒の連続入力と約61秒のアイドルを確認した。
新入力の投入から正答までは、未使用の事前作成プロファイルなしで526〜536ms、ありで309〜320msだった。
同じバイナリで各2試行、全試行で旧結果を拒否し、各20個の元Workerを回収した。
事前作成ありの20呼出しは各試行とも対応するプロファイルを使用した。
[アイドルのプロファイル比較](https://github.com/pitapa79/DIY_IME/actions/runs/44693112708/artifacts/14610911731)

別実行のCI 44698665039でも同じ4アームの比較を完了した。
事前作成なしは486.553／428.505ms、ありは454.544／356.839ms。両条件の観測値の範囲は重なった。
各アームで26/26のwarm入力とアイドル後の最新入力が正答し、旧結果は拒否された。
各20件の元Workerの回収と後継による実呼出しを照合し、事前作成ありの各20呼出しがプールから取得したプロファイルを使用した。
4アームともJobの全子孫と作業領域の回収を確認し、メモリとtraceの欠測は0件だった。
[2回目のアイドル比較](https://github.com/pitapa79/DIY_IME/actions/runs/44698665039/artifacts/14613127895)

先行する別試行では、最新入力の待ちが元WorkerのACL回収と130.5ms重なった。
もう一つの遅延例では、新Workerのプロファイル作成が待ちの203.8msと重なり、元WorkerのACL回収は1.88msだった。
それぞれの待ちを観測した試行であり、両者の原因を一つにまとめられる根拠はない。
[ACL回収の観測](https://github.com/pitapa79/DIY_IME/actions/runs/44661420851/artifacts/14596328282)・[プロファイル作成の観測](https://github.com/pitapa79/DIY_IME/actions/runs/44666605975/artifacts/14598623736)

CI 44693112708の初期起動比較では、安全検査の操作と最終回収は成功したが、プロセスメモリに欠測1件が残った。
欠測直後に対象PIDがJob一覧から消え、後で観測した同番号のPIDには別の作成時刻が記録された。
後の測定値による補完はできず、集約チェックは不合格。
[欠測を含む起動比較](https://github.com/pitapa79/DIY_IME/actions/runs/44693112708/artifacts/14609819494)

CI 44698665039の20段アイドル取消では、観測したstage3のWorker（試行3、PID 4944）が、
プロセス再開から14.85ms後に同じRunKeyを引き受けた。取消終端で古い出力は破棄され、
全子孫と作業領域の回収を確認した。起動途中のWorkerの未採択回収は立証できず、この試験は不合格。
同じ実行の保護試験は旧キー拒否と回収を確認した。
[アイドル取消・保護の成果物](https://github.com/pitapa79/DIY_IME/actions/runs/44698665039/artifacts/14612564830)

## 未完了

Wasm API内の意味的処理区間の分離、受付・公開を含む基盤追加遅延の評価、通常HostのCore計測が残る。
cold起動・再生成遅延と計測欠測を調査し、安全回帰と最新CIを検証する。
一般のHost審査資格、main統合、全グラフ/変更境界/長時間運用、Goal109完了は別に残る。
