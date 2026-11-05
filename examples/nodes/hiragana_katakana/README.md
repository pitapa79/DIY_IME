# ひらがな→カタカナ（独立実験）

> 現在はIssue #56 / PR #119でCLI・エディターの実験カテゴリへ登録し、全体検査を通過した実験実装です。正式採用は未決定です。以下の「未登録」や古い検査結果は個別PR作成時の記録として保持しています。最新の状態・限界・戻し方は[統合記録](../../../docs/pre-ime-integration-2026-10-30.md)を参照してください。

Issue #52。候補一覧の「ひらがな→カタカナ」を検査できるRust Node。
後で採否を判断するための独立実装で、標準Registryとエディターには未登録。

- kind: `experimental.hiragana-katakana@1`、struct: `HiraganaKatakana`
- 入出力: `in` / `out`、`text-packet@1` のText、言語ラベル`ja`。
- 設定: `{}`のみ。オブジェクト以外・未知キーは`invalid-config`。
- U+3041〜U+3096（ぁ〜ゖ）、U+309D/U+309E（ゝ/ゞ）を、それぞれ0x60先の全角カタカナへ変換する。
- それ以外（結合濁点、長音、半角カナ、漢字、英数字、補助平面文字等）はそのまま。Unicode正規化、合成・分解、半角変換はしない。
- 本文が変わったときだけmetaを省略。本文が変わらなければmetaを保持する。新しい位置対応や読みを推測しない。
- 空TextはData。NoOutputはNoOutput。Text以外は`invalid-root`、不正Packetは`invalid-packet`、直接呼出しで入力キー欠落は`missing-input`。
- 必須入力なので通常のEngineではNoOutput入力のノードはSkippedになる。

[実験の期待値](contract.json)はこのノードの担当テストで読み込む。既存の正式`tests/contracts`を変更しない。

```powershell
cargo test --locked --test nodes_hiragana_katakana
```

テストは実装を直接読み込み、自身を登録したRegistryでEngine接続も確認する。
標準Registry登録・UI接続・他の文字種変換・IME操作は別の統合工程。
## 統合担当の検査記録

2026-10-28: 作成時に未実行だった担当Rustテストを、作業フォルダー内のRust 1.90.0 GNUで実行し通過。rustfmtで担当ファイルを整形。全体test/fmt/clippyと登録/UI接続は統合Issue #56で引き続き確認する。上の作成時の未検査記録はこの記録で更新する。

対象Issue: #52
