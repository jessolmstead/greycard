# Japanese (ja) glossary for greycard's window

This is the glossary of a draft translation that no native speaker has
read yet. Corrections to it and to the catalog are welcome.

Source of the terms: the Japanese Lightroom Classic (LrC) first,
Capture One (C1) and darktable (dt) where LrC has none or they differ.

## Conventions

- **Register.** A sentence that speaks to the user is plain polite
  (〜します, 〜できません, 〜してください). Buttons, menu items, and
  labels are nouns or a verb's dictionary or noun form, as Japanese
  desktop software writes them ("書き出し", "フォルダーを開く...",
  "すべてリセット"). Status lines say what happened in the polite past
  ("{n}枚を読み込みました"); one still running ends in 中
  ("書き出し中..."). A reason fragment that fills an error ("reading
  it: {e}") is a noun phrase in 時 ("読み取り時：{e}").
- **Punctuation.** Full-width 、。 in sentences, full-width colon ：
  after Japanese words, full-width parentheses （） around Japanese,
  corner brackets 「」 around a control's name or a user's name for a
  thing ("「バックアップ」で確認します", "ルック「{}」"). Half-width
  digits, Latin, and the source's "..." (menus and running states keep
  three dots, as Japanese desktop software does). A question ends in
  ？. A status line ends without 。 where the English has no period.
- **Spaces.** None between Japanese and Latin or digits
  ("Lightroomカタログ", "{n}枚"). Kept where the source has UI
  structure: a key before its word ("P 採用 · X 除外"), a number before
  a unit symbol ("0.5 MB", "{size} KB"), and the " · " and " — "
  separators.
- **Long vowels.** The current JIS / Microsoft style: フォルダー,
  ユーザー, ブラウザー, フィルター, モニター, コンピューター.
  Words Microsoft keeps short stay short: ライブラリ,
  ディレクトリ, メモリ.
- **Headings.** The develop panel's ALL CAPS headings are plain
  Japanese, which has no capitals ("WHITE BALANCE" → ホワイトバランス).
- **Counters.** 枚 for frames, pictures, and photos ("{n}枚"); 個 for
  things (files, sidecars, folders, groups, presets); 件 for records
  and entries (sets, edits, failures). One plural form
  (nplurals=1; plural=0); where English's singular and plural differ in
  more than the count, the form that reads right for both was taken.
- **Placeholders.** Reordered as Japanese needs; a `{}` that moves is
  written `{0}`, `{1}`.
- **Joined fragments.** A tail the English joins with ", " starts with
  、; one joined with "; " or ". " starts with 。; neither keeps the
  English's leading space. A few joins are built in code with an
  English "; " the catalog cannot reach (the delete's refusal, the
  archive copy's report, the sync note, the camera match's replace
  warning): those read "; " in Japanese. Lists of names are joined with
  ", " in code the same way.
- **Product words.** greycard, Lightroom, RawTherapee, darktable,
  lensfun, GitHub, JPEG, TIFF, PNG, DNG, ICC, EXIF, XMP, RGB, AgX, file
  names, units (MB, KB, GiB, px, EV, K, mired), and the search's tokens
  (`camera:`, `iso>=`) stay as they are. RAW is written in capitals,
  as Japanese camera makers do. Seconds are 秒 ("{seconds}秒"), not
  "s", as LrC writes them.
- **Keys.** Key names stay as the keys sheet draws them (Ctrl, Shift,
  Alt, Space, Enter, Esc), as docs/translating.md asks. The pointer's
  word is translated ("Ctrl+クリック"). Key ranges use the full-width
  wave dash ("1～5、0").
- **Numbers and dates** keep the English form (§616).
- **Frame / picture.** A frame (a file in the library) is 写真, counted
  枚; a picture (what is drawn, the image itself) is 画像.

## Terms

| English | Japanese | Why / alternative |
| --- | --- | --- |
| Library | ライブラリ | LrC module name |
| Develop | 現像 | LrC module name |
| Loupe | ルーペ | LrC "ルーペ表示" |
| Lightbox | ライトボックス | standard |
| Filmstrip | フィルムストリップ | LrC |
| Grid | グリッド | LrC "グリッド表示" |
| Panel / pane | パネル / ペイン | LrC |
| Fit (zoom) | 全体 | LrC navigator "全体"; "全体表示" in sentences |
| Exposure | 露光量 | LrC |
| Contrast | コントラスト | LrC |
| Highlights | ハイライト | LrC |
| Shadows | シャドウ | LrC |
| Midtones | 中間調 | LrC Color Grading |
| Whites / Blacks | 白レベル / 黒レベル | LrC |
| LIGHT (the section) | ライト | LrC's mask "ライト" group; parametric Lights is ライト too (LrC), in another section |
| Clarity | 明瞭度 | LrC |
| Texture | テクスチャ | LrC |
| Dehaze | かすみの除去 | LrC |
| Vibrance | 自然な彩度 | LrC |
| Saturation | 彩度 | LrC |
| White balance | ホワイトバランス | LrC |
| Temperature / Tint (WB) | 色温度 / 色かぶり補正 | LrC |
| As shot | 撮影時の設定 | LrC |
| Daylight, Cloudy, Shade, Tungsten, Fluorescent, Flash | 昼光, 曇天, 日陰, 白熱灯, 蛍光灯, フラッシュ | LrC |
| Neutral (WB dropper) | ニュートラル | |
| TINT (the section that lays a color over) | 色付け | kept apart from WB's 色かぶり補正; alt. "ティント", "着色" |
| Tone curve / Curves | トーンカーブ / カーブ | LrC |
| Lights / Darks (parametric) | ライト / ダーク | LrC |
| Point / Parametric | ポイント / パラメトリック | LrC |
| Color mixer | カラーミキサー | LrC |
| Hue / Luminance | 色相 / 輝度 | LrC |
| Red, Orange, Yellow, Green, Aqua, Blue, Purple, Magenta | レッド, オレンジ, イエロー, グリーン, アクア, ブルー, パープル, マゼンタ | LrC color mixer and color labels |
| Color grading | カラーグレーディング | LrC |
| Balance (grading) | バランス | LrC |
| Black & white | 白黒 | LrC |
| Weight (B&W filter) | ウェイト | none in LrC; alt. "比重" |
| Sharpen | シャープ | LrC "シャープ" |
| Output sharpen | 出力シャープ | LrC export "出力シャープ" |
| Amount | 適用量 | LrC |
| Radius / Detail / Threshold | 半径 / ディテール / しきい値 | LrC |
| Noise / Denoise | ノイズ / ノイズ除去 | LrC (AI ノイズ除去) |
| Luma / Chroma | 輝度 / クロマ | LrC says 輝度 / カラー for noise; クロマ (dt) because the same word is also the color range's chroma floor, where カラー would mislead |
| Structure (Detail's widest band) | 構造 | Texture is already テクスチャ |
| Grain | 粒子 | LrC |
| Cubic / Tabular (grain) | 立方体 / 平板 | film's cubic and T-grain; alt. "立方晶" |
| Vignette (the effect) | ビネット | C1; LrC "切り抜き後の周辺光量補正" is long and collides with the lens's |
| Midpoint / Feather / Roundness | 中心点 / ぼかし / 丸み | LrC |
| Lens corrections | レンズ補正 | LrC |
| Chromatic aberration | 色収差 | LrC |
| Defringe | フリンジ軽減 | LrC |
| Distortion / Vignetting (lens) | ゆがみ / 周辺光量 | LrC |
| Scale / Auto scale | スケール / 自動スケール | LrC "拡大・縮小" is longer |
| Crop | 切り抜き | LrC |
| Aspect | 縦横比 | LrC |
| Original (aspect) | 元画像 | LrC "元の画像" |
| Rotate | 回転 | LrC |
| Turn (a quarter, not part of the edit) | 向き / 左に向ける | kept apart from Rotate (回転, part of the edit) |
| Level | 角度補正 | LrC "角度補正ツール" |
| Flip / Flip vertical | 左右反転 / 上下反転 | LrC; plain 反転 is Invert |
| Perspective | 遠近補正 | LrC's panel is "変形"; alt. "キーストーン" (C1) |
| Guides: Thirds, Golden ratio, Golden spiral, Diagonals, Triangles | 3分割, 黄金比, 黄金螺旋, 対角線, 三角形 | LrC crop overlays |
| Mask / Adjustment | マスク / 補正 | LrC |
| Shape | シェイプ | LrC calls them "コンポーネント"; alt. that |
| Linear / Radial | 線形 / 円形 | LrC "線形グラデーション / 円形グラデーション", shortened as the English is |
| Brush / Paint | ブラシ / ペイント | LrC |
| Feather / Flow / Density | ぼかし / 流量 / 密度 | LrC |
| Erase | 消去 | LrC "消去" |
| Invert / Intersect / Subtract / Add | 反転 / 交差 / 減算 / 追加 | LrC |
| Refine edge | 境界線を調整 | Photoshop |
| Subject / Sky / Background / People / Object / Landscape | 被写体 / 空 / 背景 / 人物 / オブジェクト / 風景 | LrC |
| Color range / Luminance range | カラー範囲 / 輝度範囲 | LrC |
| Retouch | レタッチ | standard |
| Heal / Clone / Fill | 修復 / コピースタンプ / 塗りつぶし | LrC removal tool, Photoshop |
| Best / Fast / Patch (fill quality) | 高品質 / 高速 / パッチ | |
| Look | ルック | photographers' word; LrC "プロファイル" means something else here |
| Camera profile | カメラプロファイル | LrC |
| Camera match / Fit (a look) | カメラマッチ / フィット | none in LrC; alt. "適合" |
| Display curve | トーンマッピング | none in LrC; alt. "表示カーブ" |
| Highlight fill | ハイライト補完 | none in LrC; alt. "白とび補完", dt "ハイライト再構築" |
| Demosaic | デモザイク | dt |
| Preset | プリセット | LrC |
| Reset | リセット | LrC |
| Snapshot | スナップショット | LrC |
| History | ヒストリー | LrC |
| Restore (a snapshot) / Bring back (from an archive) | 復元 | the same word for both; they never share a menu |
| Virtual copy / Version | 仮想コピー / バージョン | LrC |
| Sidecar | サイドカー | LrC "XMP サイドカーファイル", shortened |
| Cull / Culling | 選別 | alt. "セレクト" |
| Rating / stars | レーティング / 星 | LrC; "星{n}つ" |
| Flag / Pick / Reject / Unflagged / Unflag | フラグ / 採用 / 除外 / フラグなし / フラグを解除 | LrC says "フラグ付き" for Pick, which reads as any flag; 採用 is the cull word |
| Rejects (the frames, the folder) | 除外した写真 / 除外フォルダー | |
| Label | ラベル | LrC "カラーラベル", shortened |
| Keyword | キーワード | LrC |
| Stack | スタック | LrC |
| Collection / Smart collection / Collection set | コレクション / スマートコレクション / コレクションセット | LrC |
| Root (a library root) | ルートフォルダー | alt. "ソース" |
| Archive / Back up | アーカイブ / バックアップ | |
| Export / Import | 書き出し / 読み込み | LrC |
| Add in place (import) | 追加 | LrC's import "追加" |
| From / To (import rows) | 読み込み元 / 読み込み先 | |
| Copy / Paste settings | 設定をコピー / 設定をペースト | LrC |
| Sync | 同期 | LrC |
| Merge to HDR / Panorama / Focus stack | HDR合成 / パノラマ合成 / 深度合成 | LrC "写真を結合" menu; 深度合成 is the common Japanese word |
| Soft proof | ソフト校正 | LrC |
| Gamut warning | 色域外警告 | LrC |
| Intent / Perceptual / Relative | インテント / 知覚的 / 相対的 | LrC |
| Long edge | 長辺 | LrC |
| If it exists / Increment / Overwrite / Skip | 既存の場合 / 連番を付ける / 上書き / スキップ | |
| Watermark | 透かし | LrC |
| Border | 枠 | alt. "フチ" |
| Metadata / Title | メタデータ / タイトル | LrC |
| Capture time | 撮影日時 | LrC |
| Hand order | ユーザー順序 | LrC |
| Navigator | ナビゲーター | LrC |
| Canvas | キャンバス | LrC "背景色"; 背景 is the mask's Background |
| Card | カード | memory card |
| Trash | ゴミ箱 | Windows and GNOME |
| Settings / About | 設定 / 情報 | |
| Interface (Settings' category) | 外観 | "インターフェイス" clipped in the Settings sheet's category list |
| Scroll bar size | バーの太さ | "スクロールバーのサイズ" clipped in the Settings sheet's label column |
| Fit this camera's look... | ルックをフィット... | the longer wording squeezed the Look section's Reset button |
| Start in / Cell size / Edits go (Settings' 68 px label column) | 起動時 / セルサイズ / 保存先 | the full wordings ("起動時のフォルダー", "セルのサイズ", "編集の保存先") clipped |
| Edit in (the row and the frame menu's submenu) | 外部編集 | "外部アプリで編集" clipped in the 68 px column; the menu item "Edit in..." keeps "外部アプリで編集..." |
| Keys (the sheet) | キー操作 | alt. "ショートカットキー" |
| Undo / Redo | 取り消し / やり直し | |
| Model (AI) | モデル | |
| Log | ログ | |
