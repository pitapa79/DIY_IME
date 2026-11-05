# 常駐セッション・プロトコル v2（ローカル試作）

`diy-ime2 session` は標準入力と標準出力で JSON Lines をやり取りし、1プロセス内に1つの Engine を保持する。Windows IME の入力スレッドや外部ノードの ABI ではない。

## メッセージ

要求・応答は UTF-8 JSON を1行ずつ送る。stdout に診断文は混ぜない。各応答を flush するので stdin の EOF を待たずに利用できる。

```json
{"protocol":2,"id":1,"request":{"method":"status"}}
```

成功は `{ "protocol":1, "id":1, "ok":true, "data":..., "state":... }`、失敗は `ok:false` と `error:{code,message}`。失敗時も現在の state を返す。外側の JSON の解析に失敗した場合の id は null。フレームが大きすぎる場合はエラーを1件返して終了し、残りの入力を別の命令として扱わない。

プロトコル版は2、グラフと文章Packetの版は1。それぞれ別の規格であり連動させない。protocol 1の要求はprotocol-versionで拒否する。

## 要求

| method | 引数 | 動作 |
| --- | --- | --- |
| status | なし | 状態取得 |
| definitions | なし | 登録ノードの宣言を取得 |
| validate | graph | 検証と実行順の確認だけを行う |
| prepare | graph | 新計画を検証して保持、準備識別子を発行 |
| activate | token | 一致する準備計画へ切替 |
| run | expectedPlanVersion、任意のinput | 指定した現行版と入力スナップショットで実行し RunReport を返す |
| rollback | expectedPlanVersion | 指定した現行版から直前の構成へ復帰 |

最初の prepare では Engine を稼働させず、最初の activate で稼働版1になる。以後も prepare だけでは出力は変わらない。検証失敗時は active と以前の prepared を保持する。新しい prepare が成功すると token を更新し、古い token による切替は拒否する。

run / rollback の想定版が現行版と異なる場合は `plan-conflict` を返す。rollback でも版番号を増やし、古い実行結果を再び有効にはしない。復帰対象は直前の計画・設定・接続1件であり、ノードの学習状態や外部への副作用は戻さない。

## state

- `revision`: 準備・切替・復帰が成功するたびに増える状態の改訂番号。
- `active`: `{version, graph}` または null。
- `prepared`: `{token, graph, order}` または null。token は不透明な文字列。
- `canRollback`: 直前の計画があるか。

各 run には別の実行 ID を付ける。同じプロセスを使うので、実行 ID は操作のたびに1へ戻らない。異なるセッションではプロセスが異なるため、画面ではセッション ID と計画版も合わせて照合する。

## 実行時入力

```json
{"protocol":2,"id":2,"request":{"method":"run","expectedPlanVersion":1,"input":{"revision":7,"text":"kya","mode":"compose"}}}
```

inputは全文スナップショット。revisionは正のJavaScript安全整数、textは4096コードポイント以内の文字列、modeはcompose / finalize。未知フィールド・不正値・nullはinvalid-runtime-inputで拒否する。計画版の照合を先に行う。入力不正時は実行IDや計画状態を更新しない。

入力を変えてもstate.revision、planVersion、prepared、canRollbackは変えない。入力を渡したRunReportにはinputRevisionを返す。input省略時は固定グラフを従来どおり実行し、input.textがある場合はruntime-input-missingのノードエラーになる。前回の入力を暗黙に再利用しない。

画面はgraphKey・セッション・計画版に加え、入力revisionと内容も照合する。古い入力への応答を最新結果にせず、保持している旧結果は「入力変更前の結果」と表示する。ケーブルのデータも同じ規則で扱う。詳細は [実行時入力](runtime-input-v0.1.md) を参照。

## ブリッジと画面

Vite ブリッジがランダムな ID のセッションをタブごとに作り、1つの子プロセスへ要求を直列送信する。応答の protocol / id / 形式を確認する。同時に1要求だけが送信済みなので、Rust パーサーが返す id:null の入力エラーもその要求への拒否として扱える。未知の応答やプロトコル不一致は接続を終了する。

画面は `revision` が古い状態応答を反映しない。グラフは JSON のオブジェクトキー順を正規化して比較し、Rust とブラウザーの並び方の差で別グラフと判定しない。配列の順序は維持する。

編集中に準備の応答が届いた場合、その状態は記録するが、現在の編集と一致しない計画を切り替えられる表示にはしない。編集中に返った古い実行結果も現在の結果へ反映しない。編集操作で実行プロセスを停止しない。

未適用・無効な編集があっても「実行」は稼働計画を使う。出力に計画版と実行 ID を表示し、編集中と異なる場合は参考結果として区別する。復帰しても編集内容は保持し、稼働グラフを画面へ読み込む操作は別にする。

## 寿命と上限

- 常駐セッション4件、セッション内の未完了要求16件（送信中を含む）。
- 要求1 MiB、応答1フレーム8 MiB。送信後の応答待ちは既定10秒。ホスト起動時の `DIY_IME_BRIDGE_RESPONSE_TIMEOUT_MS` で有限ミリ秒または `unlimited` を選べる。範囲・設定方法・戻し方は[ブリッジの応答待ち時間](bridge-response-timeout.md)を参照。
- 送信中・送信待ちの要求がない状態で15分未使用なら終了（1分ごとに検査）。要求完了時から未使用時間を数える。画面は30秒ごとに状態を確認する。
- タブの終了・再読込時、Vite サーバー終了時に子プロセスも終了する。
- 送信前の取消はその要求だけを破棄。送信後の切断・タイムアウトでは処理結果が不明なため、セッション全体を無効化する。
- 再接続で新しい空の Engine セッションを作る。計画の自動再適用や状態変更の自動再試行はしない。編集下書きはブラウザーに残す。

現段階は信頼された同期 Rust ノード向け。非同期キャンセル、実行中の計画コンパイル、IME の未確定文字列の安全境界、プロセス終了後の状態復元は後続段階。
