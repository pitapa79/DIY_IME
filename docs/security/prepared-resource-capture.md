# 辞書準備成果の回収 v1

[固定公式経路の承認済み契約](official-retained-pipeline.md)と [範囲取得](resource-range-transport.md) の後続、[#66](https://github.com/pitapa79/DIY_IME/issues/66) の native 基盤。Host は承認済み原本と不透明な成果 bytes を所有する。辞書の全件検証・索引の構築・原本との意味の一致は Node が担当する。

この段階では通常 Host の計画切替、正式 Node/SDK、固定公式成果の選択、保持 Worker を接続しない。構築成功は、それだけで計画の有効化や権限を与えない。後続の統合は既存計画を失敗時に置き換えず、現在の認可の下で全準備の原子的な切替を行う。

## 原本と成果の区別

`PreparationSources::open` に渡せるのは、正しい設定の資源許可で既に捕捉した不変の原本だけ。元の gate/target/RunKey/取消を保持し、read の範囲・累積 bytes・回数・期限は `ResourceRangeSession` と同じ境界で検査する。この型は原本を全件検証済みの辞書と宣言しない。実行用 `ResourceRangeSession::open` の「完全な準備検証を終えた資源だけを渡す」という契約は維持する。

原本の名前・長さ・全 bytes の SHA256 を安定順で結び付けて原本版の fingerprint を作る。hash は gate の mutex の外で計算し、前後に現在性を確認する。hash 一致を辞書の意味の検証に置き換えない。

`PreparationIdentity` のコード・Node 定義・正確な設定の SHA256 は信頼側の構成担当が実際の検証済み資産へ結び付ける。native API は64桁の lowercase hex を検査するが、この引数だけで公式コードや設定との一致を独立に証明しない。guest からこの proof を受け取らず、package の宣言だけで構築・実行経路を有効にしない。

## 構築専用の通信と Wasm import

Guest の共通要求と安全な Rust 呼出しは [Guest SDK v1](resource-guest-sdk.md) に定義する。構築と消費は異なる import 能力を持つ module としてビルドする。

native Worker の `--build-resources-v1` は `DIYBLD01` で開始する。既存 `DIYRNG01` の読取モードと混同しない。開始フレームの入力長・入力 bytes、READ/DATA/RESULT、8 MiB の入力/最終返答上限、EOF/trailing の検査は範囲取得と同じ。保持ループではなく1回限りの構築であり、構築成功には正常な実終了を要する。

追加フレームは次のとおり。整数は little endian。

| 方向・種別 | 内容 |
| --- | --- |
| Worker CHUNK (4) | 種別 u8、成果 offset u64、length u32、成果 bytes |
| Host ACK (5) | 種別 u8、同じ offset u64、同じ length u32 |

成果は1本の不透明な byte stream。offset は0から直前の終端へ連続し、空 chunk、重複・飛越・巻戻しを拒否する。一回は1〜65,536 bytes。READ は従来どおり1から連続する要求番号を使い、CHUNK は別の連続 offset を使う。両者を交互に呼んでも読取番号を再利用しない。短い ACK、種別・offset・長さの違い、未知フレーム、不完全な bytes は失敗とし、再試行や最終返答で回復させない。

追加 import は `diyime_prepared_v1.emit(i32 source, i32 length) -> i32 copied_length`。source は unsigned Wasm32 offset、length は上限内の正数。memory export の型、checked 加算、memory 終端を検査し、最大65,536 bytesを独立の buffer へコピーしてから native pipe へ渡す。返却値は Host の正しい ACK を得たときの length とする。失敗は trap とし、panic を C 境界へ unwind させない。

構築モードだけが emit を linker に登録する。読取モードで emit を import した module は instantiate 時に拒否する。WASI、native path、ファイルの書込権限やネットワークは追加しない。固定コンパイル成果の既存 hash 判定と v1 の通常呼出しは維持する。

## 上限と終了の所有

信頼側 `PreparationLimits` は正の成果 bytes 上限・chunk 数上限・成果 buffer の resident 予算を指定する。成果 bytes は u32 の範囲内。一回の chunk、成果 buffer、最終 Arc 化で同時に存在するコピーを含め、`2 * max_artifact_bytes + 65,536` 以上の buffer 予算を checked 計算で要求する。これは成果 buffer 用の予約であり、プロセス全体の RSS 上限ではない。原本・要求・返答・実行環境と同時構築数の全体会計は後続の Host 統合で別に計上する。

成果 buffer を構築前に予約し、各 chunk の配送前に回数・連続 offset・累積長を予約する。短い配送や失敗で予算を戻して再利用しない。許可の mutex を保持したまま pipe IO しない。取消所有者は RunControl と同一でなければ開始前に拒否し、期限は原本セッションの残り予算と RunControl の短い方とする。

資源の絶対期限を起動前と元 process の停止判定へ引き継ぐ。起動に費やした時間を、resume 後に数える相対 walltime だけで除外しない。資源期限切れが共有 token を取消状態にした場合も、絶対期限の経過を確認した停止は Deadline と記録する。終了未確認の所有・回収条件は変更しない。

`PreparationOutcome::Unconfirmed` の owner は元の `Running`、原本、途中成果と予約を保持する。利用者が owner を破棄する場合も、元の Worker の停止と IO 回収より先に原本・成果を解放しない。待機や Drop を UI スレッドで行わない。guest の完了宣言、pipe EOF、経過時間は実終了の証明にならない。

## 成果の採用条件

最終 RESULT は元の JSON bytes のまま扱い、既存の重複キー拒否と guest-v1 envelope を検証する。data は次の完全な形とし、未知フィールド・型違い・false・長さ不一致・空/過大な形式名・形式版0を拒否する。

```json
{"valid":true,"prepared":true,"artifactFormat":"dictionary-index","artifactVersion":1,"artifactBytes":1234}
```

形式名は1〜128 bytesの lowercase ASCII英数字と `._-`。Host は名前から辞書形式を解釈したりファイルを開いたりしない。消費 Node と固定公式構成は別途、実際に対応する形式・版を照合する。

CHUNK の全 bytes・RESULT・両方向 EOF が正しく、元の process handle で実終了を確認し、終了理由 Exited・exit code 0・IO 回収が揃い、現在の許可を保つ場合だけ `PreparedArtifact` を返す。コード/定義/設定の identity、原本版、成果 bytes/形式/版/hash を保持する。成果を Wasmtime コンパイル済みコードや Host の辞書オブジェクトへ復元しない。

正しい RESULT の後の exit 73、不正 JSON、長さ違い、途中失効、期限切れ、終了未確認では成果を返さない。最終 hash と Arc 化の前後にも現在性を確認する。計画を有効化する時点では、統合側が改めて現在の許可と全準備の成功を検証する。

## 検査と未接続部分

単体で上限・順序・chunk/ack・全位置での切断・原本版の変化・読取専用モードとの区別を検査する。`scripts/check-wasm-v3.ps1` は固定 DLL と実 native Worker を用い、AppContainer で17ケースを明示実行する。通常の Wasm fixture、成果途中の取消/失効/保護、期限、上限、壊れた最終返答と、独立した native crash fixture を含む。正しい返答後に実際に exit 73 で終了した成果を採用しないことを確認する。終了未確認時は元の原本を保持し、同じ owner の retry による実終了後の回収を確認する。

fixture は通信と所有の検査であり、正式 Node の辞書全件検証を実装したとは扱わない。正式 Node/SDK は本体契約・検査を整えた後の別 Issue/PR。既存 v1 module に未対応 import を混ぜず、通常経路との互換性を維持して固定公式の両対応を検証する。通常 Host の選択・原子的切替、保持 Worker の物理会計、実配布版の正答と性能は [#61](https://github.com/pitapa79/DIY_IME/issues/61) / [#27](https://github.com/pitapa79/DIY_IME/issues/27) の残作業とする。
