# 一回限りの隔離Workerの準備と配送後の費用

[Issue #29](https://github.com/pitapa79/DIY_IME/issues/29)。開始main `5e2282f61d75decb100b77c0cb86d2faa89c2f35`。固定DLL・公式Wasmは[公開Windows CI](https://github.com/pitapa79/DIY_IME/actions/runs/42155771450) のartifact `13474977409`。Windows x64、Rust 1.90.0 GNUの開発用probeを使用しました。各試行に新しいAppContainerとJobを使用し、親が保持する元プロセスハンドルで実終了を確認します。

```powershell
$env:DIY_COMPILED_PROBE_BUNDLE = 'C:\path\to\windows-app'
cargo test --locked --lib isolated_worker::runtime::compiled_module_probe::prepared -- --ignored --exact --nocapture
cargo test --locked --lib isolated_worker::runtime::compiled_module_probe::prepared_guest -- --ignored --exact --nocapture
cargo test --locked --lib isolated_worker::runtime::compiled_module_probe::prepared_withdrawal -- --ignored --exact --nocapture
```

信頼された固定テスト子が準備完了を通知するまで親はパイプに入力を書きません。公式ゲストは通知後に配送を受けてから、事前生成した成果を照合・デシリアライズし、新しいStore/Instanceで実行します。通知前に行うのはDLLの完全検証・ロードと固定成果の読み込みです。結果の計測点は完全な応答フレームの受信です。その後の正答照合と実終了は両方必須です。準備の時間は親の起動処理開始から配送直前までです。Hostの清掃はJob資源計数待ちを除きます。

| 各12試行、中央値（ms） | 準備 | 配送→応答 | 配送→実終了 | 清掃 | Job CPU |
| --- | ---: | ---: | ---: | ---: | ---: |
| 短いパイプのみ | 61.25 | 0.29 | 2.67 | 12.04 | 39.1 |
| 固定DLLをロードしてから短いパイプ | 197.61 | 0.27 | 4.38 | 11.84 | 164.1 |
| 固定DLLと公式ゲスト | 191.55 | 19.45 | 24.10 | 13.96 | 164.1 |

公式ゲストは準備通知ありと即時配送を12ペア交互順で比較し、全24試行で `kanji` → `かんじ` を照合しました。即時配送のプロセス・パイプ区間中央値218.54 ms、準備通知あり216.11 msでした。公式ゲスト準備済み試行の応答受信範囲は18.20〜22.94 ms、実終了は23.29〜28.02 ms。Jobのピークコミット中央値は約29 MB、1枠の準備は170〜231 msでした。最初の導入費用と複数Workerの並列補充を測った値ではありません。

準備通知を出さない固定子を使い、期限切れと起動確認後の取消を各1回試しました。両方で入力は配送されず、元プロセスの終了と物理カウント0を確認してから回収しました。

100 ms・20 msはどちらもこの単一枠の準備の最短観測より短く、1枠の直列補充は追いつかない見込みです。待機枠を増やした場合の並列CPU/RAMと枯渇は未測定です。12件の単一Node測定を正式分位点や複数Node・候補確定・TSFの実測には使いません。中央値同士や重なる区間を足していません。

この試作では、入力後に残る固定成果の検証・ロードとゲスト実行だけでも応答受信の最短が10 msを超えました。準備の移動だけで到達する案としては通常Hostへ進めません。子の処理をさらに前倒しする場合でも、毎回の真の終了と回収、連続入力時の補充を別に成立させる必要があります。
