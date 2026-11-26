# TSFの変換候補

[#195](https://github.com/pitapa79/DIY_IME/issues/195) は開始main `dd24b5988a13b5141829fb065b6f78a5b817eb49` の [隔離Host経路](tsf-isolated-host.md) に候補表示をつなぐ。

- 固定の公式辞書グラフから、最終Textと選択前の `dictionary.out` のChoiceを同じ実行キーで取得する。検証済みのText候補を辞書側の順序で並べる。段階Bの一覧は最大64件で、1件の文字列はUTF-16で4096コード単位まで扱う。
- 現在の未確定文書に対応する候補だけを、TSF候補一覧としてアプリへ通知する。アプリが表示を引き受けない場合は、フォーカスを奪わない一覧を対象ウィンドウに表示する。試験プロファイルの登録用CLIがTSFのUIElementカテゴリを登録・解除する。
- Spaceと下キーで次候補、上キーで前候補に移り、選択中の文字列を未確定範囲に表示する。Escで原文に戻る。未確定文字列の新しい編集、文書の切替、外部終了、保護・無効化で古い一覧を閉じる。閉じた一覧への参照は候補を返さない。
- 現在文書への確定は #196、保護操作と配送の統合は #197。Windows 11、通常アプリでの表示位置、インストール済み試験プロファイルは #198 で確認する。

Windows 10 x64の前面Win32試験アプリで、実AppContainerの公式辞書から取得した候補、選択と未確定文字列の一致、照会の非変更、Esc、文書切替後に保持した旧候補へのアクセス拒否を検査する。ハーネスの構成は合成データの専用Hostを利用する。

実装は [MicrosoftのTSF UIElement仕様](https://learn.microsoft.com/en-us/windows/win32/tsf/uiless-mode-overview) と [候補一覧API](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itfcandidatelistuielement) を参照。
