# カタカナ→ひらがな（独立実験）

> 現在はIssue #56 / PR #119でCLI・エディターの実験カテゴリへ登録し、全体検査を通過した実験実装です。正式採用は未決定です。以下の「未登録」や古い検査結果は個別PR作成時の記録として保持しています。最新の状態・限界・戻し方は[統合記録](../../../docs/pre-ime-integration-2026-10-30.md)を参照してください。

IME前候補一覧の「カタカナ→ひらがな」を検査できるRust Node。
後で採否を判断するための独立実装で、標準Registryとエディターには未登録。

- kind: `experimental.katakana-hiragana@1`、struct: `KatakanaHiragana`
- 入出力: `in` / `out`、`text-packet@1` のText、言語ラベル`ja`。
- 設定: `{}`のみ。オブジェクト以外・未知キーは`invalid-config`。
- U+30A1〜U+30F6（ァ〜ヶ）、U+30FD/U+30FE（ヽ/ヾ）を、それぞれ0x60前のひらがなへ変換する。
- それ以外はそのまま。U+30F7〜U+30FA（ヷ〜ヺ）、U+31F0〜U+31FF（小書き拡張カナ）、半角カナを分解・正規化しない。結合濁点もそのまま。
- 本文が変わったときだけmetaを省略。本文が変わらなければmetaを保持する。新しい位置対応や読みを推測しない。
- 空TextはData。NoOutputはNoOutput。Text以外は`invalid-root`、不正Packetは`invalid-packet`、直接呼出しで入力キー欠落は`missing-input`。
- 必須入力なので通常のEngineではNoOutput入力のノードはSkippedになる。

[実験の期待値](contract.json)はこのノードの担当テストで読み込む。既存の正式`tests/contracts`を変更しない。

```powershell
cargo test --locked --test nodes_katakana_hiragana
```

テストは実装を直接読み込み、自身を登録したRegistryでEngine接続も確認する。
標準Registry登録・UI接続・他の文字種変換・IME操作は別の統合工程。

## 開始記録

開始SHA: `2d7a26187b686c74132731d5b00c3b6dc7bb43d7`。
専用ブランチ: `codex/katakana-hiragana-prototype`。
担当: kana_node。上記4担当ファイルのみ。共通API・登録・UI・依存変更なし。
この実行環境ではcargo/rustfmtの実体がなく、Rust検査・整形は統合担当へ引き継ぐ。
Issue作成要求を送信済み。ネットワークで失敗した場合も、この記録から再開できる。
## 統合担当の検査記録

2026-10-28: 作成時に未実行だった担当Rustテストを、作業フォルダー内のRust 1.90.0 GNUで実行し通過。rustfmtで担当ファイルを整形。全体test/fmt/clippyと登録/UI接続は統合Issue #56で引き続き確認する。上の作成時の未検査記録はこの記録で更新する。

対象Issue: #66
