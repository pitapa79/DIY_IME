// 実験ノードの表示情報。正式採用は後で判断する。
export const experimentalMetadata = {
  "experimental.text-case": {
    "title": "大文字・小文字変換",
    "description": "文字の大文字・小文字を変換",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.text-trim": {
    "title": "空白除去",
    "description": "前後・全体の空白を除去",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.whitespace-collapse": {
    "title": "連続空白整理",
    "description": "連続する空白をまとめる",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.line-endings": {
    "title": "改行処理",
    "description": "改行の形式をそろえる",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.text-extract": {
    "title": "文字列抽出",
    "description": "開始・終了位置を指定して抽出",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.text-prefix": {
    "title": "前方抽出",
    "description": "先頭から指定文字数を取得",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.text-suffix": {
    "title": "後方抽出",
    "description": "末尾から指定文字数を取得",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.character-filter": {
    "title": "文字種フィルター",
    "description": "英字・数字・空白を抽出または除去",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.json-escape": {
    "title": "エスケープ・復元",
    "description": "JSON文字列のエスケープ・復元",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.hiragana-katakana": {
    "title": "ひらがな→カタカナ",
    "description": "ひらがなをカタカナへ変換",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.katakana-hiragana": {
    "title": "カタカナ→ひらがな",
    "description": "カタカナをひらがなへ変換",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.text-replace": {
    "title": "文字列置換",
    "description": "文字列をまとめて置換",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.text-split": {
    "title": "文字列分割",
    "description": "区切り文字で文章を要素に分ける",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.choice-deduplicate": {
    "title": "重複候補削除",
    "description": "最初の候補を残して重複を除去",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.choice-top-n": {
    "title": "上位N件抽出",
    "description": "上位N件抽出を適用",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.choice-sort": {
    "title": "候補並べ替え",
    "description": "候補並べ替えを適用",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.choice-limit": {
    "title": "候補数制限",
    "description": "候補数制限を適用",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.choice-threshold": {
    "title": "スコア閾値フィルター",
    "description": "スコア閾値フィルターを適用",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.choice-remove": {
    "title": "候補削除",
    "description": "候補削除を適用",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.score-set": {
    "title": "スコア付与",
    "description": "スコア付与を適用",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.score-add": {
    "title": "スコア加算",
    "description": "スコア加算を適用",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.score-multiply": {
    "title": "スコア乗算",
    "description": "スコア乗算を適用",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.score-normalize": {
    "title": "スコア正規化",
    "description": "スコア正規化を適用",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.choice-extract": {
    "title": "Choice展開",
    "description": "Choice展開を適用",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.sequence-extract": {
    "title": "Sequence展開",
    "description": "Sequence展開を適用",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.packet-passthrough": {
    "title": "パススルー",
    "description": "パススルーを適用",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.packet-depth-limit": {
    "title": "候補木の深さ制限",
    "description": "候補木の深さ制限を適用",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.packet-rebuild": {
    "title": "候補木再構築",
    "description": "候補木再構築を適用",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.text-concat": {
    "title": "文字列連結",
    "description": "区切り文字で2つのTextを連結",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.choice-build": {
    "title": "Choice生成",
    "description": "設定と任意入力をChoiceとして保持",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.choice-merge": {
    "title": "候補統合",
    "description": "Choiceを順序を保って連結",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.sequence-build": {
    "title": "Sequence生成",
    "description": "設定と任意入力をSequenceとして保持",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.sentence-split": {
    "title": "文分割",
    "description": "終端記号の連続群の後で分割",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.ascii-width": {
    "title": "ASCII全角・半角",
    "description": "ASCII英数記号と空白の幅を切替",
    "category": "実験",
    "symbol": "experiment"
  },
  "experimental.japanese-punctuation": {
    "title": "句読点変換",
    "description": "日本語句読点の表記を切替",
    "category": "実験",
    "symbol": "experiment"
  }
};
