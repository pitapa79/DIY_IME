# スコア正規化（実験）

> 現在はIssue #56 / PR #119でCLI・エディターの実験カテゴリへ登録し、全体検査を通過した実験実装です。正式採用は未決定です。以下の「未登録」や古い検査結果は個別PR作成時の記録として保持しています。最新の状態・限界・戻し方は[統合記録](../../../docs/pre-ime-integration-2026-10-30.md)を参照してください。

`experimental.score-normalize@1` / `ScoreNormalize`。ユーザーのIME前ノード試作依頼による独立実装。正式採用は後で判断する。

- in: ja / Choice、out: ja / Choice。
- 完全設定の既定値: `{"method":"softmax","unscored":0}`。未知キー・欠落キー・不正型をinvalid-configで拒否。
- 最上位Choice内だけsoftmax/min-max。min-max全同値は0、未設定はunscoredで補完。確率の正しさやChoice間比較を保証しない。
- Packet妥当性検査、NoOutput伝播、missing-inputを区別。空Choice/Sequenceは特記しない限りDataとして保持。
- 入力を変更せず、操作対象以外のmeta/score/構造を保持。木の複製は反復map_textを利用するが、Rustの既存CandidateのDropや外部呼出側のCloneまで無制限深度を保証するものではない。
- 下位Choiceの評価やスコアの合算を暗黙に行わない。

検査: `cargo test --locked --test nodes_score_normalize`。作成直後は未実行。標準登録/UI/全体ゲートは統合Issue #56。

戻し方: 対象PRをrevertし、実験kindを使うグラフを先に交換する。

対象Issue: #77

## 統合担当の検査記録

2026-10-28: 担当Rustテスト4件（複数契約例、異常系、下位score/meta保持、実装自身Registry/Engine）をRust 1.90.0 GNUで実行し通過。担当rustfmt実施。全体test/fmt/clippyと標準登録/UI接続は統合Issue #56で確認中。作成時の未検査記録はこの記録で更新する。

## min-maxの数値境界

独立レビューで近接する巨大有限値の桁落ちを確認。max-minが有限なら差分から比率を求め、差分がオーバーフローする場合だけscale式へ切り替える。正負の近接巨大値、±MAX、最小サブノーマル値を回帰検査する。

2026-10-28: 独立レビュー指摘の修正後の担当6テストが通過。min-max近接巨大値/±MAX/サブノーマル値、softmaxの非同点と極端値の回帰を含む。全体ゲートは統合Issue #56で確認する。
