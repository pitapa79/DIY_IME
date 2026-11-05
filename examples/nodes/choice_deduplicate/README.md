# 重複候補削除（独立試作）

> 現在はIssue #56 / PR #119でCLI・エディターの実験カテゴリへ登録し、全体検査を通過した実験実装です。正式採用は未決定です。以下の「未登録」や古い検査結果は個別PR作成時の記録として保持しています。最新の状態・限界・戻し方は[統合記録](../../../docs/pre-ime-integration-2026-10-30.md)を参照してください。

`experimental.choice-deduplicate@1` / `ChoiceDeduplicate`。
汎用ノード候補の検証用実装であり、標準Registry・エディターには未登録。

- 入出力: `in` → `out`、`ja` / `text-packet@1` / 最上位 `Choice`。
- 設定: `{}` のみ。未知キー、null、部分的な別設定は拒否する。
- 最上位の各 `ChoiceOption.value` を構造で比較し、最初のoptionを保持する。
  同じvalueの後続optionはトップscoreが高くても削除する。スコアの加算・平均はしない。
- Textの本文・meta、Sequenceの順序、下位Choiceの順序・scoreも比較に含む。
  スコアの比較は数値の等値（`0` と `-0` は同じ）。Unicodeの正規化はしない。
- 下位Choice内の重複は削除しない。保持した候補木・meta・スコアはそのまま複製する。
- 空ChoiceはDataの空Choiceとして返す。NoOutputはNoOutput。
- 不正Packetは`invalid-packet`、Text/Sequenceルートは`invalid-root`、
  不正設定は`invalid-config`、入力キー欠落は`missing-input`。

実装自身をテスト用Registryへ登録して確認する:

```powershell
cargo test --locked --test nodes_choice_deduplicate
```

[契約例](contract.json)を実装へ照合する。正式な採用済み契約ではなく、後で取捨選択するための試作契約。

比較と再構築は反復処理を使い、このノード独自の深さ上限を加えない。
候補数をn、比較する候補木の大きさをmとすると最悪O(n²m)で、巨大候補群向けの最適化ではない。
結果は独立した候補木を保持するため、その分のメモリを使う。
既存CandidateのDropや外部コードによるClone、JSON境界にはそれぞれ既存の深さ・資源上の限界がある。
本試作はそれらを変更しない。トップ以外の重複削除、意味的同義判定、重み統合、標準登録/UI接続は対象外。

戻す場合は本ノードの追加コミットをrevertする。共有Coreや既存ノードは変更していない。

## 作業引継ぎ

- 開始SHA: `2d7a26187b686c74132731d5b00c3b6dc7bb43d7`。
- `cargo test --locked --test nodes_choice_deduplicate` と対象rustfmtを試行したが、
  この実行環境ではcargo/rustfmtがPATHになく、ユーザーの既定`.cargo/bin`・
  `.rustup/toolchains`にもツールチェーンを確認できなかった。**Rustテスト・整形は未実行**。
- 検査は、fixture照合・Unicode/meta・最初のscore保持・下位構造保持・空Choice・
  NoOutput・不正設定/Packet/root・深さ128の比較再構築・自身RegistryでのEngine実行を用意している。
- 統合担当はツールチェーンを使える環境で担当テスト・rustfmt・全体ゲートを実行し、
  成功を確認してから登録・公開する。

## 統合担当の検査記録

2026-10-28: 作成時に未実行だった担当Rustテストを、作業フォルダー内のRust 1.90.0 GNUで実行し通過。rustfmtで担当ファイルを整形。全体test/fmt/clippyと登録/UI接続は統合Issue #56で引き続き確認する。上の作成時の未検査記録はこの記録で更新する。

対象Issue: #68
