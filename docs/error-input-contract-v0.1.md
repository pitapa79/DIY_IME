# 同期エラー入力・復旧記録 v0.1

状態: **採用**。2026-10-29（Asia/Tokyo）。ユーザーの「3まで進めて」に基づき、統合担当が[提案](execution-control-proposal-v0.1.md)のE/Rと同期Fをこの具体的な境界へ確定した。提案の保存だけを承認と扱ったものではない。
対象Issue #122、実装は[#46](https://github.com/pitapa79/DIY_IME2/issues/46)。開始main: ecbb705192512349e6bf27484f979c734261aadd。本PRのマージSHAをIssue #46の開始記録へ残す。提案A/T/B（非同期・時間制御・新通信）は未採用。Fのノード実装・登録は停止中で、別Issue #47の依頼を待つ。

## 互換性と公開境界

- 既存Node/Inputs/Outputs/OutputValue/InputPort/NodeDefinitionの契約とJSON形状を維持する。Packet v1・保存グラフv1・同期run・succeededの意味を変更しない。
- 別trait ErrorAwareNodeをRegistry::register_error_awareで登録する。definitionは通常のNodeDefinition、error_inputsはErrorを受け取る入力IDの一覧。未知・重複IDを登録前に拒否する。空一覧も許すが新境界として扱う。同じkind/versionの通常登録との重複は禁止。
- 通常のRegistry::definitionsは従来Nodeだけを返す。新境界の宣言はerror_aware_definitionsで取得する版1の別宣言（definitionとerrorInputs）。旧クライアントへ新能力を通常宣言に偽装して返さない。
- v2 Sessionのvalidate/prepareは、新境界を含む計画をunsupported-capabilityで拒否し、計画・token・revisionを変えない。新ノードは標準Registryへ追加しない。エディター接続は後続の版交渉後に行う。

## 入力解決と失敗

ErrorAwareValueはData(Arc<Packet>)/NoOutput/Error(ErrorSet)の三値。Pendingは作らない。通常Nodeには従来のData/NoOutputだけを渡す。

1. 全入力を宣言順で解決。未接続の任意入力はNoOutput。必須未接続はprepare拒否。
2. receiveでない入力にErrorがあれば、必須/任意を問わずBlocked。receive側を含む全原因を保持。旧診断は従来どおり最初の上流診断。
3. block Errorがなく必須NoOutputならSkipped、全出力NoOutput。観測したErrorは履歴に保持し、復旧扱いしない。
4. それ以外は実行。receive必須Errorも解決済みとして渡す。NoOutputを空Text/空Choiceに読み替えない。
5. 新境界でも設定、Packet、全出力の有無、root、接続時のdataType/言語を検査する。panic/不正出力はFailed。エラー時に部分出力を返さない。

ErrorSetは非空の不透明な原因集合。新しい原因はEngineだけが発行し、ノードは受け取った集合のclone/mergeだけを返せる。各原因は元DiagnosticとCauseId(runId/planVersion/nodeId)を持つ。同期runで各nodeの新規失敗は最大1件なのでこの組で一意。runIdはプロセス内で一意、枯渇時は拒否。これは永続・分散セッションIDではなく、将来の輸送ではsessionEpoch等を追加する。
原因と配送経路は分離する。経路は接続端点の有向辺の重複なし集合。分岐・合流で原因を増殖させず、入力宣言順の初出、同一原因内も初出順で安定保持する。元port不明のnullを推測しない。Error出力は現在runで受け取った原因・経路の部分集合だけを認め、過去runの集合や捏造をinvalid-error-propagationとする。

## 復旧記録と成功判定

ErrorAwareExecutionはoutputs、recoveries、任意のdependenciesを返す。RecoveryClaimはinput/output/causes。causesは受け取ったError入力の非空・重複なしのCauseId、outputはDataでなければならない。Coreが検査しRecoveryRecordへnodeIdを付ける。未知入力・原因・出力、NoOutput/Errorへの復旧宣言、同じinput/outputの重複宣言はinvalid-recovery。元Failed/Blocked/Diagnosticは消さない。復旧を申告しないDataも許すがrecoveredとは呼ばない。

出力の復旧由来は依存入力から伝搬する。通常Nodeは全Data入力に依存するものとして保守的に集計する。新境界はdependenciesに出力ごとの採用したData入力IDを列挙できる。省略した出力は全Data入力に依存。未知・重複・Data以外の入力や出力への指定をinvalid-dependenciesとする。これは値の等価性や外部副作用を検証するものではなく、作者の依存宣言である。未選択Errorの復旧申告を推測しない。

Engine::run_with_targets(targets,input?)は存在する出力端点の非空・重複なし集合を実行前に検査する。不正targets/RuntimeInputではrun ID、last_run、計画状態を変えない。
戻り値TargetedRunReportには通常RunReportとtargets/deliveryを分けて持つ。targetが一つでもErrorならfailed、ErrorなしでNoOutputがあればno-output、全Dataで復旧由来があればrecovered、なければcomplete。空Text/Sequence/ChoiceもData。選択外の枝の失敗でdeliveryを変えないが、従来succeededはfalseのまま。
通常RunReport::to_jsonは旧形状を維持。TargetedRunReport::to_jsonだけにexecutionControlVersion:1/targets/delivery/causes/recoveriesを追加する。新宣言・集計をv2 HTTPや既存UIへ送らないためUI変更は不要。新しい診断表示は後続通信/UIタスクで扱う。

## 復旧ノードFの引継ぎ契約（未実装）

kind text.recover-original@1 / RecoverOriginal、src/nodes/recovery/、tests/nodes_recovery.rs、examples/nodes/recovery/。primary/originalはともに任意のreceive入力、outを一つ。すべてtext-packet@1、Any root、言語ja。設定は完全なオブジェクト {"onNoOutput":false} のみ（bool、未知キー拒否）。正式受け入れ例は#46でtests/contracts/recovery.jsonへ追加する。

| primary | original | 設定 | out・記録 |
|---|---|---|---|
| Data(P) | 任意 | 任意 | 同一Arc P、originalは非依存、復旧なし |
| NoOutput | 任意 | false | NoOutput、復旧なし |
| Error(E) | Data(O) | 任意 | 同一Arc O、Eの全原因を復旧記録。依存はoriginal |
| Error(E) | NoOutput | 任意 | Eを再伝搬 |
| Error(E) | Error(F) | 任意 | E/Fをmergeし再伝搬 |
| NoOutput | Data(O) | true | 同一Arc O、エラー復旧記録なし、依存はoriginal |
| NoOutput | NoOutput | true | NoOutput |
| NoOutput | Error(F) | true | Fを再伝搬 |

meta、score、入れ子、Unicodeは変更しない。空ChoiceもData。RuntimeInputを原文と推測しない。言語ja-Latnをjaへ偽装しない。ノード意味はEngineへ追加しない。

## 検査・戻し方

#46で提案E01–E05/F01–F08/R01–R03を基盤テスト用のテストダブルに対して確認する。Fの製品実装を完成扱いしない。正常Data/NoOutput/Error、必須/任意、複数原因・経路、再伝搬、偽造/旧run、復旧記録拒否、選択外失敗、複数targets、言語/型、v2拒否・状態不変を含める。既存fixtureは弱めない。
実装の統合ゲートはcargo test --locked、fmt check、clippy全target -D warnings、CLI build、editor test/build。辞書・非同期・OS登録・依存更新は対象外。
本書だけならrevertで戻せる。実装は新境界を使う計画を退避してから#46をrevert。旧Nodeとv2は引き続き利用できる。正式採用後の改善は別Issueに分ける。
