# 候補木再構築（実験）

> 現在はIssue #56 / PR #119でCLI・エディターの実験カテゴリへ登録し、全体検査を通過した実験実装です。正式採用は未決定です。以下の「未登録」や古い検査結果は個別PR作成時の記録として保持しています。最新の状態・限界・戻し方は[統合記録](../../../docs/pre-ime-integration-2026-10-30.md)を参照してください。

`experimental.packet-rebuild@1` / `PacketRebuild`。ユーザーのIME前ノード試作依頼による独立実装。正式採用は後で判断する。

- in: ja / Any、out: ja / Any。
- 完全設定の既定値: `{"mode":"wrap-sequence"}`。未知キー・欠落キー・不正型をinvalid-configで拒否。
- 最上位をSequence/Choiceで包むか、単一要素の最上位を1段外す。単一Choiceを外す時だけ外側scoreを捨てる。暗黙の平坦化・合算なし。
- Packet妥当性検査、NoOutput伝播、missing-inputを区別。空Choice/Sequenceは特記しない限りDataとして保持。
- 入力を変更せず、操作対象以外のmeta/score/構造を保持。木の複製は反復map_textを利用するが、Rustの既存CandidateのDropや外部呼出側のCloneまで無制限深度を保証するものではない。
- 下位Choiceの評価やスコアの合算を暗黙に行わない。

検査: `cargo test --locked --test nodes_packet_rebuild`。作成直後は未実行。標準登録/UI/全体ゲートは統合Issue #56。

戻し方: 対象PRをrevertし、実験kindを使うグラフを先に交換する。

対象Issue: #82

## 統合担当の検査記録

2026-10-28: 担当Rustテスト4件（複数契約例、異常系、下位score/meta保持、実装自身Registry/Engine）をRust 1.90.0 GNUで実行し通過。担当rustfmt実施。全体test/fmt/clippyと標準登録/UI接続は統合Issue #56で確認中。作成時の未検査記録はこの記録で更新する。
