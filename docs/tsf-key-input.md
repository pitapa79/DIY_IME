# 段階Bのキー入力

[#192](https://github.com/pitapa79/DIY_IME/issues/192) の固定試験用キー割当。開始mainは `ed7381d2c556ae213245d7b73df07a35039c29e6`。[TSF接続契約](tsf-stage-b-connection.md) の承認範囲を使用する。

Windowsのキー変換は現在スレッドの配列と修飾状態を参照する。Ctrl、Alt（AltGrを含む）、Windowsキーの組み合わせと、Shift付きの編集・候補操作キーは通過する。ローマ字入力はASCII英字と撥音表記用のアポストロフィを扱う。配列の変換先が確定しない入力、非ASCII、対象外のキーはアプリへ渡す。Windowsのキーボード変換状態を変更しないフラグを使う。[MicrosoftのToUnicodeEx仕様](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-tounicodeex)。

キー判断は現在の入力状態から操作を返す読み取り処理。TSFの事前照会で編集を行わず、キー押下で改めて判断する。入力を閉じている間はどの操作も返さない。空の未確定状態ではSpace・Enter・Esc・Backspace・上下を通す。選択中だけ上下で候補の移動を返す。Backspaceは末尾の拡張書記素クラスタより前のUTF-8境界を返し、選択中かどうかにかかわらず未確定原文を一文字分だけ短くする。[Unicodeの書記素境界](https://www.unicode.org/reports/tr29/)。

Stage Aの診断DLLは組み込んだキー判断を経由し、従来どおり小文字 `a` の診断を扱う。未確定状態を対象にした操作は #193 の文書編集、#195 の候補、#196 の確定・取消で接続する。キー判断にNode実行や待機はない。実際の隔離Hostは #194 で接続する。

自動検査はキー割当、対象外のキー、入力無効、候補状態、結合文字・絵文字等の削除境界を網羅する。前面Win32アプリの追加検査は対象外キーの通過、事前照会での編集件数ゼロ、通常キー押下の一度だけの編集、保護欄での非編集を確認する。物理キー、Windows 11、製品としての未確定表示・候補は後続の実機検証範囲。
