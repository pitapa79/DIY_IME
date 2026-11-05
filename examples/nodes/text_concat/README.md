# 文字列連結ノード（独立実験）

> 現在はIssue #56 / PR #119でCLI・エディターの実験カテゴリへ登録し、全体検査を通過した実験実装です。正式採用は未決定です。以下の「未登録」や古い検査結果は個別PR作成時の記録として保持しています。最新の状態・限界・戻し方は[統合記録](../../../docs/pre-ime-integration-2026-10-30.md)を参照してください。

`experimental.text-concat@1` / `TextConcat`。IME前の汎用ノード候補「文字列連結」の独立実装です。正式採用・標準Registry・画面への登録を行うものではありません。統合は[Issue #56](https://github.com/pitapa79/DIY_IME2/issues/56)で別途扱います。

## 接続と処理

必須入力 `a`、`b` と出力 `out` は `text-packet@1`・Text・jaです。言語判定は行わず、初期パイプラインに合わせてjaを宣言しています。候補木の暗黙の選択・文字列化はしません。

設定は厳密な `{"separator":""}`。separatorは空を含む任意の文字列です。グラフでconfig自体を省略した場合だけ既定の空文字を使い、明示した `{}` や部分設定は受理しません。未知キー、null、文字列以外もinvalid-configです。

処理順は `a + separator + b`。入力が空でも区切り文字は挿入します。Unicode正規化、置換変数、エスケープの解釈は行いません。空の結果はDataの空Textです。

出力metaは常に省略します。複数入力のreading/sourceSpanを推測して統合しません。入力Packetは変更しません。

## NoOutputとエラー

設定と必須入力キーを先に検査し、与えられたDataのPacket妥当性・Text rootを検査します。有効な入力のいずれかがNoOutputならoutはNoOutputです。NoOutputを空文字へ変換せず、不正なもう片方のDataをNoOutputで隠しません。

キー欠落はmissing-input、不正Packetはinvalid-packet、Text以外はinvalid-rootです。Engine経由では必須入力がNoOutputのノードはSkippedになります。上流エラーの復旧や非同期処理は対象外です。

## 確認方法

~~~powershell
cargo test --locked --test nodes_text_concat
~~~

`contract.json` はこの実験の期待値で、正式なtests/contractsの契約ではありません。対象テストが#[path]で実装自身を読み、専用Registry/Engineで既定設定・3インスタンスの独立性・NoOutput・準備時エラーも確認します。登録されていないため標準CLIでこのkindを含むグラフを実行するには、別途統合が必要です。

## 制限と戻し方

同期のメモリ内処理のみ。OS接続、外部I/O、新しい基盤データ型、標準登録は含みません。実行時Inputの4096コードポイント制限を、このNode固有の入力上限として追加していません。

このIssueの実装コミットをrevertすると専用module・対象テスト・サンプルが戻ります。共通ファイル、依存、既存ノード、保存済みグラフは変更していません。親の統合後に戻す場合は、先に別の登録変更を戻す順序を確認してください。

担当Issue: [#110](https://github.com/pitapa79/DIY_IME2/issues/110)。
