# ローマ字変換ノードの例

`text.romaji-to-hiragana@1` の例。契約は [node-contracts-v0.1.md](../../../docs/node-contracts-v0.1.md)。

- `config-examples.json`: 設定の例（`default` / `strict-unknown` / `with-override`）。設定は `overrides` と `unknown` の2キーが必須。
- `fixed-graph.json`: 固定入力 `kin'en` を変換する最小グラフ。実行時入力がないためfinalizeで動き、`ん` と `n'` の扱いを確認できる。

標準Registryへ登録済み。固定グラフは次のコマンドで実行でき、`romaji.out` に `きんえん` を出力する。

```powershell
cargo run --locked --bin diy-ime2 -- run examples/nodes/romaji/fixed-graph.json
```

実行時入力の例は [runtime-romaji.json](../../graphs/runtime-romaji.json)。エディターのサンプル「実用ローマ字変換」を選び、「計画を準備」→「切替」した後、Inputへ全文を入力して実行する。

| 入力 | モード | romaji.out | romaji.pending |
| --- | --- | --- | --- |
| k | compose（入力途中） | 空文字 | k |
| ky | compose（入力途中） | 空文字 | ky |
| kya | compose（入力途中） | きゃ | 空文字 |
| n | compose（入力途中） | 空文字 | n |
| n | finalize（末尾を確定） | ん | 空文字 |

結果欄の「表示する出力」で `romaji.out` と `romaji.pending` を切り替えられる。文章・モードを変えても計画版は変わらない。Backspaceは変更後の全文から再変換する。固定グラフに実行時入力がなければfinalizeを使う。

設定パネルで未知のローマ字を残すかエラーにするか選び、追加規則は設定JSONのoverridesで指定する。設定変更は計画の準備・切替が必要。辞書・かな漢字変換・Windows IME接続は含まない。

`tests/nodes_romaji.rs` は契約全22例をNodeへ照合し、実行時入力を使うEngine、固定グラフ、Unicode・設定・不変性・反復・複数インスタンスを検査する。
