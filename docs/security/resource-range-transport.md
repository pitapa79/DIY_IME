# 辞書範囲取得の native 通信 v1

[固定公式経路の承認済み契約](official-retained-pipeline.md)の段階3、[#64](https://github.com/pitapa79/DIY_IME/issues/64) の接続基盤。辞書形式・検索・索引の意味は Node 側に残す。準備成果の生成、全件検証、公式 Node/SDK の固定、通常 Host の経路選択と Worker 保持は後続。これだけで製品の高速化が完了したとは扱わない。

## 信頼側の接続と所有

`execute_resources` は信頼側の native API。`ResourceWorkerRequest` で今回の入力 bytes と `ResourceRangeSession` を一緒に渡す。セッションは準備済みの正しい資源版、元の RunKey、lease、取消所有者を保持する。RunControl と同一の取消所有者でなければプロセス作成前に拒否する。資源 ID はその接続内でだけ意味を持ち、要求に RunKey やファイルパスを自己申告させない。

同じセッションの所有を IO スレッドと元の Running に保持する。通信エラーでセッションを失効させ、元の Job を停止する。EOF・最終結果・期限だけで実終了の証明にはしない。停止未確認なら Running が process/Job/cache/profile/資源版を所有し続け、元の OS process handle の signalled を待って IO を回収する。

実行期限はセッションの残り予算と RunControl の期限の短い方。RunControl に期限がなくても資源の期限は有限。ブロック中の読取・書込を理由に無制限に待機しない。認可の mutex を保持したまま pipe IO をしない。

## フレーム

整数は little endian。各呼出しは1入力・1最終結果であり、保持ループではない。

| 方向・種別 | 内容 |
| --- | --- |
| Host 開始 | `DIYRNG01` の8 bytes、入力長 u32、入力 bytes |
| Worker 開始 | `DIYRNG01` の8 bytes |
| Worker READ (1) | 種別 u8、要求番号 u64、資源 ID u32、offset u64、length u32 |
| Host DATA (2) | 種別 u8、同じ要求番号 u64、length u32、要求と同長の bytes |
| Worker RESULT (3) | 種別 u8、結果長 u32、元の結果 bytes |

入力・結果は各8 MiB以下。1読取は1〜65,536 bytesで、さらにセッション固有の一回・累積バイト数、回数、範囲、期限の制限を満たす必要がある。要求番号は DecimalId の有効範囲で1から連続し、同時要求は1件。重複、順序違反、未知種別、別資源、短い DATA、番号違い、不完全なフレームを拒否する。結果後に Host は入力 pipe を閉じ、Worker は Host の EOF、Host は Worker の EOF を確認する。余分なデータを拒否し、元の結果 bytes を再エンコードしない。JSON 重複キーなどの最終検証は既存の信頼側で行う。

## Wasm の読取口

native Worker の `--resource-ranges-v1` は上の接続だけを選択する。通常の v1 呼出し・固定公式コンパイル成果の経路は変更しない。package の宣言だけでこのモードを選ばず、任意の第三者 package を公式扱いしない。この段階では JIT 用の明示モードだけであり、固定公式のコンパイル成果と併用しない。

追加 import は `diyime_resource_v1.read`。型は `(i32 resource, i64 offset, i32 length, i32 destination) -> i32 copied_length`。資源 ID は正、offset は非負、length は上限内の正数とする。destination は Wasm32 の unsigned offset として扱う。従来の v1 の呼出し exports と生の入出力 bytes を保ち、既存 v1 の入力に未対応フィールドを追加しない。対応公式 Node の要求・準備成果との接続は別タスクで固定する。

native callback は輸送前に memory export の型、offset+length の checked 加算、memory 終端を確認する。輸送後に返却長・終端を再確認して memory pointer を再取得してからコピーする。読取失敗は trap とし、未知語・短い成功へ変換しない。callback 内の panic を C 境界へ unwind させない。WASI、ファイル、ネットワークの import は登録しない。

FFI の型・所有権は固定 [Wasmtime v50.0.1 の C API](https://github.com/bytecodealliance/wasmtime/tree/v50.0.1/crates/c-api/include) に合わせる。callback の Box は Store 削除後まで所有し、linker、関数型、caller export の各所有物を成功・失敗双方で回収する。DLL は既存の固定 hash 検査を通す。

## 検査

フレームの正常・異常・各位置での EOF と途中失効を単体検査する。`scripts/check-wasm-v3.ps1` は固定 archive を検証し、実 native Worker をビルドした後、`real_appcontainer_ranges` を明示実行する。実 AppContainer 内の検査用 Wasm で複数読取、memory 終端・負 offset・過大要求・別資源・未登録 import・型違い、読取後の取消・権限失効・保護切替、有限期限、既存 v1 の15ケースを検査する。取消所有者が異なる呼出しは開始前に拒否する。終了未確認が返った場合は元の資源版を保持し、元の process owner の retry で実終了した後の回収を確認する。読取直後の失効を決定的に起こす hook は Host のテストビルド内だけに置き、検査対象の子は製品の native Worker を使う。通常のテストで ignore される実 OS 検査を、CI で未実行のまま成功扱いしない。
