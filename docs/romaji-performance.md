# ローマ字変換の既定Table共有

Issue #23。開始SHA `c4f855ca872197b78b57e55ac26679b2b4b02076`。変換規則と公開APIを維持し、毎回の既定表の準備を減らす最小変更を検討・実測する。

## 調査と変更方針

`convert` は設定検査後に毎回 `Table::build(config)` を呼ぶ。既定TSVの148ルールをStringのHashMapへ読み、44種の接頭辞と24種の先頭文字を作る。overridesが空でも再構築される。

今回の変更はoverridesが空のときに、標準ライブラリのOnceLockで一度構築した既定Tableを読み取り専用で共有する範囲に絞る。unknownはTableの構成要素ではなく、引き続き各呼出の設定から判断する。overridesがある場合は既存のTable::buildを使い、任意設定の共有キャッシュを作らない。設定検査は共有表の取得前に毎回実施する。

プロセスの存続中は既定表1組のメモリを保持する。保持量は同梱TSVから決まり、呼出回数や任意設定の種類に応じて増えない。メモリ量そのものは今回測定していない。

最長一致は各候補を `lower(slice)` のStringにして検索するため、短命な割当を減らせる余地もある。最大8文字のASCII候補を固定バッファへ一度小文字化して借用する案はあるが、今回は検索順・Unicode境界・pendingの判定に触れず、別の変更として切り分ける。プロファイリングで支配的な時間を確定したわけではない。

## 検査方針

- 既存の契約・ローマ字テストを維持する。
- 既定unknown=preserve/errorと複数のoverridesを交互に呼び、表の内容が混ざらないことを確認する。
- 並行呼出で同じことを検査する。当該テストだけを別プロセスで実行し、初期化前の状態から並行呼出も確認する。
- 全Rust・example・fmt・Clippyを実行し、[入力長別ベンチ](runtime-benchmark.md)を同条件で変更前後に一度ずつ測る。ベンチ本体・契約期待値・共通core・UI・依存は変更しない。

## 検査結果

- `cargo test --locked --test nodes_romaji cached_default_table_is_safe_during_parallel_configurations -- --exact`: 別プロセスの初回並行実行1件成功。
- `cargo test --locked --test nodes_romaji`: 全20件成功。既定/厳格unknown・異なる2つのoverridesの交互/並行呼出を追加し、既存の22契約例・境界・Unicode・生成入力等も維持した。
- `cargo test --locked`: 全138件成功（doctestを含む）。
- `cargo test --locked --examples`: 全2件成功。
- `cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`: 成功。
- 前後ベンチは各14ケース・100回・warmup10回で、各1540runすべての出力/識別子/完了状態の検証に成功。

## 同じベンチの前後比較

10月27日22:24の変更前記録は開発負荷がある参考値として保持した。以下では再開後に他担当が文書作業中心の条件で取り直した変更前と、親のRustコンパイル完了後に測った変更後を比較する。OS全体の負荷・電源・クロックを固定した環境ではない。

| 項目 | 変更前 | 変更後 |
| --- | --- | --- |
| ソースSHA（両方clean） | `3878edd20a21a0dd60ed2b40c972436e67a9258f` | `4b0ced5ab96f68b30915664860d036cb87b23e1a` |
| 測定開始UTC | 2026-10-27 15:48:00 | 2026-10-27 15:55:50 |
| 測定開始JST | 2026-10-28 00:48:00 | 2026-10-28 00:55:50 |

両方ともWindows 10 Pro 10.0.19045 / Intel Core i5-7300U / 論理4プロセッサ / rustc 1.99.1（58cf8f9c3）・x86_64-pc-windows-gnu・LLVM23.1.1。Cargoの既定release profile、debug assertions=false。`RUSTFLAGS`と`CARGO_BUILD_TARGET`は未設定。ベンチ本体のGit blobは前後とも `b9f4c7b5667909470d4bc200dd6448ab240da1e9` で変更していない。

各プロセスで次のコマンドを使った。environmentラベルだけbefore/afterを区別する。反復数・入力・モード・4ノード・計画版・検証条件は同一。

```powershell
cargo run --locked --release --example runtime_benchmark -- --iterations 100 --warmup 10 --build-label release --environment "Windows 10 Pro 10.0.19045; i5-7300U; before/after table reuse; concurrent agent work limited to documentation; power not pinned"
```

下表はすべて**µs**。変換結果が一致することを検証した上で、前後の値をそのまま載せる。

| 系統 | 入力コードポイント | 前・中央値 | 後・中央値 | 前・p95 | 後・p95 |
| --- | ---: | ---: | ---: | ---: | ---: |
| ascii | 1 | 274.35 | 12.90 | 394.20 | 16.70 |
| ascii | 8 | 273.95 | 19.60 | 696.30 | 29.10 |
| ascii | 32 | 339.25 | 25.60 | 1089.70 | 39.90 |
| ascii | 128 | 435.40 | 132.90 | 979.40 | 154.80 |
| ascii | 512 | 871.50 | 504.25 | 3513.50 | 645.10 |
| ascii | 2048 | 2816.85 | 1657.85 | 19399.70 | 4094.50 |
| ascii | 4096 | 7067.45 | 5324.95 | 31403.00 | 13380.60 |
| unicode | 1 | 240.85 | 16.05 | 387.20 | 32.00 |
| unicode | 8 | 246.10 | 22.15 | 465.20 | 37.90 |
| unicode | 32 | 386.80 | 20.30 | 1013.40 | 33.90 |
| unicode | 128 | 333.55 | 18.45 | 766.50 | 37.40 |
| unicode | 512 | 349.60 | 48.30 | 597.30 | 133.00 |
| unicode | 2048 | 555.40 | 197.70 | 3011.60 | 599.90 |
| unicode | 4096 | 814.25 | 407.30 | 5449.30 | 1043.30 |

計画compileの中央値は47.90→36.50µs、p95は58.20→59.40µsだった。共有表を使わない準備処理にも変動があるため、この単回比較から詳細な改善率を確定しない。全14ケースでrunの中央値減少を観測し、短入力のwarm実行で既定表の再構築を省く効果を確認できたため、この最小変更だけを残す。

共有表の初回構築は最初のwarmupに含まれ、測定100回の値には含まれない。初回実行のコストがなくなる変更ではない。性能保証・CI閾値にはしない。

overrides有りの速度、短命lower候補の割当数、メモリ量、cold初回時間は測っていない。[ベンチ自体の範囲と限界](runtime-benchmark.md)も引き続き適用される。

## 戻し方

本タスクのPRをrevertする。既存の公開Config/convert/Node API、TSV、契約の意味を変えないため、保存グラフや設定の移行は不要。
