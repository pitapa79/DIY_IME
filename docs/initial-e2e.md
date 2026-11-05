# Issue #1：最初の文章変換を確認する

[Issue #1](https://github.com/pitapa79/DIY_IME2/issues/1) の初期 E2E 用に、固定規則と小辞書の7ノードを実装した。ノード API の正式採用や一般文章の変換精度を決定するものではない。キーボード入力・Windows TSF は対象外。

**辞書の扱い:** このE2Eではノードに組み込まれた小辞書を使う。後から外部TSVやSKK等を扱う場合の設計候補は[辞書の取り扱い・拡張方針](dictionary-policy.md)を参照。新文書は既存E2Eの実装・結果を変更しない。

## 画面で動かす

1. `scripts/start-editor.ps1` で起動し、サンプルから「Issue #1：降水確率」を読み込む。初回起動時もこのサンプルになる。保存済みの編集は自動復元する。
   Input のノード内にあるテキスト欄から固定文字列を直接編集できる。この欄の変更は即時にグラフへ反映され、右側の設定欄と連動する。
2. 「計画を準備」→「切替」→「実行」を押す。
3. 出力 `output.out` で **本日の降水確率は30%です** を確認する。
4. 出力ポートを切り替えると各段階の Packet と候補木を確認できる。
5. 「欠落入力：kaku なし」を読み込み、再び準備・切替・実行すると **本日の降水率は30%です** になる。欠けた `kaku` を補わない。

旧デモのノードはライブラリから外したが、保存済み `demo.*` の JSON は引き続き読み込み・編集・実行できる。

## 7段階とデータ

| ID / 種別 | 処理 | 出力の形 |
| --- | --- | --- |
| input / e2e.input | `mode: fixed`、設定 `text` をそのまま出力。上限4096コードポイント | Text、ポート言語 ja-Latn |
| hiragana / e2e.romaji-to-hiragana | `honjitunokousuikakurituha30%desu` → `ほんじつのこうすいかくりつは30%です` | Text、以降のポート言語 ja |
| segments / e2e.segment | 小さな語リストの最長一致で2種類の分割を作る | Choice → Sequence → Text |
| kanji / e2e.kana-kanji | `tree::map_text` で読みを小辞書の複数表記へ置換。外側の構造・スコアを保持 | Choice → Sequence → Choice → Text |
| select / e2e.select-candidates | 各 Choice 内で最大スコアを選ぶ。Sequence と Text のメタデータを保持 | Sequence → Text |
| result / e2e.stringify | 選択済みの Text を Sequence の順で結合。未選択 Choice はエラー | Text |
| output / e2e.output | stderr に最終文・ノードID・計画版・実行IDをログ出力。画面用にも同じ Packet を返す | Text |

正のサンプルの分割は次の2候補。スコアは同じ Choice 内の順位であり、確率ではない。

- 85: `ほんじつ / の / こうすいかくりつ / は / 30% / です`
- 15: `ほんじつ / の / こうすい / かくりつ / は / 30% / です`

辞書は `ほんじつ → 本日`、`こうすいかくりつ → 降水確率 / 降水確立`、`こうすい → 降水`、`かくりつ → 確率 / 確立`、`りつ → 率` を持つ。各組の先頭を100、他候補を10または20にする。助詞・数値・記号・未知語は維持する。

分割した Text の `reading` はひらがな原文、`sourceSpan` は**ひらがな化直後の文字列のコードポイント位置**を指す。ローマ字原文のバイト位置ではない。辞書で表記が短くなっても区間を保持する。文字列化後は複数片のメタデータを合成せず省略する。

## ノード側の方針と制限

- ローマ字は固定表の最長一致。訓令式の `ti / tu / si` と `chi / tsu / shi` などに対応し、英字の大小を区別しない。促音、撥音、少数の拗音・小書きも扱う。未対応の英字列は `unsupported-romaji` として失敗し、後続は上流エラーになる。かな・数字・記号・非英字はそのまま通す。汎用 IME の変換表を網羅する実装ではない。
- 分割候補は複合語を残す案と分ける案。入力によって両案が一致するときは重複させず1候補。未知部分は1コードポイントずつ、数字列と任意の末尾 `%` はひとかたまり。正のサンプルでは必ず2候補を生成する。
- 候補選択は外側の Choice を選んでから選択経路内の Choice を処理する。別の Choice のスコアを加算・比較しない。同点は先頭、スコア省略時は設定 `unscored`（既定0）。選択経路に空 Choice があれば NoOutput。空 Sequence は維持され、文字列化すると空文字になる。
- 不正な設定は計画準備時に拒否。不正な Packet、未対応ローマ字、未選択 Choice は診断付きエラー。未知語をかなのまま維持するのは辞書ノードの方針であり、失敗ではない。
- 各ノードは状態を持たず、同じ入力・設定から同じ候補木と文章を返す。実行IDは実行ごとに変わる。

## CLI と回帰テスト

```powershell
cargo run --bin diy-ime2 -- run examples/graphs/weather.json
cargo run --bin diy-ime2 -- run examples/graphs/weather-negative.json
cargo test --test weather_pipeline
```

CLI の stdout は全ノードの結果 JSON、stderr は Output のログ。常駐セッションでも stdout の JSON Lines と混ぜない。実装は `src/builtins/initial_e2e.rs`、サンプルは `examples/graphs/weather*.json`。

回帰テストは全7段階、2つの分割案、入れ子構造、メタデータ、助詞・数値・記号、局所スコア・同点・省略、選択と文字列化の分離、空候補、不正データ、失敗の伝播、再現性、欠落入力、CLI のログを確認する。エディター側でもサンプルの実コア実行、7ノードの一覧、固定文字列の設定とサンプル切替を検証する。
