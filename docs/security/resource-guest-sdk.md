# 資源範囲取得の Guest SDK v1

[#68](https://github.com/pitapa79/DIY_IME/issues/68)。[範囲取得](resource-range-transport.md)と[準備成果回収](prepared-resource-capture.md)を使う Guest 側の共通部品。正本は本体の `crates/wasm-resource-sdk`、package は `diy-ime2-wasm-resource-sdk`。辞書の形式・全件検証・索引・候補の意味を含まない。Rust は実装例であり、他言語は同じ版付き ABI に適合すればよい。

この段階は製品経路への導入前。通常 Host の選択、固定公式コードと各設定への結合、Node 側の実装・固定 import、原子的計画切替、保持 Worker と実用性能は後続。仕様事前承認は [#53](https://github.com/pitapa79/DIY_IME/issues/53)、Level 3 のマージ承認は別途とする。

## 役割と既存 v1 の共存

SDK の既定 feature は `serde`。読み出しに `diyime_resource_v1.read` を使い、`artifact-output` を明示した producer だけが `diyime_prepared_v1.emit` と `ArtifactWriter` を使える。Host の読取モードは emit import 自体を登録しない。

構築と消費は別々の Wasm module にビルドする。producer は構築専用モード、consumer は読取専用モードへ渡す。両操作の実装を1 moduleへ混ぜ、未使用の emit import を読取用 Worker に要求しない。Node 側の導入では既存の全ノード v1 module を維持し、対応する構築・消費 module を別成果として固定する。

guest の既存呼出し exports と `apiVersion:1` の envelope は変えない。既存 `describe / validate / prepare / execute` をこの SDK の操作で置き換えず、対応する経路を信頼側が明示選択する。package の自己申告で新モードや公式扱いを有効にしない。describe/config 検証の経路と、構築・消費コードの対応も統合側が照合する。

## 構築要求

追加操作は `prepareArtifact`。以下は共通 ABI の検査用 fixture 例であり、正式な辞書 Node の config の定義ではない。

```json
{
  "apiVersion": 1,
  "node": {"kind": "fixture.resource", "version": 1},
  "operation": {
    "method": "prepareArtifact",
    "config": {},
    "resources": {"source.bin": {"id": 1, "length": 8}}
  }
}
```

resources は論理参照から descriptor への map。参照の重複（escape 等価も含む）、異なる参照での同一 ID、空の集合を拒否する。名前は native path に使わない。SDK の descriptor は `id:u32 / length:u64`。ID は正の i32 範囲、length は非負の i64 範囲とし、空の原本も表現できる。

型付き decode の後に `Request::validate` を呼ぶ。ヘッダーの API 版・Node識別、descriptor の形を確認した上で、Node 自身が対応する kind/version と正確な config、原本全件の意味を検証する。SDK の generic な config/inputs 型は、Node 固有の schema・Packet・重複キー等の検証の代わりにはならない。

原本と同一設定の全件検証・構築が成功してからだけ、`PreparationAcknowledgement::new` を使って次の data を返す。SDK の型は Host の最終確認にも共有し、双方の形を別定義にしない。

```json
{"valid":true,"prepared":true,"artifactFormat":"fixture.sdk-bytes","artifactVersion":1,"artifactBytes":8}
```

成果は非空で u32 範囲内、形式名は1〜128 bytesの lowercase ASCII英数字と `._-`、形式版は正。flags・形・版が正しくても、Host は全 chunk・実終了・IO 回収・許可の現在性と実長を確認するまで成果として返さない。

## 消費要求

追加操作は `executePrepared`。既存 execute と同じ config / inputs / mode の意味を保ち、資源全体の本文を毎入力の JSON に加えない。

```json
{
  "apiVersion": 1,
  "node": {"kind": "fixture.resource", "version": 1},
  "operation": {
    "method": "executePrepared",
    "config": {},
    "artifact": {
      "resource": {"id": 1, "length": 8},
      "format": "fixture.sdk-bytes",
      "version": 1
    },
    "inputs": {},
    "mode": "finalize"
  }
}
```

artifact は準備済み bytes の descriptor と形式/版。空の成果、形式・版の形違いを拒否する。消費 Node は実際に対応する形式/版と内部データを確認する。Host は当該コード・Node定義・正確な config・原本版・計画/文脈と成果を結び付ける。guest の自己申告はその proof ではない。

inputs は既存ポート値と同じ。Data / NoOutput / Error、Packet の検証は既存契約を維持する。mode は省略、`compose`、`finalize` だけを受け付け、明示 null と未知値を拒否する。全文 RuntimeInput、revision、RunKey、許可 token、resource root は追加しない。実行結果は既存の guest-v1 outputs envelope と最終検証へ渡す。

追加要求・Node識別・descriptor・acknowledgement は未知フィールドを拒否する。Node の version と API の version、資源形式の version を混同しない。

## 安全な Rust API

`Resource::try_from(ResourceDescriptor)` は guest 内の形を検査する。`read_exact(offset, length)` は1〜65,536 bytesだけを読み、checked 加算と descriptor の終端を検査する。返却長が違えば error とし、短い成功・暗黙の再試行へ変換しない。native callback が独立に実際の資源範囲、ID、予算、認可を検査するため、偽の descriptor は権限にならない。

`artifact-output` の `ArtifactWriter` は `std::io::Write` として最大65,536 bytesずつ送り、ACK 後だけ書込済み長を進める。失敗した writer は以後の write/flush を拒否し、成果は u32 範囲を超えない。空の write は標準の Write と同じく0を返し、native の空 CHUNK を作らない。

native の失効・拒否は trap で invocation 自体を終える場合がある。SDK の Result だけでそれを回復しようとしない。wasm32 以外に OS/ファイル経路の fallback を用意せず `UnsupportedTarget` とする。WASI やファイル・ネットワーク権限は追加しない。

## 固定 import と検査

Node 側へは、レビューした本体 commit と SDK の各 source blob、Cargo manifest と依存関係、適用 feature を記録して固定 import する。`sdk/runtime` を改造したり、改変版を未変更 snapshot と呼んだりしない。本 SDK は Core の Node trait や実行エンジンから独立しており、個別処理を本体へ戻さない。

`scripts/check-resource-sdk.ps1` は全 feature / 最小 feature の検査、固定依存の producer/consumer Wasm build と clippy、実 native Worker/AppContainer の接続を行う。通常の Windows CI は既存の v1 検査とともにこれを実行する。

実 fixture は UTF-8、NUL、非UTF-8 bytes を含む8 bytesと65,553 bytesを正確に往復し、複数 READ/CHUNK を通す。API版、重複参照、偽ID/終端、null mode と role 違いを拒否する。producer が読取用 Worker で instantiate できないことは、単なる操作エラーと区別して検査する。fixture は正式な処理 Node ではなく、辞書全件検証や実用性能を達成したとは扱わない。
