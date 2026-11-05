# 実行制御の拡張契約案 v0.1

状態: **未採用の設計提案**。[Issue #44](https://github.com/pitapa79/DIY_IME2/issues/44)の成果物であり、この文書の保存・マージは仕様採用、API追加、ノード登録、後続実装の開始を意味しない。採否はユーザーが項目ごとに判断する。

調査基準: `2d7a26187b686c74132731d5b00c3b6dc7bb43d7`（開始時のorigin/main）。本Issueで変更するのはこの新規文書だけ。以下のフィールド名・状態名は比較と検査用の仮称であり、製品APIではない。

## 1. 目的と現行契約

[設計思想](design-philosophy.md)に従い、Coreは値・失敗・完了・時計・期限・取消・配送を提供し、結果の採用、原文への復旧、速い候補と遅い候補の優先度は構成とノードが決める。一律に全ノードを並列化したり、LLM等の使用数を制限したりする設計にはしない。

| 現行の根拠 | 現在の保証 | 拡張で必要になる差分 |
| --- | --- | --- |
| [コア契約](core-architecture-v0.1.md) §2–4、[Node API](../crates/runtime/src/node.rs) `OutputValue` / `Inputs` / `Node::execute` | Candidateとエラーは別。ノードへ渡す入力はData / NoOutputだけ。全入力解決後に同期実行 | 明示的に対応した入力だけがErrorを受け取る境界 |
| [ランタイム](../crates/runtime/src/runtime.rs) `run_internal` / `failed` | 任意入力を含め上流Errorが一つでもあればBlocked。必須NoOutputならSkipped。前者を優先し、最初の上流診断を伝搬 | 入力ごとの受け取り方と複数エラーの追跡 |
| 同 `execute` / `validate_outputs` | ノードの失敗・捕捉可能なpanic・出力契約違反はFailedとなり全出力へError。プロセス停止は捕捉できない | 復旧可能なrun内エラーと、配送不能な接続故障を分離 |
| 同 `RunReport::succeeded` | Failed / Blockedが一つでもあればfalse。Skippedだけならtrue | 履歴上の失敗と、必要な結果が得られたかの区別 |
| [実行時入力](runtime-input-v0.1.md)、[セッション](session-protocol.md)、[実装](../src/session.rs) | v2は1要求1応答、コマンド間が切替境界。入力でplanVersion・prepared・canRollback・state.revisionを変えない | runを受理した後に完了・取消を扱う独立した境界 |
| [常駐ブリッジ](../editor/server/session.ts) `PersistentCore.request` / `close` | 直列送信。送信済み要求は10秒でセッション終了。送信前取消は要求だけ破棄、送信後切断はセッション無効化 | 処理期限と接続・プロセス監視を別々に扱う |
| [HTTPブリッジ](../editor/server/bridge.ts)、[画面](../editor/src/App.tsx) | 旧入力・旧計画・別セッションの結果を最新扱いしない。自動実行は最新1件を待機 | 完了イベントにも同じ照合を適用し、取消済みrunを失効 |

[実用ノード契約](node-contracts-v0.1.md)と[既存fixture](../tests/contracts/)は採用済みのまま維持する。確認対象は[engine検査](../tests/engine.rs)、[session検査](../tests/session.rs)、[入力検査](../tests/runtime_input.rs)、[bridge検査](../editor/tests/session-bridge.test.ts)、[画面検査](../editor/tests/app.test.tsx)。文書内の古い例が実装と違う場合も、この提案で既存文書を修正しない。特にsession文書の成功包絡例にはprotocol:1が残るが、現在の実装定数と要求・応答は2である。

## 2. 採否を分ける単位

| 判断単位 | 推奨案 | 代替案・保留点 | 現時点 |
| --- | --- | --- | --- |
| E: エラー入力 | 入力宣言の明示的なopt-inと、入力専用のData / NoOutput / Error型 | 専用エラーポート、ノード全体opt-inと比較（§3） | 未採用 |
| R: 復旧と成功判定 | 診断を保存し、従来succeededと結果の利用可否を分離 | succeeded自体の再定義は破壊的（§4） | 未採用 |
| F: 復旧ノード | 同じ型・言語の主経路と原文を選択、Error時だけ復旧する既定案 | NoOutput時の復旧は別の明示設定（§5） | 未採用 |
| A: 後からの完了 | 同期Nodeを維持し、境界アダプターから完了イベントを配送 | 全async化、外部プロセス専用境界と比較（§6） | 未採用 |
| T: 時間と取消 | 単調時計、明示期限、協調取消、世代照合 | 数値上限・具体ABIは後続評価（§7–8） | 未採用 |
| B: 通信 | 非同期受理・状態取得・取消の新契約をv2と分離 | push配送とpollingを比較（§9） | 未採用 |

E/R/Fだけを先に採用し、A/T/Bを保留してよい。採用記録には対象節・変更点・決定者・日付・Issue/PR・正式契約が入ったmainの完全SHAを残す。「方向性に賛成」だけでは正式fixtureやAPIを確定しない。

## 3. エラーを受け取る入力の案

### 3.1 比較

| 案 | 長所 | 問題 | 推奨 |
| --- | --- | --- | --- |
| 入力ごとのopt-in | 通常入力は従来Blocked。復旧対象ポートだけErrorを受け取れる | 新しい入力型・宣言検査が必要 | 採用候補 |
| ノード全体のopt-in | 宣言が少ない | 任意の補助入力の失敗まで処理が必要になり、意図しない復旧が起きる | 不採用を推奨 |
| 独立したエラーポート | 配線でエラー経路が見える | 値とエラーの対応、複数出力、同期条件が複雑になる | 将来比較用に保留 |
| CandidateにErrorを追加 | 既存の文章処理で運べる | 文章候補と失敗を混同し全ノードへ影響 | 採用しない案 |

入力宣言に仮称 `errorPolicy: block | receive` を加え、省略はblockとする案。ノード作者の対応宣言であり、グラフ編集で未対応ノードをreceiveへ変更できない。既存Node::executeのInputs / OutputValueを直接拡張するとRustの網羅的matchが壊れるため、既存同期アダプターにはData / NoOutputだけを渡し、新しい入力インターフェースを明示選択する案を推奨する。具体trait名・宣言の版付けは#46で確定する。既存kind/versionの宣言を後からreceiveへ変えず、新しいkindまたは明示した新版として公開する。保存済みグラフの既存ノードを置換しない。

receive入力の概念値はData(Packet)、NoOutput、Error(診断参照の非空集合)。Pendingは未解決の実行状態であり、NoOutputやErrorへ置換しない。ノード自身の新規失敗は従来のResultエラー相当で返し、入力Errorの再伝搬には元の診断を保つ専用経路を用意する。

### 3.2 全入力が解決した時の規則案

1. すべての接続入力を同じrunで解決する。1ポート1接続は維持し、未接続の任意入力はNoOutput。未接続の必須入力は従来どおり計画検証で拒否する。
2. block入力にErrorがあれば、必須・任意を問わずBlocked。receive入力にもErrorがあってもノードは呼ばず、全入力の原因を診断に残す。
3. block Errorがなく、必須入力にNoOutputがあればSkipped、全出力NoOutput。receiveはNoOutputの実行許可を意味しない。
4. それ以外は実行する。receiveの必須Errorは「解決済み」として渡す。任意Errorも同じくErrorのまま渡し、任意NoOutputはNoOutputとして渡す。
5. 実行結果は宣言済み全出力について検証する。不正Packet・型不一致を成功へ変換しない。

| 入力状態 | block | receive |
| --- | --- | --- |
| 必須Data / 任意Data | 実行候補 | 実行候補 |
| 必須NoOutput | Skipped | Skipped |
| 任意NoOutput / 未接続 | NoOutputを渡す | NoOutputを渡す |
| 必須Error / 任意Error | Blocked | Errorを渡す |

この表は他の入力も含め上記優先順で判定する。同じエラーが複数ポートへ分岐しても診断を増殖させない。元の原因IDと、そこへ至った接続元node/output・接続先node/inputの経路を分ける。原因IDはsession世代・planVersion・runId・発生node・発生連番で一意化し、既存DiagnosticのrunId / planVersion / nodeId / port / code / messageを保持する。port不明ならnullを維持し、特定の出力の失敗と推測しない。複数エラーは入力宣言順、同一入力内は原因ID順で安定表示し、原因はIDで重複排除、経路は別に記録する。集合と経路の拡張は新契約の報告に載せ、v2への報告は従来の代表診断と形式を維持する。既存fixtureの診断を複数件へ置換しない。

## 4. 復旧の診断と成功判定の案

エラーを受け取っただけでは復旧ではない。復旧ノードは「どの入力の、どの原因を、どの出力で代替したか」を実行記録に返し、Coreが今回の入力集合と出力を検証する。元のFailed / BlockedをExecutedへ書き換えず、元診断・復旧ノード・代替出力・選択理由を保存する。

推奨は従来の `RunReport.succeeded` を維持する案。Failed / Blockedが残ればfalseであり、復旧成功でもtrueに変えない。新契約では要求側が必要な終端ポート（仮称targets）を明示し、それらの解決と採用記録から `delivery: complete | recovered | no-output | failed | cancelled | expired` を別に報告する。新契約でこの集計を要求する場合、targetsは存在する出力端点の非空集合を必須とし、未知・重複端点は受理前に拒否する。targets未指定の既存runには新しい成功判定を暗黙適用しない。

- 全targetがDataで代替なしならcomplete、代替ありならrecovered。Dataの空Text / 空SequenceもData。
- targetがErrorならfailed。Errorがなく一つでもNoOutputならno-outputとし、UIは結果なしと表示する。NoOutputを許す出力用途の意味は呼出側が判断する。
- cancelled / expiredはrun全体の終端理由。確定前の部分結果を最終成功として扱わない。部分表示を許す場合は仮結果であることと世代を示す。
- 一つの分岐で復旧しても、別targetへのErrorは未復旧。未選択の枝の失敗は履歴には残すが、その枝を不要と明示した採用結果のfailedにはしない。従来succeededはfalseのまま。
- エラーなしのNoOutputによるSkippedは従来succeededをfalseにしない。必須NoOutputで復旧ノードがSkippedになった場合、Errorを消費・復旧したとは記録しない。

非同期契約のcancelled / expiredは、全評価を終えた従来RunReportとは別の終端応答で返す案とする。途中のノード一覧から従来succeededを計算して成功表示せず、部分結果にはsucceededを付けない。新クライアントは必ずrunの終端理由とdeliveryを確認する。新契約の最終報告形式はR/Bの採用時に確定し、v2のRunReport形式を途中結果用に流用しない。

代替案はsucceededを「必要な結果が得られた」に変える方法だが、既存CLI・UI・テストが成功と解釈する条件が変わるため推奨しない。新しい集計フィールドとtargetsの正式名・複数targetの細部はR採用時に確定し、既存利用者には従来の意味を提供する。

プロセス終了、stdout破損、セッション消失はrun内の入力Errorではない。信頼できる診断配送ができず、復旧ノードの実行も保証できない。v2では従来どおり接続を無効化する。新契約でも接続層の失敗として別に報告し、原文を返したように偽装しない。捕捉可能なRust panicのnode-panicと、abort・クラッシュ・OS killを区別する。

## 5. 後続の独立復旧ノードの契約案

正式kind・struct・ファイル名・versionはF採用時に確定する。以下の「復旧ノード」は仮称で、標準Registryへ登録しない。

| ポート等 | 案 |
| --- | --- |
| primary | 任意入力、errorPolicy:receive、text-packet@1、Any root、言語ja |
| original | 任意入力、errorPolicy:receive、text-packet@1、Any root、言語ja |
| out | text-packet@1、Any root、言語ja |
| 設定 | 仮称 `onNoOutput:false` が既定。true時のみprimaryのNoOutputでも原文を選択。完全な設定オブジェクト・未知キー拒否 |
| 状態 | 純粋処理。過去のrunやRuntimeInput.textを参照せず、配線されたoriginalだけを使う |

両入力を任意にするのは、NoOutputもノードで判断するため。primaryのDataを最優先し、空Text / 空Sequence / 空Choiceも有効Packetとしてそのまま採用する。空ChoiceをNoOutputへ読み替えない。Data選択時に構造・Unicode・score・Text.metaを丸ごと保持し、入力を変更しない。

| primary | original | onNoOutput | out / 診断の扱い |
| --- | --- | --- | --- |
| Data(P) | 任意の解決済み状態 | 任意 | Data(P)。originalのErrorは未選択として履歴保持し、復旧済みとは呼ばない |
| NoOutput | 任意 | false | NoOutput。原文を選ばず、Errorの復旧記録も作らない |
| NoOutput | Data(O) | true | Data(O)、理由はno-output-fallback。エラー復旧とは区別 |
| Error(E) | Data(O) | 任意 | Data(O)。Eをこのoutで代替した復旧記録 |
| Error(E) | NoOutput | 任意 | Error(E)を元の原因のまま再伝搬。未復旧 |
| Error(E) | Error(F) | 任意 | Error(E ∪ F)。原因IDを重複排除、経路保持。未復旧 |
| NoOutput | NoOutput | true | NoOutput |
| NoOutput | Error(F) | true | Error(F)。未復旧 |

言語と型の接続検査を緩めない。ja-Latnのinput.textをこのja原文入力へ直結しない。ここでいう原文は同じ言語の変換前Packetであり、例は日本語校正前の文章。ローマ字から日本語への変換失敗でローマ字を返す用途は、別の言語契約・明示アダプターを採用してから扱う。言語を偽装する汎用ラベルや暗黙変換を本案で導入しない。

## 6. 同期ノードを維持した待機境界

| 方式 | 特性 | 評価案 |
| --- | --- | --- |
| 同期Node + 完了ハンドルを返す境界アダプター | 既存executeはそのまま。重い処理だけ開始・後続完了を分離し、Coreが受理・配送を担当 | 推奨する調査方向 |
| 全NodeをFutureへ変更 | 一つのモデルに揃うが、既存API・実行方式全体へ影響。同期関数を包んでも強制中断はできない | 初期移行には推奨しない |
| 外部プロセスだけを非同期境界にする | 強制終了の隔離単位が明確 | 有力な選択肢。ただし同一プロセス内の軽い非同期I/Oまで必須隔離しない |
| 現行runを別スレッドで丸ごと待つ | UIから受理だけ先に返せる | 内部の全入力待ちや速い候補の採用は解決しない。暫定案 |

通常ノードは全入力の解決後に一度呼ぶ既存規則を維持する。これだけでは「速い候補＋遅い処理」で先に採用できないため、別途明示的な制御ノードの境界を比較する。推奨案は、制御ノードが完了・時計イベントから待機／採用／経路取消の判断を返し、Coreが判断を検証して実行する方法。未完了入力はPendingとして観測し、通常のInputsへ渡さない。ノードがイベント順に依存する場合はその方針を宣言し、汎用Coreが「最初が勝ち」「スコアが高い方」を決めない。

代替案はグラフ外の構成規則で採用を宣言する方法。エディターは簡単になるが判断ロジックがCoreへ入りやすい。#48の独立試作と#49の評価で境界を比較してから正式化する。制御イベントはCandidateへ押し込まない新しい契約であり、NodeのkindやイベントABIは本案では確定しない。

仮の仕事状態は queued → running → completed、またはqueued / running → cancel-requested → cancelled。期限ではexpired、採用対象から外れればdiscardedとして配送権限を失う。completedは物理処理の完了であり、採用とは別。同期Nodeの返却は即時completedイベントに適合させる。runはaccepted → running → completed / cancelled / expiredの一つで終端し、終端後のイベントで再開しない。採用不要になった仕事の物理完了は監査・解放だけに使う。cancelledは配送上の論理終端であり、workerの停止確認は別に持つ。一つのrunで最終結果を一度確定したら、遅い結果で上書きしない。段階的な候補更新を望む場合は別runまたは別途採用するストリーム契約を使う。

未選択経路へは後続を起動せず、分岐の非選択出力はNoOutputに解決する。既に開始した仕事は協調取消し、遅延完了を破棄する。他の利用中経路と共有する仕事は参照を追跡し、利用者が残る間は取り消さない。取消をNoOutputへ一律変換せず、構成が不要と決定した経路のNoOutputだけを正常値として扱う。

## 7. 時計・期限・同時到着の案

時計はCoreの単調時計、期間の単位は整数msを候補とする。OSの実時刻変更に影響されない。基準時刻をブラウザーから信用して渡さず、受理や開始をCoreが記録する。検査では仮想時計を注入する。

| 対象 | 起点・含む時間 | 期限到達時の案 |
| --- | --- | --- |
| run全体 | 検証・容量予約が成功したaccepted時刻。以降のCore内待機も含む。HTTP送信前の待機は含めない | expiredで終端、未開始仕事を起動せず、実行中へ取消要求 |
| 個別仕事 | Coreがrunningへ遷移させた時刻。キュー待ちはrun期限で制御 | その仕事をexpiredとして解決し、受け取り可能な制御境界へ通知 |
| 特定結果を待つ観測 | 制御ノードが待機を登録したCore時刻 | deadlineイベント。待ち続ける／代替を採用する意味は制御側 |
| 接続・プロセス監視 | 別の監視契約の起点 | 接続故障。runの結果選択とは独立（§9） |

個別仕事の期限は親run期限を延ばせない。run全体がexpiredになる前なら、仕事の期限失効を仮称deadline-exceededのErrorとしてreceive対応先に届けて復旧できる。取消により配送を失効させた仕事の後着成功を新しいDataにしない。既定の処理期限は本体の一律10秒から継承せず、明示設定と資源上限を分離する。省略時の待機継続を候補とし、値域・最大値・無期限の表現は採用時に確定する。

期限の競合は「Coreが配送境界で検証・記録した時刻 < deadline」だけを期限内とする案。deadlineと同時なら期限を優先する。外部workerが付けた完了時刻で遡って採用しない。期限前に到着しても採用前に世代が失効すれば破棄する。等しい時刻で取消と完了が競合する場合も取消・世代失効を優先する。すでに期限前に確定したrunへの取消は終端済みを返す。イベント連番を併記し、同時刻の通常完了同士の順序も記録する。

経過時間は入力イベントだけでなく、待機登録時・完了配送時・予約タイマー発火時・状態取得時に観測できる。500msを待つ場合はCoreに次回観測を登録し、入力が来なくても起床する。固定間隔の全ノード再実行は要求しない。状態取得は読み取りであり、採用判断を暗黙実行しない。通常の純粋同期ノードへ時計を暗黙注入せず、再現性の条件を維持する。タイマーの物理的な起床時刻はOSや実行枠の遅延を受けるため、期限時刻ちょうどの関数停止は保証しない。制御ループで長い同期関数を直接待たず、必要な仕事だけ別の実行枠・隔離境界へ分けることを本接続の条件とする。復帰時にも期限を検査して期限後の結果を採用しない。実時間依存の制御については入力スナップショットだけで再現できると主張せず、イベント順・観測時間・判断を記録する。

## 8. 世代・取消・物理停止

配送封筒の概念キーは `(sessionEpoch, planVersion, runId, inputRevision?, jobId, deliveryGeneration)`。sessionEpochは再接続ごとに一意、inputRevision省略は固定グラフとして明示する。同じinputRevisionで再実行してもrunIdは異なる。HTTP要求id、準備token、state.revisionをrunの識別子に代用しない。

受理時に計画版を検査し、次に入力・制御設定・容量を検査し、不正なら仕事を開始しない。配送時と採用時の両方で全キーを照合し、取消済み・期限失効・旧入力・旧plan・旧session・重複job完了・古いdeliveryGenerationを拒否して理由を記録する。UIでは従来のgraphKey・入力本文/modeのスナップショット照合も維持する。

入力更新は新runの受理前でも画面の旧結果を失効させる。新契約でCoreにも失効させるには、呼出側が取消または最新入力世代の通知を送る必要がある。ローカル編集だけをCoreが知るとは仮定しない。Core通知が届く前の結果は画面で拒否する。同じrevisionで違う本文を送る不正も検出するか、通知にスナップショット識別を持つかはB採用時の確定事項とする。

計画prepareだけでは稼働中runを失効させない。activate / rollbackは新規配送を止める安全境界で旧runの配送世代を失効し、旧仕事を取り消してからplanVersionを増やす案。rollbackも版を増やし、旧結果を再有効化しない。物理停止を待たずに切り替える場合、旧plan・資源は残存仕事の終了まで保持する。[準備資源の別提案](prepared-node-resources-proposal.md)の採用や寿命APIを本案から確定しない。

同期Rust関数の強制中断は保証しない。協調取消の確認点・外部I/Oの取消手段はノード作者が用意し、取消要求の受理と実際の停止を別々に報告する。止まらない仕事も配送結果は破棄できるが、CPU・メモリ・接続・副作用は残り得る。取消やrollbackは既に送ったメール・書込・外部API操作を巻き戻さない。外部操作の識別子・重複抑止・再試行方針はノードの契約で宣言し、Coreが自動再実行しない。

資源保護は重い構成の禁止ではなく、明示した実行枠、待機仕事数、残存取消仕事数、結果保持件数/byte数、保持期間の上限を持つ。残存仕事も物理終了まで枠へ数え、取消だけで枠を解放して無限起動しない。容量予約に失敗した要求はbusyとして受理前に拒否し、既存結果や計画を壊さない。実際の数値・共有単位・ユーザー設定幅は試作測定後に決める。同期の無限ループを止める必要がある用途は外部プロセス等で隔離し、killの単位と同居仕事への影響を明示する。信頼できない処理をin-processで安全に強制停止できるとは扱わない。

## 9. ブリッジ・v2互換の移行案

現在のv2は変更しない。長い同期runが処理中なら同じstdinの後続status / cancelを処理できないので、HTTP側にcancel APIだけを追加しても解決しない。

| 通信候補 | 長所 | 条件・制約 |
| --- | --- | --- |
| startRunで即時受理、getRunで状態/結果、cancelRunで取消 | 1要求1応答と直列送信を維持でき、未知のpush応答を扱わない | runは背後の仕事へ分離し、制御ループを塞がない。結果の保持期限・取得cursorが必要。初期候補 |
| 受理後に完了イベントをpush | polling待ち時間を減らせる | 応答とイベントの別フレーム、順序/重複/再取得、背圧が必要。新契約として比較 |
| v2のrun応答を長く待つ | 変更量が少ない | status / cancelが後ろへ詰まる。時間制御の本接続案には不足 |

受理は完了でも成功でもない。概念上startRunはrunキーを返し、getRunはrunning / 終端状態と世代を返す。cancelRunは対象runキーを必須にし、繰返しは同じ取消状態、既存終端なら終端済みを返す。古いsessionや別runへ波及しない。状態取得だけでstate.revisionやplanVersionを増やさず、入力変更をprepare/activateにしない。取消状態と物理停止確認は別項目とする。

新方式は新protocol版または独立capabilityとして明示交渉し、旧実装へ未知methodや未交渉pushを送らない。v2クライアントには現行同期方式を維持する。Packet/保存済みグラフのversion 1とは連動させず、グラフに新能力が必要なら非対応環境のprepareで明確に拒否する。receive等を黙って無視しない。既存宣言に新フィールドを足したJSONは旧deny_unknown_fieldsで拒否され得るため、旧クライアントへ返すdefinitionsも版ごとに分ける。

処理期限、受理応答の監視、プロセス生存、通信フレーム破損を分離する。新制御ループのstatus/heartbeatが応答している限り、仕事が長いだけでsession-lostにしない。受理応答が失われたときの重複起動を避けるため、開始要求の一意キーと状態照会・保持期間をB採用時に確定する。結果不明のstartRunを自動再送しない。監視不能、EOF、壊れたJSON、世代不明、上限超過では接続を閉じ、結果不明と診断する。処理期限切れによるexpiredは接続健全なまま通知できる。

[#45](https://github.com/pitapa79/DIY_IME2/issues/45)の10秒設定化は独立した短期変更であり、上記非同期方式の採用を必要としない。その設定は現行の送信済み要求の監視時間であって、ノード期限・取消・強制中断を実装したことにはならない。本Issueではその実装ファイルを編集しない。現行の4セッション・16未完了要求・1MiB要求・8MiB応答・15分未使用終了も自動変更しない。将来の長時間仕事は監視成功を利用中と数える等、アイドル条件もBの採用時に定義する。

## 10. 正式fixtureの候補

ここからは**未採用の期待値案**。既存tests/contractsへは追加せず、#46でE/R採用後に正式形式・ファイル名を確定する。#47ではFを採用した正式fixtureとNode APIだけを使い、推測で補完しない。

Data例のPacket自体は既存v1形式。診断IDやrecovery記録は制御側の概念表記でありPacketに埋め込まない。

```json
{
  "case": "upstream-error-to-original",
  "proposalOnly": true,
  "inputs": {
    "primary": {"status":"error","causes":["E1"]},
    "original": {"status":"data","packet":{"version":1,"root":{"type":"text","text":"かな😀","meta":{"reading":"かな","sourceSpan":[0,3]}}}}
  },
  "causes": [{"id":"E1","sessionEpoch":"S1","runId":41,"planVersion":3,"nodeId":"convert","port":null,"error":{"code":"conversion-failed","message":"test failure"}}],
  "config": {"onNoOutput":false},
  "expected": {
    "out": {"status":"data","packet":{"version":1,"root":{"type":"text","text":"かな😀","meta":{"reading":"かな","sourceSpan":[0,3]}}}},
    "recovery": {"causes":["E1"],"input":"primary","output":"out","reason":"error-fallback"},
    "legacySucceeded": false,
    "delivery": "recovered"
  }
}
```

| ID | 入力・操作 | 正式検査へ移す期待値案 |
| --- | --- | --- |
| E01 | 従来ノードの必須/任意入力へError | 両方Blocked。旧fixture・保存グラフの結果不変 |
| E02 | receive必須Error、他はData | 実行され元原因を受け取る。NoOutputへ変換しない |
| E03 | receive任意Errorと必須NoOutput | Skipped・全出力NoOutput、原因履歴保持、復旧記録なし |
| E04 | block Errorと必須NoOutputとreceive Error | Blocked優先、全原因と経路保持、ノード未実行 |
| E05 | 二つの上流失敗、同じ失敗の分岐も合流 | 原因IDは2件、全経路保持。宣言順で安定表示 |
| F01 | 上記JSON | meta・Unicode・原因保持、legacySucceeded=false / delivery=recovered |
| F02 | primary Data、original Error | primaryを無変更で採用。originalの原因は未選択として履歴に残る |
| F03 | primary NoOutput、original Data | 既定はNoOutput。onNoOutput=trueのみ原文Data、理由no-output-fallback |
| F04 | primary Error、original NoOutput / Error | 元Error / 原因集合の和、未復旧。新しい汎用エラーで上書きしない |
| F05 | 空Text、空Sequence、空Choice、入れ子、score、metaを持つprimary Data | すべて同一Packetで通る。入力不変 |
| F06 | originalだけにError、primary NoOutput、onNoOutput=true | original Errorを再伝搬 |
| F07 | 未接続optional、両NoOutput | 既定・明示設定ともout NoOutput、Data空文字と区別 |
| F08 | 不正設定、ja-Latn→ja、dataType/root不適合 | 計画準備で拒否。実行境界の不正Packetも成功にしない |
| R01 | Eが復旧経路とblock経路へ分岐、両方target | 一方の復旧で全体の未復旧を消さず、delivery=failed |
| R02 | Failedなし、Skippedのみ、target NoOutput | legacySucceeded=true、delivery=no-output |
| R03 | 同一入力の反復、複数インスタンス | Packet/意味は再現、診断IDはrunごとに分離、状態干渉なし |

## 11. 状態遷移と時間の検査シナリオ案

仮想時計と手動完了workerで実時間sleepを使わず検査する。時刻はすべてCoreの単調時計ms、各行は独立したrun。例の採用規則・期限値はユーザー構成の例でありCoreの既定値ではない。

| ID | 遷移・イベント | 期待値 |
| --- | --- | --- |
| T01 上流失敗→原文 | S1/p3/r41入力7 accepted(0) → convert Failed(E1,10) → 復旧ノードがoriginal Dataを選択(11) → completed | outはF01のPacket。E1保持、復旧記録あり、従来succeeded=false、delivery=recovered |
| T02 速い候補＋遅い処理 | accepted(0)、速い候補Data(20)、遅い仕事はPending。構成「速い候補があれば採用」を明示 → completed(20) | 遅い完了を待たず速い候補採用。遅い枝を不要とし取消。完了(200)は破棄。全入力待ちの通常合流ノードなら200まで待つ |
| T03 待ち続ける選択 | T02と同じ入力、構成「両方待って選択」 | 20では仮結果または待機表示、200で制御ノードが選択。Coreが速い方を暗黙採用しない |
| T04 仕事期限→復旧 | run期限1000、仕事開始10・期間100 → deadline110、まだ未完了 | 仕事expiredのErrorを復旧先へ配送、原文採用でrun completed/recovered。仕事完了111は破棄、session健全 |
| T05 run期限到達 | accepted0・期限100、Dataが未確定のまま100 | run expired。未起動中止・実行中取消要求。完了101を破棄。代替を採用したい構成はrun期限前に判断する |
| T06 完了と期限同時 | deadline100、配送境界の記録99 / 100 | 99は期限内、100は期限優先。workerの自己申告99でもCore到着100なら破棄 |
| T07 取消→遅延結果 | running → cancel受理30 → run cancelled、物理仕事は残存 → complete40 | cancel受理と物理停止を別表示。40はDataへ戻さず、残存枠は物理終了で解放。繰返しcancelも同じrunだけ |
| T08 入力更新 | r41入力7実行中、画面が入力8へ変更30 → 世代失効通知/取消 → r42入力8、r41完了40 | 画面は通知到着前でも旧結果を拒否。Coreは失効後に旧結果を拒否。planVersion・prepared・state.revision不変 |
| T09 計画更新 | p3実行中 → prepare p4候補20 → p3完了25、別ケースでactivate30 → p3完了40 | prepareだけならp3結果は稼働結果として有効（編集中との差は表示）。activate後の40は拒否。rollbackも新しい版になり過去結果を復活させない |
| T10 session更新/同じ入力 | S1切断→S2で同じplan/run/input番号を受信、または同じinputRevisionで別run | sessionEpochまたはrunId不一致で拒否。数値一致だけで採用しない |
| T11 経過時間だけの変化 | 入力更新なし、待機開始0で次回観測500を登録 | 500のタイマーで判断可能。時計変更に影響されず、状態取得は採用副作用なし |
| T12 未選択枝・共有仕事 | 二つのtargetが同じ仕事を参照、一方だけ不採用 | 共有の利用者がいる間は取消しない。全利用者不要なら取消、未起動の子は開始しない |
| T13 同期関数が取消非協調 | 取消後も処理継続、残存枠上限に達する | 結果破棄、枠を早期解放しない、新規受理busy。in-processの停止を保証しない |
| T14 長時間処理・通信故障 | 仕事が10秒超でも制御応答は健全 / EOF・不正JSON | 新契約では前者はrunning継続、後者は接続故障・結果不明。現行v2は従来タイマーで切断 |
| T15 容量・保持・再試行 | 予約上限超過、結果保持切れ、start受理の応答消失 | busyは起動なし、保持切れは明示的な取得不可、結果不明の開始を自動再送しない。重複排除の確定仕様が必要 |

競合検査では同じイベント列を再生し、採用結果・破棄理由・原因経路が一致することも確認する。画面・ケーブルに旧結果を出さない検査、実プロセスの受理/完了/取消/再接続の検査は本接続の担当へ分け、独立試作の成功だけで本体対応完了としない。

## 12. 採用後の分割と未決定点

| Issue | 着手条件・担当範囲 | 完了時も含めないもの |
| --- | --- | --- |
| [#45](https://github.com/pitapa79/DIY_IME2/issues/45) | 独立したbridge監視タイマー設定化。#44採用待ちは不要 | 非同期取消・ノード期限の実装 |
| [#46](https://github.com/pitapa79/DIY_IME2/issues/46) | E/Rと必要なFの仕様をユーザーが採用し、正式契約がmainへ反映済み。入力宣言/API、runtime、診断・集計、正式fixtureと関連検査を共通基盤担当が実装 | 復旧ノード、非同期本接続の先取り |
| [#47](https://github.com/pitapa79/DIY_IME2/issues/47) | #46統合済み、Fのkind/ファイル・型/言語・設定・正式fixture確定。独立復旧ノードと担当テスト・example | 共通API変更、標準Registry/UI登録。必要なら別の統合担当へ依存記録 |
| [#48](https://github.com/pitapa79/DIY_IME2/issues/48) | 独立exampleによる非同期試作。#44と並行可 | 製品API、既存契約、他担当の文書変更 |
| [#49](https://github.com/pitapa79/DIY_IME2/issues/49) | #44と#48の成果・判断記録を比較し、本体接続と時間制御の後続Issue案を作成 | 評価だけでA/T/Bを自動採用、本番実装開始 |

本接続の前にユーザー判断が必要な点は、入力インターフェースと宣言の版管理、復旧記録とtargets/集計の正式形式、Fの名称と設定、完了アダプター/制御ノード境界、期限の値域・無期限表記、容量の数値と設定幅、protocolの版/交渉、開始重複排除・保持期間・結果再取得、入力失効通知の形式である。上の推奨規則の変更点も採用記録で明示する。

移行順は、(1)採否と正式契約を記録、(2)同期のエラー入力を旧アダプターと共存、(3)独立復旧ノード検査、(4)別途統合判断、(5)試作評価後に非同期・時間制御の正式契約、(6)新protocol・画面・Coreを整合させて接続。どの段階でも既存demo/e2e、保存グラフ、Packet v1、v2の同期runを回帰検査する。新能力のない既存グラフへ自動的に復旧・期限・取消を設定しない。

#44はこの提案書とリンク・例・差分の検査で完了する。コード検査を実装済みの証拠に代用せず、本提案だけならRust/Editorのビルドは不要。戻し方は本書の追加をrevertするだけで、実行動作・保存グラフへの影響はない。正式採用後の基盤変更の戻し方は各実装PRで別に記録する。
