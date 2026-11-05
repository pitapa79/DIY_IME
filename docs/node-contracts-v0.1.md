# 実用ノード契約 v0.1


更新（2026-10-30）: 辞書の限定案は[Issue #15](https://github.com/pitapa79/DIY_IME2/issues/15)でユーザー採用済み。以下の辞書保留記録は当初の状態。採用範囲の正本は[辞書仕様](dictionary-node-proposal-v0.1.md)、受け入れ例は[dictionary_exact.json](../tests/contracts/dictionary_exact.json)。Node登録・Host APIは#206〜#208で実装・検査した（末尾の辞書契約参照）。
状態: **最初の実装用として採用した契約。ローマ字変換・候補選択・文字列化は実装・登録済み。** 公開プラグインABIや全標準ノードの最終仕様ではない。現在動作する `demo.*` / `e2e.*` の契約を変更しない。

初回の並行開発対象は **ローマ字変換と候補選択**。現在は実行時入力・ローマ字変換・候補選択・文字列化を標準登録済み。辞書の形式・保存・読み込み・検索API・ノード境界の正式採用は未決定のまま保留し、この契約では決めない。[辞書の純粋処理実験](../examples/nodes/dictionary_exact/README.md)は未登録の別実験であり、下記の契約や期待値を変更しない。

## 共通の契約

- Rustの既存 `Node`、`Registry`、`Packet` を使う。kindは下記の名前、ノードversionは1。設定省略はdefault_config、指定時は完全な設定オブジェクトであり部分マージしない。
- 文章Packetはversion 1。Text / Sequence / Choice、任意メタデータ、有限スコアの規則は既存仕様に従う。ポートのdataTypeは `text-packet@1`。
- 入力Packetは検証する。型不一致は `invalid-root`、不正Packetは `invalid-packet`、不正設定は `invalid-config`。設定は計画準備時に拒否し、実行中の不整合はCoreErrorとして返す。
- NoOutputを受けた場合は宣言したすべての出力へNoOutputを返せること。必須入力がNoOutputの場合、通常のEngine実行ではノード自体がSkippedになる。
- 同じ入力・設定・実行モードから同じ出力を返す。ノード間・ノードインスタンス間の可変なグローバル状態を持たない。
- 旧ノードを上書きせず、新kindを追加する。実装中のモジュールは標準Registryへ登録しない。担当のテストでは個別Registryへ登録する。
- `tests/contracts/*.json` は受け入れ例。型・境界・Unicode・異常系の追加テストは各担当が用意する。契約の例を固定文字列の正解合わせで実装しない。

## ローマ字→ひらがな

| 項目 | 契約 |
| --- | --- |
| kind / struct | `text.romaji-to-hiragana@1` / `RomajiToHiragana` |
| 実装・テスト | `src/nodes/romaji/`、`tests/nodes_romaji.rs` |
| 必須入力 | `in`: Text、言語 `ja-Latn` |
| 出力 | `out`: Text、言語 `ja`。`pending`: Text、言語 `ja-Latn` |
| 既定設定 | `{"overrides":[],"unknown":"preserve"}` |
| モード | 実行時入力のcompose / finalizeを使う。実行時入力がない固定グラフではfinalize |
| 期待値 | [romaji.json](../tests/contracts/romaji.json) |

`out` は変換できた部分、`pending` は入力途中の末尾。空でもDataの空Textを返し、NoOutputとは区別する。出力Textのmetaは省略する。元の文字列との位置対応を推測して付けない。

入力は毎回**全文スナップショット**で受け取る。前回からの差分・キーイベント・内部バッファを要求しない。Backspace等は呼出側が変更後の全文を再送すればよい。

composeでは、末尾が変換規則の未完の接頭辞ならpendingへ保持する。例: `k` → out空/pending `k`、`ky` → out空/pending `ky`、`kya` → out `きゃ`/pending空。曖昧な末尾 `n` も保持する。`na` は `な`、`nn` と `n'` は `ん`。後続が母音・y以外なら `n` を `ん` として処理する。`nnya` は `んにゃ`。

finalizeでは末尾 `n` を `ん` にする。残る未完のローマ字はunknownの方針でoutへ保存するか `incomplete-romaji` とする。finalizeの成功時はpendingが空になる。

英字は大小を区別しない。pendingと未変換で保存する文字は原文の大小を保持する。数字・空白・記号・かな・その他の非ASCII英字をそのまま通す。ASCII英字の未知部分はunknown方針に従う。対応する子音の重複は促音、nnは撥音として扱い、規則のない子音の重複を勝手に促音化しない。

既定表はコードと分けた同梱データとして管理する。最低限、五十音・濁音・半濁音・wo、si/shi・ti/chi・tu/tsu・hu/fu・zi/ji、基本の拗音（k/g/s/sh/t/ch/n/h/m/r/b/p/j系）、x/l系の小書き母音・ゃゅょ・っに対応する。外来音の網羅、独自方式、かな入力、カタカナ出力の設定は後続タスク。

設定は以下の2キーだけを許可する。

- `unknown`: `preserve` または `error`。preserveは未知の原文をoutに残す。errorは確定できない部分を `unsupported-romaji`（未知列）または `incomplete-romaji`（finalize時の未完末尾）にする。
- `overrides`: `{ "from":"ka", "to":"ヵ" }` の配列。既定表の同じfromを置換・未登録のfromを追加し、長い一致を優先する。fromは小文字ASCII英字とアポストロフィからなる1〜8文字、toは1〜16コードポイント。配列内の同一from重複、空文字、未知キーはinvalid-config。上限256件。n/nn/n'の制御規則は予約し、fromに指定するとinvalid-config。

エラー時に部分変換結果を成功として返さない。ノードが失敗した場合、Engineは両出力をエラーとして扱う。

並行作業では、担当モジュール内に `Mode::{Compose, Finalize}` と純粋関数 `convert(input, mode, config)` 相当を実装して先行検査できる。Nodeアダプターが読む `RunContext.input` は[実行時入力タスク](runtime-input-v0.1.md)で追加した（[PR #6](https://github.com/pitapa79/DIY_IME2/pull/6)）。準備時の共通開始コミットには含まれないため、その基盤PRを取り込むまでは共通APIを独自に追加しない。アダプターの完成・統合は基盤に依存する。

## 候補選択

| 項目 | 契約 |
| --- | --- |
| kind / struct | `text.select-candidates@1` / `SelectCandidates` |
| 実装・テスト | `src/nodes/candidate_selection/`、`tests/nodes_candidate_selection.rs` |
| 必須入力 | `in`: Any root、言語 `ja` |
| 出力 | `out`: Any root、言語 `ja` |
| 既定設定 | `{"unscored":0.0}` |
| 期待値 | [candidate_selection.json](../tests/contracts/candidate_selection.json) |

各Choice内で最大スコアを持つ選択肢を選び、選択経路の内側にあるChoiceも処理する。同点は元の先頭を選ぶ。省略スコアの評価値は設定unscored。設定はこの1キーのみ、値は有限数。負数も有効。

外側のChoiceを選ぶ時に、内側や別のChoiceのスコアを合算・比較しない。スコアは確率として正規化しない。

Sequenceは順序・入れ子を保持し、Textの文字列・metaはそのまま保持する。選ばれなかった枝は処理結果に含めない。入力を変更しない。選択のみを担当し、文字列の結合は行わない。

- 選択経路に空Choiceがあれば、全体をNoOutputとする。
- 空SequenceはDataの空Sequenceとして残す。
- 空TextはDataの空Textとして残す。
- 選ばれなかった枝の空Choiceは結果へ影響しない。

アルゴリズムは言語に依存しないが、初期の登録ポートは日本語パイプラインに合わせてjaとする。汎用ノードで言語ラベルを保存する型変数等の仕組みは、この版では追加しない。

このノードは現在のNode APIで実装・単体検査できる。実行時入力タスクの完了を待たずに着手可能。辞書がなくても任意の候補木をテスト入力にできる。

## 文字列化

| 項目 | 契約 |
| --- | --- |
| kind / struct | `text.stringify@1` / `Stringify` |
| 実装・テスト | `src/nodes/stringify/`、`tests/nodes_stringify.rs` |
| 必須入力 | `in`: Any root、言語 `ja` |
| 出力 | `out`: Text、言語 `ja` |
| 設定 | `{}` のみ。未知キー・オブジェクト以外は `invalid-config` |
| 期待値 | [stringify.json](../tests/contracts/stringify.json) |

Textの文字列と、入れ子になったSequence内のTextを順番どおり連結する。区切り・空白・改行を追加せず、Unicode・空白・改行を原文のまま保持する。候補選択と文字列化は別の責務とし、Choiceが1つでも残っていれば `unresolved-choice`。空Choiceや選択肢が1つだけのChoiceもエラーにする。

空Text・空SequenceはDataの空Text。NoOutputはNoOutput。不正Packetは `invalid-packet` として処理前に拒否し、エラー時に部分的な連結結果を返さない。入力を変更しない。出力Textのmetaは省略し、複数Textの読みや参照区間を推測して結合しない。

Nodeは再帰呼び出しなしに走査する。JSON入出力境界の既存深さ制限は維持する。実行時入力のモード・過去の実行・他インスタンスの結果に依存しない。[専用サンプルと確認方法](../examples/nodes/stringify/README.md)を参照。

## 完了条件と統合

各ノードの正常系・NoOutput・エラー・Unicode・設定の検査を自動化する。入力Packetの不変性、同じデータでの再現性、同一Registryに複数インスタンスを置いた際の干渉がないことも確認する。

統合担当は公開するNodeを既定Registryへ追加し、設定UI・ラベル・サンプル・ノード数を前提にする既存テストを同じ統合変更で更新する。kindの登録だけを先に公開して未実装の成功を返さない。新ノードのサンプルも、辞書の新しい扱いを決定する根拠にはしない。

## 完全一致辞書（Issue #15採用 / #208接続）

kind `text.dictionary-exact@1` / `DictionaryExact`。実装はsrc/nodes/dictionary_exact、必須inはja/Text、outはja/Choice。設定は`{"path":"dictionary.tsv"}`のみ。pathは非空・4096 UTF-8 bytes以下・NULなしの文字列。パスのアクセス可否はHostが準備時に検査する。構文として正しい参照も、ルート未設定/範囲外等ならdictionary-pathで失敗する。

[辞書仕様](dictionary-node-proposal-v0.1.md)の全文一致/行順/score/meta省略/未知語・空入力/TSV/上限を採用し、[13例](../tests/contracts/dictionary_exact.json)を純粋APIと準備済みNodeへ照合する。外部ファイルは[明示Hostルート](host-resources.md)からprepare時だけ取得して全件検査し、PreparedNodeへ所有する。共通Nodeのexecuteを直接呼ぶ場合はdictionary-not-prepared。準備済みexecuteにNoOutputが来た場合はoutへNoOutput、通常Engineの必須入力欠落はSkipped。

Hostのresource-path/unavailableはdictionary-path、read/busy/deadline/platformはdictionary-read、limitはdictionary-limitへ対応させる。辞書のencoding/format/limit、不正Packet/root/input-limitは純粋APIのコードを保持する。新しい読込失敗を未知語として隠さず、準備失敗で旧計画/辞書を維持する。[専用サンプル](../examples/nodes/dictionary_exact/README.md)で候補選択/文字列化へ接続する。
