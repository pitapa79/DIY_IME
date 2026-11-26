# TSFから隔離Hostへの変換

[#194](https://github.com/pitapa79/DIY_IME/issues/194) は [TSF接続契約](tsf-stage-b-connection.md) に従い、開始main `647faf74736900ace77ec563577d3bbb83ec7f65` の [未確定文字列](tsf-preedit.md) を既存の隔離Hostに接続する。

- 固定グラフは [公式辞書の試験例](../examples/graphs/official-dictionary.json)。Hostは既存の `OfficialGraphReview` が公式配布物との一致を証明した計画だけをこの経路で受け付ける。通常の実行・本文配送・辞書の既存の承認を再利用する。IME用クライアントから新しい権限を付与する操作は呼ばない。
- TEST用に登録したDLLの隣の `diy-ime2-external-host.exe` と `ime-execution.json` を使用する。構成は信頼した試験配布側で用意する。プロセス内の試験ハーネスは、合成データを使う専用Hostの絶対パスを指定できる。Hostが準備できていない間は、未確定文字列をローカルで編集でき、Spaceはアプリに渡る。
- Spaceの実処理で確認済みの未確定原文を受け付ける。TSFアパートメントでは文書・入力世代の照合と有界のキュー操作だけを行う。Hostの準備、IPC、実行、取消後の結果確認、プロセス終了待ちはバックグラウンドで行う。
- 結果は完全な実行キーに束縛し、受信時と編集直前に対象を照合する。本文の置換先は自分が所有する未確定範囲で、Nodeは文書位置を指定できない。入力更新・対象変更・保護時は以前の要求を失効する。空のText、NoOutput、エラーは区別する。
- Spaceで結果を未確定表示する。候補の表示・移動は #195、Enterによる確定は #196、保護操作と配送・復旧の総合検証は #197。Windows 11、通常アプリと一時設置した登録環境は #198 で検査する。

`scripts/check-wasm-v3.ps1` は既存の公式パッケージ検査に続けて、実Hostで合成本文のcompose、finalize、空Text、失効、Host終了を検査する。前面Win32試験アプリの追加検査は実行時に渡された合成データの構成を使い、TSFキー経路から変換結果の表示と文書切替を確認する。
