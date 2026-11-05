# 診断・保存のcanary監査（#248）

開始main: fa899cbba3decfbca080cfd93973321a056d791b。
実秘密・認証情報・外部通信を使わず、実CLI/実Node/bridge/Editorへ偽canaryを置いて現状を確認した。
監査テストの成功は「秘密を隠せる」という意味ではなく、露出の現状と経路分離を再現した証拠である。

| ID | 経路・観測 | 現在の対応と限界 |
| --- | --- | --- |
| D01 | Graph固定設定はAppの下書きへ自動保存。実行時入力の保存OFFとは独立。 | 「実行時入力をこのブラウザーに保存」へラベルを具体化し、説明画面でも固定文字列/設定の下書き保存を明示。保存仕様は変えない。 |
| D02 | 実CLI validate-graphのunknown-kindでNode ID/kindがstderrへ出る。 | 実CLI canaryで観測。次の別CLIへ混入しない。Graph識別子に秘密を入れれば診断に現れ得る。 |
| D03 | 明示的に配置した互換e2e.outputは本文を結果とstderrへ出す。 | 実CLIの入力→かな→Outputで同じcanaryを両方に確認。既存Output（ログ）Nodeの意味を維持する。 |
| D04 | 互換e2e.romaji-to-hiraganaのunsupported-romaji診断は未変換入力末尾を含む。 | 実CLIの失敗report JSONで確認。CLI stderrは空。構造化だから秘密が除去済みとは扱わない。 |
| D05 | 単発invokeCoreは有効なstdout結果がない場合に当該子プロセスの生stderrをエラーへ使う。 | fake childの同時2要求と後続要求でcanaryの他要求混入なし。正常JSON/構造化失敗reportへ生stderrを転載しない。 |
| D06 | 常駐v2/v3は#230で生stderrの保持/終了HTTP応答への転載を停止済み。 | 既存response-timeoutのcanary検査を維持。構造化Node診断やOS dumpの匿名化を保証しない。 |

製品修正はD01の誤解しやすい表示の具体化。チェックボックス/フッター/説明とApp検査を揃える。
Graphの下書きに固定文字列が保存され、runtime入力キーは保存OFFで存在しないことを実Editorで検査する。
既存localStorage key/同意/採番/Graph保存形式/Node出力/診断契約は変えない。

実行した検査:
- tests/security_diagnostic_audit.rsの3件は実CLIを起動してstdout/stderr/終了コード/JSONを照合。
- response-timeout.test.tsの2件は同時/後続要求と有効stdout結果をfake childで検査。
- security-info/appで保存対象の説明と実Graph下書きへの保存を検査。
- Rust全target/別doctest/fmt/clippy、実CLI/両Host、Editor全test/buildで互換性を確認。

ソースの検索対象はsrc全体とeditor/src・editor/server。現在の入力永続化はuseRuntimeInputの明示同意、
GraphはAppの下書き、本文stderrはinitial_e2e::DebugOutput、単発表示はbridge::invokeCore。
他のRust stdoutはCLI/プロトコルreply、worker.rsのprivate fixture stderrはcfg(test)の試験コード。
この検索だけで動的依存/全OS/第三者コードのログを監査済みとはしない。

## 修正候補と採否が必要なこと

1. 製品Nodeの診断で、短いcode/位置情報と本文を含む詳細を分離し、詳細表示・export・保持の既定を決める。
2. 単発CLIの詳細stderrをブラウザーへそのまま表示するか、codeと一般文だけを表示するかを決める。
3. 互換Output（ログ）は残し、新しい製品用ログNodeの明示許可/保存先/期限を別契約にする。
4. 実保護入力の製品接続時は保存同意より保護状態を優先し、Graph固定値/学習/診断/Host callbackを含めて再検証する。

上記の機械的な文字列置換だけでは、文章変換結果に含まれる任意の秘密を確実に識別できない。
今回は未選定の全診断マスク/ログ禁止/OS設定を導入しない。Nodeの既存意味と自由を維持する。
実OS dump/terminal履歴、Rust標準panic hook、外部ランタイム/依存の出力は別の実測・ポリシーが必要。
目視/実TSF/OS秘密ストア/全診断秘匿の完了とはしない。

戻す場合は本PRの表示/テスト/文書をrevertする。診断露出の監査記録は後続の正式ポリシー検討へ引き継ぐ。
関連：[入力保存](input-storage.md)、[現行Editor説明](editor-security-info.md)、親#230/#215。
