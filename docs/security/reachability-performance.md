# 接続到達解析の性能検証（#247）

開始main: 242cbffbd60b83a9c66822120a7e184f8beab8d8。
既存のport/型/重複入力/循環・未解決・risk:not-assessedの意味は維持する。

変更前はedgeごとに全Graphを再走査し、BFSでも全edgeを反復、各到達先へ経路配列をコピーしていた。
変更後はNode/Definition/port/入力占有の索引、raw edgeの反復Kosaraju強連結成分、隣接BFSを使う。
循環は不正portを含むraw edge全体で判定し、既存connectionErrorの判定順序にも一致させる。
解析は概ねO(V+P+E log E)時間・O(V+P+E)保持（Pは宣言port数）。
経路配列はpathを要求した時だけ生成・cacheする。全到達先の全経路を要求すれば経路出力量自体は二乗になり得る。

Node24.22.1/Windows x64、同じ測定器・同じPC、ウォームアップ1回＋5回の中央値。
これは局所的な実測例で、IME遅延や他環境での保証値ではない。古い版では512まで測定した。

| 形状 | Node数 | 変更前解析ms | 変更後解析ms |
| --- | ---: | ---: | ---: |
| 直列 | 32 | 0.352 | 0.110 |
| 直列 | 128 | 9.174 | 0.424 |
| 直列 | 512 | 292.887 | 0.966 |
| 放射 | 32 | 0.058 | 0.058 |
| 放射 | 128 | 0.463 | 0.213 |
| 放射 | 512 | 5.421 | 0.788 |
| 直列 | 2048 | 未測定 | 4.486 |
| 直列 | 10000 | 未測定 | 20.777 |
| 放射 | 2048 | 未測定 | 2.692 |
| 放射 | 10000 | 未測定 | 15.870 |

先頭100経路の具体化は10000直列で0.161ms、10000放射で0.013ms（各1回の参考値）。
測定JSONのheapUsedMiBはGC/過去ケース/テストrunnerを含むプロセス全体の瞬間値で、解析器の保持量や削減率には使わない。
max sampleは5標本の最大でありP95/P99ではない。

任意の再計測（通常CIではbenchmarkだけskip、正当性/大規模検査は実行）:

```powershell
$env:DIY_IME2_BENCHMARK='1'
$env:DIY_IME2_BENCHMARK_LARGE='1'
$env:DIY_IME2_BENCHMARK_OUTPUT=Join-Path (Get-Location) '../target/reachability-benchmark.json'
# 出力先の親targetを事前に作る。結果はソースへ含めない。
npm test -- --maxWorkers=1 --testTimeout=60000 tests/graph-reachability.bench.test.ts
Remove-Item Env:DIY_IME2_BENCHMARK,Env:DIY_IME2_BENCHMARK_LARGE,Env:DIY_IME2_BENCHMARK_OUTPUT
```

変更前の測定は開始mainへ同じbenchmarkファイルだけを加えて実行した。時間を合否閾値にせず、JSONを比較する。
正当性は3Nodeの全有向Graph512通り×3source、宣言/型/言語/root/重複/不正port経由循環等の差分oracle、
10000Node直列の最終完全経路と10000Node循環を実APIで検査する。再帰stackに頼らない。

Editorの解析予算を128/128から10000Node/20000edgeへ拡張し、到達先・未解決事項の表示を各100件に抑える。
予算超過は未解析と表示し、省略時は総件数を明示する。編集中Graphの保存/実行/編集を禁止しない。
Graph/Definition/source不変時はuseMemoで再解析を避ける。表示した経路だけで機密性を保証しない。
戻す場合は本PRの解析/UI/検査/文書をまとめてrevertする。正式Node/API/依存/lockは変更しない。

## 解析後に呼出側がGraphを変更する場合

後続レビューで、遅延pathがlive edgeのfrom.nodeを親として再読込すると、呼出側のin-place変更で循環し得る点を修正した。
BFS時点のprevious nodeを親Mapへ固定し、pathの走査はその値を使う。返すedge参照/順序は従来どおり。
解析後のGraphに対する内容保証は新しい解析結果で判断し、Graph変更時は再解析する。
edgeのfrom変更/元配列除去の後でも有限の元の親列を辿れることと、path cacheを回帰検査する。
