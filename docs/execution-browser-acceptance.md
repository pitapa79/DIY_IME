# v3実画面の取消・ページ離脱の受け入れ検査

Issue [#200](https://github.com/pitapa79/DIY_IME2/issues/200)。検証対象はmain `25ec9eeb1dd53d1f85c88ce33b01592252f84c18`（PR #199）。製品画面・bridgeと既存の[ゲート付きHost](../examples/execution_broker_gate.rs)を接続した手動検査であり、製品Registryへ検査ノードを追加しない。Windows 11 Home 10.0.26300 x64 / Rust 1.90.0 GNU / Node 24.22.1 / IABの標準画面で確認した。Windows 10/MSVC/TSFやブラウザー全種の確認ではない。

## 再現用の隔離設定

対応Node/npmとRust、editor依存を用意し、CLIと両検査Hostをビルドする。別CARGO_TARGET_DIRならビルドした3実行物をtarget/debugへ配置し、元とSHA256を照合する。今回のRustソース・実行物はPR #199の全Rust442件・editor192件と同一である。本Issueは文書だけの差分なので全ゲートを再実行したとは数えない。

```powershell
cargo build --locked --bin diy-ime2 --example execution_broker_gate --example execution_transport_fixtures
# リポジトリrootで実行。既存のQA/所有ファイルを上書きしない。
New-Item -ItemType Directory target/qa200 -ErrorAction Stop
$env:DIY_IME_BROKER_TEST_GATE = (Resolve-Path target/qa200).Path
```

次をtarget/qa200/policy.jsonへ保存する。これは本検査で明示した設定で、製品の既定値やノード数制限の提案ではない。queueWaitMs:nullは無期限、cancelGraceMs:100は停止確認の猶予であり強制停止時間ではない。

```json
{"maxActiveRuns": 8, "maxPhysicalJobs": 1, "maxObservers": 8, "maxQueuedJobs": 0, "maxQueuedInputBytes": 1048576, "maxRetainedRuns": 32, "maxRetainedBytes": 4194304, "maxResultBytesPerRun": 1048576, "retentionMs": 60000, "cancelGraceMs": 100, "maxDedupeEntries": 64, "maxTombstones": 64, "queueWaitMs": null}
```

次をtarget/qa200-config.mtsへ保存する。v2は通常Core、v3だけを検査Registryへ接続する。HTTP記録は専用loopbackのmethod/path/statusだけで、入力履歴・ユーザー辞書を採取しない。

```typescript
import react from '../editor/node_modules/@vitejs/plugin-react/dist/index.js';
import {createApiMiddleware} from '../editor/server/bridge.ts';
import {resolve} from 'node:path';
import {appendFileSync} from 'node:fs';
const root=resolve(import.meta.dirname,'..'), qa=resolve(root,'target/qa200');
export default {root:resolve(root,'editor'),clearScreen:false,plugins:[react(),{name:'qa200-gated-core',configureServer(server){
const api=createApiMiddleware({execution:{policyFile:resolve(qa,'policy.json'),core:resolve(root,'target/debug/examples/execution_broker_gate.exe'),root}});
server.middlewares.use((req,res,next)=>{if(req.url?.startsWith('/api/v3/'))res.once('finish',()=>appendFileSync(resolve(qa,'http.log'),`${req.method} ${req.url} ${res.statusCode}\n`));
if(req.url==='/qa-away'){res.setHeader('Content-Type','text/html; charset=utf-8');res.end('<h1>ページ離脱の検査</h1><a href="/">エディターへ戻る</a>');return;}next();});
server.middlewares.use(api);server.httpServer.once('close',api.close);
}}],server:{host:'127.0.0.1',port:5179,strictPort:true}};
```

次をtarget/qa200.mjsへ保存し、対応Nodeで `node target/qa200.mjs` を実行する。標準入力を開いたターミナルを使う。

```javascript
import {createServer} from '../editor/node_modules/vite/dist/node/index.js';
const server=await createServer({configFile:new URL('./qa200-config.mts',import.meta.url).pathname.replace(/^\/([A-Za-z]:)/,'$1')});
await server.listen();console.log('QA200_READY_5179');process.stdin.setEncoding('utf8');process.stdin.on('data',async data=>{if(data.trim()!=='stop')return;await server.close();console.log('QA200_CLOSED');process.stdin.destroy();});process.stdin.resume();
```

## 通常UIからの検査と実測

1. http://127.0.0.1:5179/ を開き、「非同期実行」を選ぶ。「グラフ JSON」から次を適用して「実行結果」へ戻る。

```json
{"version":1,"nodes":[{"id":"held","kind":"test.http-held","version":1}],"edges":[]}
```

2. 「計画を準備」→「切替」→「実行」→「取消」。画面が「結果の採用取消 / 出力取消」と「停止未確認1 / 実際の処理終了は未確認」を表示した。開始回数はstarts.logの1行、cancelledファイルが存在。run期限は空欄で、Core経過296msを表示した。UI時計から経過時間を合成していない。
3. 検査側で `Set-Content target/qa200/producer-end 1 -NoNewline` とし、producer-endedを待つ。callback-returnは作らない。この間も停止未確認1を維持し、producer通知だけで物理枠を返さなかった。
4. `Set-Content target/qa200/callback-return 1 -NoNewline` とする。画面は初めて全物理数0と「実際の処理終了を確認しました」を表示。結果の採用取消は完了へ変化しなかった。
5. 全物理数0を確認してから、このQAのproducer-end/callback-return/producer-ended/started/cancelledだけを除去する（starts.logは保持）。2度目の「実行」で実行中1を確認し、同じタブを /qa-away へ通常遷移させる。合成pagehideイベントは使わない。
6. http.logに対象SessionへのDELETE 200を観測し、cancelledも生成された。所有ファイルとCoreは生存し、プロセスIDは初回と一致した。producerとcallbackはまだ解放しない。
7. ブラウザーの履歴で戻ると、この環境ではページが新規ロードされ同期既定へ戻った。「非同期実行」を明示すると新Sessionの「計画未設定」、実行無効・自動実行OFF。元の稼働計画や結果を復活させない。実producer開始数は2のまま、同じHostプロセスが処理を継続し、旧仕事の所有を捨てた別Hostを生成しなかった。StrictModeによる遅延open用の余分なSessionも自身のDELETE 200で閉じられた。
8. 2件目のproducer-end/callback-returnを両方解放し、検査タブを閉じる。サーバーの標準入力へ `stop` を送る。server.close後にCore PIDの消滅とpolicy.json.broker-lockの自力削除を確認した。Host killやlockの強制削除で終了を代用しない。

ゲートのファイル操作は検査Registryの外部刺激であり、ブラウザー内の状態やAPIを直接書き換える操作ではない。操作対象は今回作った専用ディレクトリーと自プロセスだけ。停止が必要な場合も、先に両ゲートを解放して真正な終了を待つ。所有ファイルが残った場合は自動削除しない。

## 限界と残件

履歴復帰は新規ロードだったため、実BFCacheのpersisted:true経路は未検証。React/jsdomの合成イベント検査を実ブラウザー成功へ読み替えない。ブラウザー停止/電源断、全ページ寿命条件、期限境界・復旧の実画面、全54件wire適合もこの検査の完了条件ではない。

初期weatherグラフは検査Registryに登録されていないため、検査グラフへ差し替えるまでReact Flowの未知handle警告が出た。警告0とは記録しない。最初のconfig.tsではViteの将来native loaderに関する警告も出たため、再現例は同内容のconfig.mtsとした。この基点では取消後のNode表示が未実行だった。Issue #202 / PR #203でdiscardedを採用対象外として保持する修正と回帰検査を追加し、同じ実Host/ブラウザーでstalled1→終了0でもラベル保持・Dataなし・警告/エラー0を確認した。

戻す場合は本Issueの文書差分をrevertする。製品コード・Registry・依存・正式fixtureは不変。現在の実装の境界は[画面モデル](execution-ui-model.md)、輸送所有権は[HTTP文書](execution-http-v3.md)を参照。
