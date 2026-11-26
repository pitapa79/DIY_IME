# TSFの保護入力

[#197](https://github.com/pitapa79/DIY_IME/issues/197) は開始main `6a5a0c75a8282cfa833f958005384f029dab3b7f` の [TSF接続契約](tsf-stage-b-connection.md) と [保護ゲート](security/protected-input-gate.md) をWindowsの入力処理に接続する。

- 手動保護は試験プロファイルの言語バー項目「DIY IME 保護入力」で切り替える。Windowsセッションの専用TSFコンパートメントで状態を共有する。入力欄のInputScope、TSFのキーボード無効、読み取り専用は別に確認する。手動を解除しても、通常欄と確認できないときは自動保護を継続する。
- 保護中と判定不能時はキーをアプリへ通す。変換用の本文を読み出さず、候補と古い要求を失効させる。片付けは所有と内容を照合できる従来の未確定範囲に限る。
- 保護開始でバックグラウンドHostへの新規送信を閉じる。Hostは入力パイプの終了を受けて実行とWorkerを回収する。実際に正常終了したことを確認できるまで再開せず、強制終了や異常終了だけでは回収成功とみなさない。保護解除後は新しい通常欄入力を確認してから、保存済みの権限で新しいHostを起動する。古い入力や結果は再送・再適用しない。
- Windows 10 Pro 22H2 x64の前面Win32試験アプリで、実TSFの保護ボタン、パスワード欄、情報を取得できない欄、キーボード無効、アプリによる直接入力、保護中の配送件数ゼロ、旧候補の消失とHost回収、新入力での復帰を検査する。本文には合成データを使う。Windows 11と通常アプリの実機結果は #198 で扱う。

手動操作には [TSF言語バー](https://learn.microsoft.com/en-us/windows/win32/tsf/language-bar) と [TSFコンパートメント](https://learn.microsoft.com/en-us/windows/win32/tsf/compartments) を使用する。
