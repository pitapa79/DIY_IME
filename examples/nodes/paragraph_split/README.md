# 段落分割（独立試作）

Issue #213。`experimental.paragraph-split@1` / `ParagraphSplit`。
必須 `in`: ja/Text → `out`: ja/Sequence。完全設定は `{}` のみで、省略時も `{}`。
未採用・未登録の試作。標準Registry・module・UIへ追加しない。

CRLFを1改行、単独CRとLFも各1改行と数え、連続改行が2個以上ある連続群の直後で分割する。
区切り群は直前のTextにすべて保持する。単一改行では分割しない。
先頭の区切り群は独立した要素になり、末尾の区切り群の後へ余計な空要素は追加しない。
空入力は空Text1個。要素を順に連結すると原文のUTF-8 bytesに完全一致する。

空白を挟む改行は連続と見なさない。Unicode行区切り・段落区切り、Markdown、引用、文法は解析しない。
正規化・trim・空白追加・外部I/Oは行わない。入力は不変で、出力各TextのmetaはNone。
NoOutputはNoOutput、通常Engineでは必須入力がNoOutputならSkipped。
設定不正、入力欠落、不正Packet、非Textはそれぞれinvalid-config、missing-input、invalid-packet、invalid-root。

[独立契約15例](contract.json)を実Registry/Engineへ照合する。
短文総当たりで原文保存と区切りの性質、複数インスタンス・反復実行・異常系も検査する。

```powershell
cargo test --locked --test nodes_paragraph_split
cargo test --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

標準登録・正式採用は別工程。共通API・正式fixture・依存は変更しない。
戻す場合は対応PRの所有実装・担当テスト・例をrevertする。

2026-10-31検査: 開始main `1cff347702828ac5af05d7b12dc4294243b939b4`、
Windows x64 GNU / Rust 1.90.0で担当5件・全体648件（doctest含む）、fmt check、
clippy全target `-D warnings`が成功。独立AIレビュー・MSVC・TSFは未実施。
全体検査の初回はサンドボックスで既存Hostテストのリンク作成2件がアクセス拒否となり、
同じ全体検査をサンドボックス外で再実行して成功した。期待値や検査項目は変更していない。

GitHubコネクタで上記SHAの全412ファイルを取得し、各Git blobハッシュを照合した専用スナップショットで検査。
既存412ファイルは検査後も元のblobと一致する。ローカルGit cloneの認証やシステムPATHの変更は不要。
