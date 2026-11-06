# 辞書 JSON の生成とゲスト実行の費用

[Issue #33](https://github.com/pitapa79/DIY_IME/issues/33) の Windows x64、Rust 1.90.0 GNU、release probe。固定の Wasmtime DLL と公式 Wasm は [公開 Windows CI](https://github.com/pitapa79/DIY_IME/actions/runs/42155771450) の artifact 13474977409 を使用した。

```powershell
$env:DIY_COMPILED_PROBE_BUNDLE = 'C:\path\to\windows-app'
cargo test --locked --release --lib isolated_worker::runtime::compiled_module_probe::dictionary_wire::dictionary_wire_cost -- --ignored --exact --nocapture
```

同じ読みの正解を持つ3行の辞書に無関係な行を足した。固定入力に対し、Host の JSON Value 構築、JSON エンコード、事前生成したバイト列のコピーを各100回計測した。生成・コピー結果は各試行でバイト比較した。実 AppContainer のゲストにはそのバイト列を渡し、同じ成果物から毎回新しいランタイムを作って prepare と execute を別々に各20回計測した。ゲスト呼び出しの計測区間からランタイム生成と応答検査を除外した。成功時は候補の順序とスコアを照合した。

| 辞書行数 | execute JSON (bytes) | Value 構築 P50 / P95 (ms) | エンコード P50 / P95 (ms) | コピー P50 / P95 (ms) | ゲスト prepare P50 / P95 (ms) | ゲスト execute P50 / P95 (ms) |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 3 | 638 | 0.020 / 0.245 | 0.007 / 0.050 | <0.001 / <0.001 | 0.100 / 0.195 | 0.160 / 0.332 |
| 1,000 | 100,338 | 1.216 / 2.240 | 0.401 / 0.902 | 0.055 / 0.097 | 5.982 / 9.141 | 5.495 / 6.367 |
| 3,000 | 300,338 | 3.397 / 13.374 | 2.072 / 8.172 | 0.081 / 0.205 | 燃料切れ 20/20 | — |
| 10,000 | 1,000,338 | 31.539 / 97.608 | 19.066 / 65.998 | 0.488 / 12.793 | 燃料切れ 20/20 | — |

3行・1,000行は各20回すべて prepare と execute が成功した。3,000行以上は prepare が成功した場合に限り execute を行う条件のため、execute の値は得ていない。Host の各分位点は独立した区間の値で、足し合わせて全体の分位点を推定しない。この固定入力のバイト列全体を再利用する対照は、打鍵ごとに変わる入力を持つ通常 Host の性能測定とは区別する。

1,000行では辞書リソース部分の JSON の再利用を試す価値がある。採否には、入力が変わる通常のリクエストで権限照合・応答・実終了を含む経路を比較する必要がある。バイト列をそのまま再利用しても、3,000行以上の prepare の燃料切れは再現した。大きい辞書の受け入れには別途ゲスト側の費用を解消する必要がある。
