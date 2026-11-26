# TSFの未確定文字列

[#193](https://github.com/pitapa79/DIY_IME/issues/193) のWindows x64実装。開始mainは `bd183c3613c2b05be1c127bc88e19ae0b95f9cd5`。入力の整合条件は [TSF接続契約](tsf-stage-b-connection.md) に従う。

- 入力欄で許可したキーをTSFの読み取り編集セッションで照会し、キー押下時の書き込み編集セッションでもフォーカス、入力欄、キャレットまたは選択範囲を照合する。未確定文字列はTSFのcompositionに置き、入力用の表示属性を適用する。入力中のキャレットは末尾に置く。
- 開始前に選択範囲の両端がUTF-16境界であることを検査する。許可された範囲の元の文字列を保持し、未確定文字列を取り消したときは元の文字列と選択範囲を戻す。未確定中のBackspaceは [キー割当](tsf-key-input.md) の書記素境界で原文を削る。
- 文書切替とフォーカス喪失で入力世代を失効する。旧compositionの終了通知を受けた後はその範囲を編集しない。フォーカス喪失時の取消は、まだ編集可能で、所有と内容を照合できた旧compositionに限る。旧文書が編集セッションを拒否した場合と、アプリ側が先に編集・終了した場合の後始末はアプリに委ねる。
- セッションが実行前に拒否された場合は元の未確定文字列を保持する。書き込みに入った後に成否を確認できなければ、入力状態を失効し、同じキーを再適用しない。入力欄の保護状態を確認できない間は文書の内容を読まず、キーをアプリへ通す。
- TSFプロファイルの登録用CLIは、既存のテスト用CLSIDの表示属性カテゴリも登録・解除する。公式Nodeの実行は #194、候補表示は #195、確定と候補状態での取消は #196、手動保護と保護境界の統合は #197 で扱う。

Windows 10 22H2 x64の前面Win32試験アプリで、未確定表示属性、キャレット位置、更新、書き込みロック拒否後の続行、削除、アプリ側で終了した文書への再編集拒否、文書切替の取消、サロゲートを分断する位置の拒否、選択範囲の復元を検査する。テストアプリはTSFに渡す本文と範囲を制御し、表示属性の値を読み戻す。通常アプリでの実際の表示、物理キー、Windows 11、インストール後の表示属性プロバイダーの解決は #198 の実機検証に含める。

MicrosoftのTSF仕様を参照。[Composition](https://learn.microsoft.com/en-us/windows/win32/tsf/compositions)、[表示属性の提供](https://learn.microsoft.com/en-us/windows/win32/tsf/providing-display-attributes)、[外部の編集通知](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itftexteditsink-onendedit)。
