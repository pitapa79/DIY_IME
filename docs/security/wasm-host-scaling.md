# 通常 Host の Wasm ノード増分の予備測定

対象は #85 と #84。開始点は本体 main `bb5293ba2bbbcd2d34a764221aeb2afaa4128470`。Windows x64、Node 24.22.1、Wasmtime 50.0.1。実行ファイルは [main CI の配布物](https://github.com/pitapa79/DIY_IME/actions/runs/42555426096/artifacts/13648045743)。実際の External Host と AppContainer Worker を使い、HTTP・Editor 描画を含めない。資源枠は本測定用に明示した。検査用 Node は入力 Packet を変えずに返す。

## 再現

配布版の `bin/diy-ime2-external-host.exe`、`bin/diy-ime2-wasm-worker.exe`、`runtime/wasmtime.dll` を使う。リポジトリの Node package tool と検査用 Wasm を次のコマンドでビルドする。

```powershell
cargo build --locked --bin diy-ime2-node-package
cargo build --locked --manifest-path tests/fixtures/wasm_guest/Cargo.toml --release --target wasm32-unknown-unknown --target-dir target
node scripts/benchmark-wasm-host-scaling.mjs HOST PACKAGE_TOOL WORKER DLL WASM 10 4 all
node scripts/benchmark-wasm-host-scaling.mjs HOST PACKAGE_TOOL WORKER DLL WASM 10 1 parallel
node scripts/benchmark-wasm-host-scaling.mjs HOST PACKAGE_TOOL WORKER DLL WASM 10 4 official
```

各ケースは最初の1件、計測10件、詳細トレース1件を分けて返す。入力失効と開始の前から、現在の正しい終端結果と物理コールバック数ゼロまでを計測する。追加の status 照会で Worker と物理枠のゼロを確認する。JSON には全試行、診断トレース、バイナリ SHA-256 が含まれる。少数試行なので下表は中央値を報告する。

| 構成 | P50 ms |
| --- | ---: |
| Wasm なし | 1.89 |
| 直列 1 / 2 / 4 / 8 / 16 段 | 82.86 / 163.08 / 331.47 / 666.10 / 1292.47 |
| 直列 4 段・2つの異なるバイナリ識別子 | 322.38 |
| 並列 4 段・1枠 / 4枠 | 332.68 / 143.39 |
| 実行1段 + 対象外15段 | 82.66 |
| 直列4段・先頭 NoOutput | 83.53 |
| 同じデータを返す検査用 / 公式の1段 | 85.52 / 60.81 |
| 同じデータを返す検査用 / 公式の直列4段 | 330.50 / 231.08 |

直列の第1段から第16段の増分は、この条件では平均約81 ms/段。異なるバイナリ識別子の対照は同じ実行コードにカスタムセクションを付けたもの。公式との対照は実コードも異なり、単に作者属性だけを変えた比較ではない。別試行の単段詳細トレースでは、検査用は staging 16.26 ms、プロセス区間61.61 ms、cleanup 5.32 ms。プロセス区間内の Worker 診断で DLL ロード16.47 ms、コンパイル24.06 ms、ゲスト呼出0.018 msだった。公式ではコンパイル区間が観測されず、DLL ロードは15.55 ms。トレースの区間は包含関係があるため合算しない。4枠並列の Host トレースは完全性を確認できなかったため、区間別の比較には使っていない。

事前コンパイルの適用範囲、Worker の事前準備、保持 Worker の一般化は別々に検証する。並列枠だけを増やしても、依存のある直列グラフには各段の固定費が残る。分岐合流、権限付き資源、20/100 ms 間隔、CPU/RAM、安全の負例、試行増加は残作業。製品経路・許可・公開契約・安全保証は変更していない。
