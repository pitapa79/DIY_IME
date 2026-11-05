# 完全一致辞書ノード

状態: **Issue #15で採用した初版を#208で実装・登録。** 本体の共通辞書形式や外部辞書の採用ではない。入力の全文が一致した場合だけ表記候補を出し、未知語・空入力は原文を1候補として残す。

## 試す

このフォルダーの[dictionary.tsv](dictionary.tsv)は本リポジトリ用に作った固定テストデータ。外部辞書の取得・転載・同梱は行っていない。

リポジトリのルートからPowerShellで起動する。

```powershell
$env:DIY_IME_RESOURCE_ROOT = (Resolve-Path .\examples\nodes\dictionary_exact).Path
.\scripts\start-editor.ps1
```

画面のサンプルで「完全一致辞書（要フォルダー指定）」を選び、「計画を準備」→「切替」。inputノードに`kousui`を入力し実行すると、dictionary.outに降水90・香水30・洪水90のChoice、result.outに「降水」が出る。同点は辞書で先に書かれた候補を選ぶ。入力は保存グラフや辞書へ保存されない。

- `honjitudesu` →「ほんじつです」（全文一致しないため、そのまま）。
- `30%` →「30%」、空入力 → 空文字。どちらも成功したData。
- composeの未完ローマ字はhiragana.pendingに残る。辞書がpendingを結合・確定することはない。
- 同じサンプルは、明示policyで起動した「非同期実行」でも使える。v3の起動条件は[エディター説明](../../../editor/README.md)に従う。

固定入力のCLI例もある。ルート設定後に実行する。

```powershell
cargo run --locked --bin diy-ime2 -- run examples/graphs/dictionary-fixed.json
```

dictionary.outは候補群、select.outは選ばれたText、result.outは「降水」。ルート未設定はdictionary-pathで準備失敗となり、通常の未登録語とは区別される。

## 設定と保持

kindは`text.dictionary-exact@1`、必須inはja/Text、outはja/Choice。設定は`{"path":"dictionary.tsv"}`の1項目だけ。pathは起動時に許可したフォルダー基準の相対名で、区切りは`/`。設定に辞書本体・絶対パス・入力文を埋め込まない。[Hostの取得境界](../../../docs/host-resources.md)により、`..`/UNC/URL/ADS/deviceとreparse point等は拒否する。

辞書はprepare時に全件検査して計画へ固定する。ファイルを編集・削除しても現在の計画は保持済みの辞書を使う。再読込は新しい「計画を準備」→「切替」で行う。読込失敗時は旧計画を維持し、「復帰」は前の計画と辞書へ戻す。ファイル監視・自動更新は行わない。

UTF-8（先頭BOM1個可）、LF/CRLF、3列reading/surface/weight。文字列を正規化・trim・部分一致しない。scoreは有限f64の優先度で、候補をファイル行順で返す。負数/ゼロ可。同reading/surfaceの重複は拒否する。出力Textのmetaは既知語/未知語とも省略し、入力は変更しない。詳細は[採用仕様](../../../docs/dictionary-node-proposal-v0.1.md)。

|Node固有の上限|値|
|---|---|
|ファイル全体|1 MiB|
|1行（改行を除く）|64 KiB|
|データ行|10,000|
|同じreadingの候補|64|
|reading/surface/検索入力|各4096 Unicodeコードポイント|

超過は切り捨てずエラー。dictionary-path/read/encoding/format/limit/input-limitを区別する。純粋APIのExactDictionary::from_tsv_bytes / lookup_packet / lookup_outputも利用できる。Nodeを直接executeしても辞書は読み込まずdictionary-not-preparedを返す。計画のprepareを通す。

## 検査と範囲

[採用13例](../../../tests/contracts/dictionary_exact.json)を純粋APIと準備済みNodeの両方へ照合する。専用検査はUTF-8/形式/全上限/Unicode/未知語/空/NoOutput/入力不変/複数インスタンス/計画切替/復帰/失敗/実CLI接続を含む。現行Wasm版の実行・権限・辞書取得の検証は[辞書ブローカーの検証](../../../docs/security/wasm-dictionary-broker.md)を参照。

```powershell
cargo test --locked --test nodes_dictionary_exact
```

文章の分割・部分一致・文脈評価・学習・ユーザー辞書編集・外部辞書の入手/同梱は含まない。これはTSF接続や実IMEの完成ではなく、全文一致の辞書部品と実行基盤への接続である。

![辞書ノードと非同期の確定結果](../../../docs/images/dictionary-preview.jpg)
