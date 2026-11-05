# 文字列置換ノード（独立実験）

> 現在はIssue #56 / PR #119でCLI・エディターの実験カテゴリへ登録し、全体検査を通過した実験実装です。正式採用は未決定です。以下の「未登録」や古い検査結果は個別PR作成時の記録として保持しています。最新の状態・限界・戻し方は[統合記録](../../../docs/pre-ime-integration-2026-10-30.md)を参照してください。

`experimental.text-replace@1` / `TextReplace`。候補一覧の「文字列置換」を現行APIで試す実装です。標準Registry・画面には未登録で、正式採用を決めるものではありません。

## 接続と設定

必須入力 `in` と出力 `out` は `text-packet@1`・Text・ja。アルゴリズムは言語に依存しませんが、初期パイプラインに合わせjaで宣言します。SequenceやChoiceを暗黙に文字列化しません。

設定は `{"from":"置換前","to":"置換後"}`。この2キーを文字列として完全に指定します。グラフで設定自体を省略した時だけこの既定値が使われ、部分設定はマージしません。空fromは拒否、空toは削除です。

一致は大小を区別するリテラル検索。正規表現・置換変数・正規化はありません。左から非重複で全件置換し、置換後の文字を再度検索しません。`aaa` の `aa → x` は `xa`、`a → aa` は1回の置換で `aa` です。結合文字と合成済み文字は区別します。

本文が変化した場合はreading/sourceSpanを推測せずmetaを省略します。本文が同一なら元Packetとmetaを保持します。入力は変更しません。空TextはData、NoOutputはNoOutputです。Engine経由では必須NoOutputのノードはSkippedになります。

## エラーと検査

不正設定はinvalid-config、入力欠落はmissing-input、不正Packetはinvalid-packet、Text以外はinvalid-root。設定を先に検査するためNoOutputでも不正設定を受理しません。グラフ準備時も設定を検査します。

`contract.json` は本実験用の期待値で、正式なtests/contractsの採用契約ではありません。実装自身を#[path]で読み込む対象テストが照合します。

~~~powershell
cargo test --locked --test nodes_text_replace
~~~

テスト内の専用RegistryへこのNodeを登録してEngineからも実行し、既定設定・複数インスタンス・NoOutput・不正設定を確認します。登録とエディター公開は統合担当の別タスクです。

## 統合担当の検査記録

2026-10-28: 作成時に未実行だった担当Rustテストを、作業フォルダー内のRust 1.90.0 GNUで実行し通過。rustfmtで担当ファイルを整形。全体test/fmt/clippyと登録/UI接続は統合Issue #56で引き続き確認する。上の作成時の未検査記録はこの記録で更新する。

対象Issue: #53
