# 共有Hostを使う明示HTTP v3接続

Issue #190。[stdio broker](execution-broker-v3.md)をViteの別APIへ接続する。画面の既定と旧 `/api/sessions` はprotocol 2のまま。v3はホスト管理者が完全なResourcePolicyを明示した場合だけ有効で、既存画面を暗黙に切り替えない。

## 設定と起動

全13項目のUTF-8 JSON policyを用意する。項目・範囲・nullの意味は[採用済み契約 §7](execution-control-contract-v0.2.md)を参照。試験fixtureのprofileを製品の既定値として作成・流用しない。

```powershell
$env:DIY_IME_V3_RESOURCE_POLICY = (Resolve-Path C:\path\host-policy.json).Path
.\scripts\start-editor.ps1
```

未設定ならv3 APIはhost-unconfiguredで拒否し、旧v2は利用できる。空の設定や不正なpolicyを隠れた値で補わない。ファイルは起動するCoreが検証する。起動後にファイルを書き換えても、生存Hostのpolicyは変わらず、helloが実際の値を返す。変更する場合は新受理を止め、実仕事終了後に明示的に起動し直す。

`DIY_IME_BRIDGE_RESPONSE_TIMEOUT_MS` は既存の接続監視設定を共用する。正の整数msまたはunlimited。これはHTTP要求に応答が戻るまでの監視であり、run/jobの処理期限ではない。健全なget-runの応答が続く長時間仕事は、この値を超えても継続する。

## HTTP API

| 操作 | パス・本文 | 意味 |
|---|---|---|
| POST | `/api/v3/sessions`、本文 `{}` | 空のSessionを作る。dataはid、resourceDomain、resourcePolicy、state |
| GET | `/api/v3/sessions/:id` | stateとstatus結果を取得 |
| POST | `/api/v3/sessions/:id/request`、v3のmethodオブジェクト | [12 method](execution-session-v3.md)を実Coreへ配送 |
| DELETE | `/api/v3/sessions/:id` | 対象Sessionを閉じる。仕事の物理終了は別に確認する |

idはCoreが生成した32桁のepoch。要求例は以下の順で送る。実際のidとprepare応答のtokenを使う。

```json
{"method":"prepare","graph":{"version":1,"nodes":[{"id":"source","kind":"input.text","version":1}],"edges":[]}}
```

```json
{"method":"activate","token":"1"}
```

```json
{"method":"start-run","startKey":"1","expectedPlanVersion":"1","targets":[{"node":"source","port":"out"}],"input":{"revision":1,"text":"kya","mode":"compose"}}
```

start-runのresultは受理情報。返されたkeyをget-runへ渡してrunning/terminalと結果を確認する。受理応答を失った場合は、同じSessionのget-startで同じstartKeyを照会し、新しい開始を自動送信しない。definitionsもこのv3経路を使い、旧 `/api/nodes` の宣言をv3宣言として流用しない。

通常のHTTP成功は `{ok:true,data:{state,result}}`。stateは[protocol 3の6項目](execution-protocol.md)で、revisionとu64識別子は十進文字列、初期revisionだけ0を許す。input.revisionやms・bytesは従来どおり安全整数のnumber。JS Numberでu64を丸めない。HTTP層の要求照合IDもBigIntから文字列を作る。

Coreの要求拒否はHTTP 422と `{ok:false,error,state}`（busyは503）。入力失効は計画改訂と分離し、旧RunKeyのcurrentを失効させる。通信故障には偽のCore stateやNodeErrorを付けない。NodeのErrorはget-runの結果に含まれるもので、HTTP送信成功と同じ意味ではない。

## 共有と切断

同じCore実行ファイル・root・canonical policy pathを使うmiddleware所有者は、一つのbroker子プロセスを参照共有する。Sessionごとのspawn、最後のタブのcloseによるHost予算のリセット、失敗後の自動再起動は行わない。新Sessionのepochと空の計画を明示し、旧SessionのRunKeyをsession-conflictとして拒否する。新epochのstart-not-seenを、旧epochで実行されなかった証拠にしない。

Core全体の4Session/16未完了要求を維持する。Node側の未完了wire要求も16件までとし、通常要求は12件まで、残り4件をSession close用に予約する。放棄した応答も照合が済むまで追跡し、古い返信を次要求に渡さない。要求はbroker包絡を含め1 MiB、返信は包絡込み8 MiBまで。非UTF-8、不正ID/版/状態/過大な返信を通信故障とする。

送信前にAbortSignalが取り消されていれば送信しない。作成前なら子プロセスも起動しない。送信後のHTTP切断や応答監視切れは結果不明のsession-lostとし、該当Sessionをローカルで無効化してclose-sessionを送る。別Sessionや共有子プロセスをkillしない。Coreが返信できない場合はcloseの受付も未確認なので、通信の監視切れを「物理仕事の終了」とは表示しない。

broker自体の通信故障ではstdinを閉じ、新規受理を止める。親はchildをkill/unrefせず、Core rootが実仕事を終了するまで所有する。最後のmiddleware所有者のcloseも同じ経路を使う。正常な終了を観測するまでは、同じ親プロセス内の新しい所有者も置換できない。異常終了したHostは自動再起動しない。

## 親プロセスをまたぐ所有

bridgeはcanonical policy pathに `.broker-lock` を付けた所有ファイルを、Coreの第3引数として必ず指定する。

```text
diy-ime2 broker-v3 C:\path\host-policy.json C:\path\host-policy.json.broker-lock
```

Coreがcreate_newで原子的に取得し、PIDと実resourceDomainだけを書く。Windowsでは読取りだけを共有し、生存中の他プロセスによる書換え・削除を許さない。HostDrainが真正なproducer終了と開始callback復帰を確認した後にファイルを解放する。親bridgeが先に終了しても、古いCoreが待機している間は置換側の起動を拒否し、別の私有予算を作らない。

通常終了ならファイルはCoreが削除する。異常終了やデバイス切断で残った所有ファイルは自動削除しない。復旧時はファイルのPID/domainと元のCore・実仕事の状態を確認し、終了確認後に管理者がその所有ファイルだけを除去して明示的に起動し直す。PIDが応答しないことだけで、外部producerの終了まで証明したと推測しない。公開外部ABIや強制隔離の保証は追加していない。

同じHostを継続する親は同じcanonical policy pathを使う。別ファイルの設定は別の明示Hostであり、接続故障の自動回避先にしない。旧 `broker-v3 <policy>` と独立 `session-v3` の既存動作は維持する。

## 検査

実Core/HTTPを使う[専用検査](../editor/tests/execution-broker.test.ts)と、[ゲート付きHost例](../examples/execution_broker_gate.rs)を用意した。Host例は検査専用Registryへ一つのproducerを登録し、製品ノードには登録しない。producer終了・callback復帰・返信flushのゲートを分け、取消・開始回数・所有ファイル・実PIDを観測する。

```powershell
# 通常のtargetを使う場合。editor検査より前に両方をビルドする。
cargo build --locked --bin diy-ime2 --example execution_broker_gate --example execution_transport_fixtures
cd editor
npm test
npm run build
```

`start-editor.ps1 -Check` も検査Hostを準備する。worker起動が競合する環境では `npm test -- --maxWorkers=2` で並列数を抑えられる。期待値やテストの隔離は変更しない。

検査する範囲は、同Host/新epoch、計画分離、u64最大値と開始重複排除、入力失効と計画改訂の分離、既存v2、送信前取消・送信後切断・監視期限、別Session生存、再接続busy、Host非killの実仕事終了待ち、置換拒否、stale owner保存、16要求とclose枠、形式/出力上限である。正式54件のCLI/HTTP照合全体やUI移行、Windows 10/11双方・MSVC/TSFの完了は主張しない。

戻す場合は新受理停止・生存仕事の真正な終了確認後に対象PRをrevertする。旧v2は維持する。正式B03/B07/B08の輸送照合はPR #193、明示画面接続は[v3画面モデル](execution-ui-model.md)と[利用手順](../editor/README.md#非同期実行v3を選ぶ)を参照。HTTP検査を、画面受け入れ全体の完了へ読み替えない。
