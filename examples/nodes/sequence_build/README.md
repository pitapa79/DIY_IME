# Sequence生成ノード（独立実験）

> 現在はIssue #56 / PR #119でCLI・エディターの実験カテゴリへ登録し、全体検査を通過した実験実装です。正式採用は未決定です。以下の「未登録」や古い検査結果は個別PR作成時の記録として保持しています。最新の状態・限界・戻し方は[統合記録](../../../docs/pre-ime-integration-2026-10-30.md)を参照してください。

`experimental.sequence-build@1` / `SequenceBuild`。IME前の候補「Sequence生成」の独立実装で、正式採用・標準Registry・画面への登録は別工程です。担当[Issue #115](https://github.com/pitapa79/DIY_IME2/issues/115)、統合[Issue #56](https://github.com/pitapa79/DIY_IME2/issues/56)。

## 接続と設定

任意入力 `a`、`b` はtext-packet@1・ja・Any root、出力 `out` はja・Sequenceです。言語判定はせず初期接続に合わせjaを宣言します。

設定は完全な `{"items":[]}`。itemsにText / Sequence / Choiceの候補を配列で指定します。グラフのconfig省略時だけこの既定値を使います。未知/欠落キー、null、型不一致、候補の未知フィールド・不正score/metaはinvalid-configです。

出力Sequenceのitemsへ、設定のitems → aのData root → bのData rootの順で追加します。a/bが既にSequenceでも1要素として入れ、暗黙にflattenしません。下位の構造・順序・score・metaをそのまま保持します。設定と入力は変更しません。

任意入力の欠落とNoOutputは省略します。空Text・空Sequence・空ChoiceはDataとして残します。設定も入力も無ければDataの空Sequenceを返します。Engineでも任意NoOutputだけを理由にSkippedにはなりません。

## 検査と深いデータの制限

設定はCandidateとしてデシリアライズし、Packetの妥当性規則で検査します。入力Dataはコピー前に両方を検査し、不正Packetはinvalid-packetとして拒否します。rootはAnyなのでChoice/Sequenceを拒否しません。設定や入力のエラーから部分結果は返しません。

入力のコピーは共有の反復 `tree::map_text` を使います。4000段の入力を1要素として包み、深さ4001・末端文字とmetaを保持するテストがあります。テストの後始末も反復処理です。

**本体Candidateの通常のDrop・Clone・比較やJSON読込の既存制限を解消する実装ではありません。** 深い生データを通常の再帰的Drop等へ渡す安全性は保証しません。設定JSONにも既存の読込境界を使い、無制限の深さを保証しません。

~~~powershell
cargo test --locked --test nodes_sequence_build
~~~

contract.jsonは本実験用の期待値で、正式tests/contractsの採用契約ではありません。対象テストが#[path]で実装自身を読み、専用Registry/Engineで2入力接続、既定値、インスタンス分離、任意入力の省略も検査します。

## 戻し方

本実装コミットをrevertすると専用module・対象テスト・examplesを戻せます。共通ファイル・依存・登録は変更していません。統合後に戻す場合は先に別の登録変更を戻す順序を確認してください。
