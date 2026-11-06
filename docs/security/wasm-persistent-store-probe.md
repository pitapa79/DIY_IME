# 同じ隔離プロセス内でStoreを作り直す試験

[Issue #30](https://github.com/pitapa79/DIY_IME/issues/30)、開始main `5e2282f61d75decb100b77c0cb86d2faa89c2f35`。固定公式WasmとDLLは[公開Windows CI 42155771450](https://github.com/pitapa79/DIY_IME/actions/runs/42155771450) のartifact `13474977409`。Windows x64、Rust 1.90.0 GNUの開発用probeを使用しました。

```powershell
$env:DIY_COMPILED_PROBE_BUNDLE = 'C:\path\to\windows-app'
cargo test --locked --lib isolated_worker::runtime::compiled_module_probe::persistent -- --ignored --exact --nocapture
```

親が所有するコンパイル専用AppContainerの出力を検証し、別の実AppContainer/Jobで固定DLL・Engine・Moduleを保持しました。公式Nodeに合成の `kanji`、`kya`、`ky` を渡し、毎回Store/Instanceを生成・破棄します。計測区間は子の同じ時計で、Store生成、インスタンス化、ゲスト実行と応答の回収、Store破棄までです。正答照合は区間の直後です。

| 子の入力系列 | 正答／全件 | P50（ms） | P95（ms） | P99（ms） |
| --- | ---: | ---: | ---: | ---: |
| 待ちなし | 2,000 / 2,000 | 0.314 | 0.712 | 1.103 |
| 20 ms | 50 / 50 | 1.243 | 2.481 | 2.820 |
| 100 ms | 50 / 50 | 1.067 | 4.697 | 5.539 |

子内の予定に対する最大投入遅れは20 ms系列で8.673 ms、100 ms系列で8.432 msでした。表の分位点は投入遅れを含みません。最初のModuleロードと未使用Storeの破棄まで19.651 msで、DLLのロード・隔離プロセス起動を含みません。Jobの全2,100入力の累積CPUは875 ms、ピークコミット29.0 MB。最後に元のプロセスの実終了とパイプ回収を確認しました。

別の合成Wasmで線形メモリーと可変グローバルを書き換え、同じStoreで値が残る正の対照とtrapを確認しました。新規Storeで両方が初期状態に戻ることを64回照合しました。これはゲスト状態の検査です。プロセス内部のWasmtimeやOS資源、権限、秘密、古い継続に対する保証には別の検査が必要です。

この段階で測ったのは単一Nodeの子内の費用です。Hostとの各入力の往復、複数Node、文書・候補・確定、途中の保護切替、異常終了や取消、実用辞書・TSFを含む結果は未検証です。Store破棄とプロセス実終了は別です。この試験では各呼出しの後も同じプロセスが存続し、製品の現在の呼出しごとの終了保証は満たしません。通常Host経路の切替はこの試験に含めません。
