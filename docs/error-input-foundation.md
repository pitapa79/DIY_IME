# 同期エラー入力基盤の利用と引継ぎ

状態: Issue #46 の基盤実装。正本は[同期エラー入力契約](error-input-contract-v0.1.md)。復旧ノードを標準登録した変更ではない。

## 利用する境界

`error_input::ErrorAwareNode` を実装し、`node::Registry::register_error_aware` で明示登録する。通常の `Node::execute`、`Inputs`、`OutputValue` に新variantを足す必要はない。

- `error_inputs()` が列挙する入力だけが Error を受け取る。
- 入力と出力は `ErrorAwareValue::{Data,NoOutput,Error}`。ErrorSet は非空の不透明な原因集合で、元入力からclone/mergeする。
- `ErrorAwareExecution::new(outputs)` は復旧申告なし。復旧を行った作者は入力・出力・原因IDを `RecoveryClaim` で申告する。
- `dependencies` の省略は全Data入力への保守的な依存。選択ノードは採用入力を出力ごとに明示する。検証できるのは申告の整合性であり、作者が実際に値をどう計算したかではない。
- `error_aware_definitions()` は新境界用の別宣言。通常の `definitions()` と通信v2は旧ノードだけを公開する。

必要な終端を明示した実行は `Engine::run_with_targets(&[Endpoint::new("node", "out")], input)`。inputは `Option<&RuntimeInput>`。targetsを検証してから実行し、`TargetedRunReport` に `report`（従来RunReport）、`targets`、`delivery` を返す。
`report.to_json()` は旧形状、`TargetedRunReport::to_json()` は版1の対象出力付き集計。後者は通信v2の新しい応答ではない。

## 診断の読み方

`succeeded` は履歴中にFailed/Blockedがあればfalseのまま。必要なtargetが復旧できた場合は別の `delivery: recovered` が得られる。元の原因・配送辺・復旧申告は詳細JSONの `causes` / `recoveries` に残る。選択外の枝の失敗も履歴から消さない。
NoOutputは成功した空Text/Sequence/Choiceと区別する。ErrorをNoOutputに読み替えない。必須NoOutputでSkippedになったノードはエラーを復旧した扱いにしない。
原因IDはプロセス内runId/planVersion/nodeId。永続・分散IDではなく、後続通信で必要となるsessionEpochなどは未実装。

## 検査と未実装範囲

[基盤テスト](../tests/error_inputs.rs) はテストダブルを実Registry/Engineへ登録して実行する。[19件の正式受け入れ例](../tests/contracts/recovery.json) は同じ境界を検査し、後続 #47 の製品実装が満たす正本となる。現在、製品ノード `text.recover-original`、そのモジュール、標準Registry、エディターのパレットには追加していない。
テストは複数原因/分岐経路、必須・任意入力、復旧申告の整合、旧runの原因拒否、出力検証、元PacketのArc保持、異なるEngineと反復実行、targetsの事前拒否、v2の宣言非公開とprepare/validate拒否を含む。

通信v2の `Session` は、新境界の計画を `unsupported-capability` で拒否する。既存エディターへ新レポートを送らないため、その診断表示を変更していない。新通信と画面への接続、非同期完了、期限、取消は #49 の計画後の別タスク。辞書、OS登録、外部プラグインABIもこの変更に含めない。

戻す場合は新境界を使うRust側計画を退避してから #46 のPRをrevertする。既存Node・Packet・保存グラフ・通信v2は移行不要。
