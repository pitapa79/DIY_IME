# Core出力・偽文書の検証（#225、IME化前）

開始main `1cff347702828ac5af05d7b12dc4294243b939b4`。
既存EngineとFakeDocumentの境界を検証する。外部Host/TSF/権限を新設しない。

悪意あるMock Nodeが複数出力の一方に正常Data、他方に不正Packet/root/span/score、
不足/未宣言ポートやpanicを返しても、全出力を原子的にErrorとし正常側の下流も実行しないことを検査する。
正常TextのHTML/shell風文字・NUL/ANSI/bidi/Unicodeは文章データとして保持し、JSONで往復させる。
Coreは意味的な悪意の判定や不可視文字の自動除去をしない。表示の可視化方針は別判断。

実行時入力4096スカラー上限とrevision不正は、Mock Nodeの実行前に拒否する。
Packetのin-memory検査はversion/有限score/span順序で、任意Textの長さ上限やOS隔離を提供しない。
wire境界のサイズ・深さ・世代・資源上限は既存json/execution_protocol/stream等の検査で別に扱う。

既存の `examples/ime_host_harness.rs` は実TSFではなく偽文書Host。
文書/Context変更・旧応答・二重確定・UTF-16境界の既存検査を維持し、
本文に見える命令風文字列を確定しても文字列の追記だけが起きる例を追加する。
意味的に不正な変換結果の完全判定や、実OS編集の安全性の証明にはならない。

| SEC | 状態と根拠 |
| --- | --- |
| 096 | 今回検証。実Engineで不正Packetと不正出力を拒否。 |
| 097 | 部分対応。入力上限・Packet構造・既存JSON深さを検査。一般Packetの合計bytes上限は未定。 |
| 098 | 今回検証。Unicode/制御文字のJSON往復。不正UTF-16は既存例、表示方針は保留。 |
| 099 | 今回検証。命令風文字はTextのまま。偽文書への確定も文字列追記のみ。 |
| 100 | 偽文書の確定内容を検査。実TSFは#234。 |
| 101 | 既存FakeDocumentの世代/Context/所有照合を再実行。実TSFは#234。 |
| 102 | 既存FakeDocumentの別文書/古い応答/二重確定拒否を再実行。実TSFは#234。 |
| 103 | 既存Text/偽文書操作の分離。公開文書編集API/権限は未選定。 |
| 104 | TextのHTMLは文字データとして検査。HTML/Markdownレンダラー導入は未承認。 |
| 105 | 不可視文字の可視化UIは未決定。勝手に変換/除去しない。 |
| 106 | 原文/変更箇所表示は既存Editorの範囲。新UIは未実装。 |
| 107 | 既存Node/port単位の結果表示。配布者由来の認証は#227以降。 |

実行対象: security_output_boundary、engine、json、実FakeDocumentのexample tests、
utf16_positionsのexample tests。全体Rust/fmt/clippyも確認する。
本PRは#225の既存Core検証部分で、全SECや実IME完成としてIssueを自動closeしない。
戻す場合は追加テストと文書をrevert。製品Core/正式fixture/依存は変更しない。
