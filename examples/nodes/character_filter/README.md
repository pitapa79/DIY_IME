# 文字種フィルター（実験）

> 現在はIssue #56 / PR #119でCLI・エディターの実験カテゴリへ登録し、全体検査を通過した実験実装です。正式採用は未決定です。以下の「未登録」や古い検査結果は個別PR作成時の記録として保持しています。最新の状態・限界・戻し方は[統合記録](../../../docs/pre-ime-integration-2026-10-30.md)を参照してください。

`experimental.character-filter@1` / `CharacterFilter`。ユーザーの「IME前候補を全部試作」依頼に基づく独立実装。正式採用・標準登録は統合担当の別工程。

- 入力 `in` / 出力 `out`: ja / Text。
- 完全設定の既定値: `{"class":"ascii-digits","keep":true}`。未知キー・部分設定・不正型はinvalid-config。
- ASCII数字/英字/英数字とUnicode空白のみ。Unicode一般カテゴリ全般や言語判定ではない。keep=falseは該当文字を除去。
- Packet検証後に処理する。NoOutputはNoOutput、空文字はData。入力欠落はmissing-input、Text以外はinvalid-root。
- 本文が変わった時は入力metaを省略し、変わらない時はPacket/metaを保持する。入力を変更しない。
- Unicode正規化は暗黙に行わない。UTF-16単位や書記素単位へ読み替えない。

検査: `cargo test --locked --test nodes_character_filter`。作成直後は未実行。正式fixtureを弱めず、このディレクトリーの契約例を実装に照合する。

戻し方: 対象PRをrevertする。実験kindを使用したグラフは、別ノードへ交換してから実装を除去する。

対象Issue: #64

## 統合担当の検査記録

2026-10-28: 作成時に未実行だった担当Rustテストを、作業フォルダー内のRust 1.90.0 GNUで実行し通過。rustfmtで担当ファイルを整形。全体test/fmt/clippyと登録/UI接続は統合Issue #56で引き続き確認する。上の作成時の未検査記録はこの記録で更新する。

対象Issue: #64
