# 文字列分割ノード（独立実験）

> 現在はIssue #56 / PR #119でCLI・エディターの実験カテゴリへ登録し、全体検査を通過した実験実装です。正式採用は未決定です。以下の「未登録」や古い検査結果は個別PR作成時の記録として保持しています。最新の状態・限界・戻し方は[統合記録](../../../docs/pre-ime-integration-2026-10-30.md)を参照してください。

`experimental.text-split@1` / `TextSplit`。IME前の候補一覧「文字列分割」を現行Node APIで試します。標準Registryと画面への登録・正式採用は別工程です。

## 契約

入力 `in` は必須のja/Text、出力 `out` はja/Sequence、いずれもtext-packet@1。既存の候補木を暗黙に文字列化しません。言語判定は行わず、初期パイプラインに合わせてjaと宣言しています。

設定は厳密な `{"separator":","}`。separatorは非空の文字列で、空・null・数値・未知キー・部分設定はinvalid-config。グラフのconfig自体を省略した場合だけ既定値のカンマを使います。

Rustのstr::splitに従う、左から非重複のリテラル分割です。正規表現・空白の特別扱い・Unicode正規化はありません。区切りは出力から除きます。連続する区切り・先頭・末尾の空Textを保持し、空入力も1個の空Textを持つSequenceです。区切りがなくてもText1個のSequenceです。

出力の各Textはmetaを省略し、読みや参照位置を推測しません。入力Packetは変更しません。NoOutputはNoOutputであり、空入力と区別します。Engine経由では必須NoOutputならSkippedです。

## 検査と制限

Packetを先に検証してinvalid-packet、Text以外をinvalid-root、入力欠落をmissing-inputにします。不正設定はNoOutputの場合でも拒否します。

`contract.json` は独立実験用の期待値で、正式なtests/contractsへの採用ではありません。対象テストが実装自身へ照合し、専用RegistryとEngineで既定値・独立インスタンス・NoOutput・準備時エラーも検査します。

~~~powershell
cargo test --locked --test nodes_text_split
~~~

本実装は同期処理・外部I/Oなし。標準登録・UI・基盤型の変更は統合担当へ引き継ぎます。

## 作業記録・Issue本文案

ユーザーから「取捨選択は後にしてIME前に作れる候補を実装」の依頼。親からこの単一ノードを割当済み。開始SHAは `2d7a26187b686c74132731d5b00c3b6dc7bb43d7`。所有はsrc/nodes/text_split/、tests/nodes_text_split.rs、examples/nodes/text_split/のみ。共通ファイル・正式fixture・依存・他担当Issue #44/#45/#48は変更しません。1タスク1ブランチ1worktree1PRで保存します。

完了条件は契約JSON照合、Unicode・空値・連続/末尾区切り・meta・NoOutput・不正設定/Packet/rootとEngine経由の検査、担当テストとPRによる引継ぎです。標準登録は対象外です。

## 統合担当の検査記録

2026-10-28: 作成時に未実行だった担当Rustテストを、作業フォルダー内のRust 1.90.0 GNUで実行し通過。rustfmtで担当ファイルを整形。全体test/fmt/clippyと登録/UI接続は統合Issue #56で引き続き確認する。上の作成時の未検査記録はこの記録で更新する。

対象Issue: #67
