# 想定できる IME 構成例

この文書は、DIY_IME 上で**どのような IME を構成できるようにしたいか**を示すための設計例である。

> **重要:** ここに登場するノードを標準ノードとして実装することを決定した文書ではない。  
> ノード名・分割方法・データ型・接続方法も概念上の例であり、正式仕様ではない。  
> DIY_IME Core の目標は、これらのノードそのものを持つことではなく、**ユーザーやノード作者が必要な部品を用意すれば、このような多様な構成を自然に表現・実行できる基盤を作ること**にある。

また、以下は推奨プリセットや「正しい IME」の一覧でもない。DIY_IME Core の表現力を検討するときの代表例として使う。

## 1. 最小限の実用的な日本語 IME

```text
Keyboard Input
  ↓
Romaji → Kana
  ↓
Dictionary
  ↓
Candidate Selection
  ↓
Output
```

一般的なローマ字入力とかな漢字変換を行う軽量構成。

[性能目標](performance-goals.md)で定義する Reference Minimal Practical IME は、この種類の構成を基準にする。ただし正式な辞書・候補処理・測定構成は、それぞれの仕様が固まった段階で決める。

## 2. SKK 型 IME

```text
Keyboard Input
  ↓
Romaji → Kana
  ↓
SKK State / Command Processing
  ↓
SKK Dictionary
  ↓
Candidate Selection
  ↓
Output
```

一般的な逐次かな漢字変換とは異なる入力方式も、Core 側の固定された変換モデルに合わせず構成できることを示す例。

## 3. LLM を候補評価に使う IME

```text
Keyboard Input
  ↓
Romaji → Kana
  ↓
Dictionary
  ↓
Candidate Generation
  ↓
Context
  ↓
LLM Reranker
  ↓
Output
```

辞書変換を置き換えるのではなく、候補順位付けなど特定部分だけに LLM を追加する構成例。

LLM を使う場所や役割は Core が決めず、IME 構成側が選べることを目指す。

## 4. 重い処理をフォールバック付きで使う IME

```text
Input
  ↓
Dictionary ─────────────────→ Fallback Result
  ↓
Heavy Processing / LLM
  ↓
Elapsed Time / Timeout
  ↓
time limit 内 ──→ Heavy Result
time limit 超過 ─→ Fallback Result
```

重い処理そのものを禁止せず、時間の観測、タイムアウト、分岐、フォールバックなどを構成側から扱う例。

これは[設計思想](design-philosophy.md)の「制限するのではなく、制御するための手段を提供する」を具体化した構成の一例である。

## 5. 独自配列・独自入力規則を持つ IME

```text
Keyboard Input
  ↓
Custom Key Mapping
  ↓
Custom Input Rule
  ↓
Kana / Text
  ↓
Dictionary
  ↓
Output
```

QWERTY ローマ字入力を前提にせず、独自キー配列や独自の文字対応・入力規則を前段へ配置する例。

入力方式を Core の固定仕様にせず、構成要素として交換できることを示す。

## 6. 文章変換を強く組み込む IME

```text
Input
  ↓
Kana / Kanji Conversion
  ↓
Grammar Correction
  ↓
Style Conversion
  ↓
Terminology Replacement
  ↓
Output
```

従来のかな漢字変換だけでなく、校正・文体変換・用語置換などを入力フローへ組み込む例。

DIY_IME が「通常の日本語 IME」の処理順だけを表現できる専用パイプラインにならないための確認例でもある。

## 7. 入力源そのものを交換する構成

```text
Keyboard ───────┐
Voice ──────────┤
Eye Tracking ───┼→ Input Processing → Conversion → Output
External Device ┘
```

キーボードだけでなく、将来的に別の入力源をノードやアダプターとして接続する場合の概念例。

これらの入力方式を実装することを現時点で決定しているわけではない。Core が将来の入力源を不必要に閉ざさないか考えるための例である。

## 設計判断への使い方

新しい Core API、データ型、実行方式、状態管理、ノード境界などを設計するとき、これらの例を使って次を確認できる。

- 特定の変換方式だけを前提にしていないか
- ノードの順序や役割を Core が不必要に固定していないか
- 分岐、複数入力、状態、時間、失敗などを必要に応じて表現できるか
- 軽量な構成と巨大な構成の両方を扱えるか
- 将来の新しい処理を追加するために Core 全体の再設計が必要にならないか

すべての例を最初から完全に実装できることを v0.1 の完成条件にはしない。  
ただし、Core の基本設計を決める際には、**これらのような構成を原理的に排除する制約を不用意に作っていないか**を確認する。
