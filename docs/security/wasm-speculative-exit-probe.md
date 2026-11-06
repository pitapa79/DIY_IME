# 固定四段グラフで Worker の終了待ちを重ねる試験

[Issue #38](https://github.com/pitapa79/DIY_IME/issues/38)。開始 main は `5e56d7948712958a3a5df9e6681533701238e400`。固定 DLL と公式 Wasm は [Windows CI](https://github.com/pitapa79/DIY_IME/actions/runs/42155771450) の artifact 13474977409。比較元は [#36](https://github.com/pitapa79/DIY_IME/pull/36)。

## 実行境界

現行 Host は pipe の出力を読みながらプロセスを待つが、元プロセスの正常終了と入出力回収を示す ExitReceipt が得られてから応答を解釈する。アダプターの `call_traced` が返った後に Node 出力または broker 手順を扱い、CompletionPermit を完了させる。通常経路で次段が前段の終了を追い越すことはない。

試験では SHA-256 固定の公式 Wasm と同梱の合成3行辞書を使う。親が四つの固定 Node の execute リクエストを組み立て、ポートから受けた Packet を検査する。各段は別の AppContainer、Job、Store、Instance を一度使用する。Wasm インスタンスに外部関数を渡す経路、ネットワーク broker、秘密、永続ファイル書込の呼び出し口は試験に含めない。辞書は親から pipe で渡した合成バイト列。第三者による能力宣言を利用しない。

直列と投機の両方で四枠を先に準備し、同じ応答フレームを計測する。投機では検査済み Packet を次段の試験用 pipe へ渡すが、各段の所有者は元ハンドルの終了と I/O 回収を待つ。全段の正常終了を照合して初めて試験内で確定可能とする。製品の CompletionPermit・権限・入力 revision・保護との統合を証明する試験ではない。revision と保護は試験内での無効化を注入する。

```powershell
$env:DIY_COMPILED_PROBE_BUNDLE = 'C:\path\to\windows-app'
cargo test --locked --release --lib isolated_worker::runtime::compiled_module_probe::speculative::compare_speculation -- --ignored --exact --nocapture
```

## 最初の比較

Windows x64、Rust 1.90.0 GNU の release 計測。各方式12件を交互順で実行し、全件の正答と元プロセスの終了を確認した。時間は四枠が準備された後の最初の配送から、検査済みの出力を確定できるまで。準備と全資源の回収は別区間。

| 方式 | 確定可能まで 最短 / P50 / 最大 | 四枠の準備 P50 | 最初の配送から全枠回収まで P50 |
| --- | ---: | ---: | ---: |
| 段ごとの終了待ち | 38.643 / 84.667 / 250.693 ms | 766.670 ms | 137.723 ms |
| 終了待ちを重ねる | 13.931 / 25.643 / 48.827 ms | 826.838 ms | 142.589 ms |

異常終了（前段・後段）、不正・欠損応答、期限、取消、試験内の revision 失効・保護移行で、各1回すべての元プロセスを回収し、結果の確定を拒んだ。準備済み・小辞書の投機試験でも初回の最短値が10 msを超えた。通常 Host の性能目標や安全性の証明には使用しない。投入予定と OS 終了確認を含む再試験は Issue #38 へ記録する。
