# §727. CJK text in its region's typeface (2026-11-21)

We bundle no font. Slint asks the system for `[SystemUi, SansSerif]`
and sets no locale on its text, so on Linux fontconfig hands every Han
string the first CJK face in its sorted list, whatever the language:
on the machine we tried, the Korean cut of Noto Sans CJK for Japanese
and Chinese alike. A native reader sees it at once on 直 and 骨. With
no CJK font installed the text is boxes. Windows and macOS fall back
per script themselves.

Bundling the regions' fonts would fix both and cost 15 to 35 MB in
every download for two languages. Instead, when the window's language
is Japanese, Simplified or Traditional Chinese, or Korean,
`i18n::face::apply` rewrites the generic family lists in Slint's shared
fontique collection, once, before anything is drawn (Slint keeps shaped
text and does not notice fonts changing):

- It finds the first family in those lists that draws the language.
  One from this region's table is left. One from another region's
  table gets this region's face put before it. One no table knows is
  the user's own choice and is left, unless it maps only some of the
  region's sample characters (a Chinese-only font ranked first for
  Japanese), when it counts as another region's.
- Where no family draws the language, the face goes at the end, which
  is the case on Windows and macOS, whose generic lists hold no CJK
  family.
- Families ahead of the inserted one keep their place, so Latin text is
  drawn exactly as before, and every other language returns before the
  collection is touched.

The face is the first of a table per platform that is installed and
maps every sample; names are matched by family id, which follows
aliases and localized names. The samples (a kokuji for Japanese, a
simplified form, a Big5 character, a hangul syllable) separate the
legacy character sets; nothing separates fonts wider than those, so for
pan-CJK families the tables decide. With no font covering the
language, the log says so, and on Linux names Noto Sans CJK and its
packages.

The `unstable-fontique-012` feature this uses only re-exports what the
renderer already builds; a test holds our copy of Slint's generic list
to Slint's own, so a Slint bump that changes either fails to build or
fails that test.

Left: Windows and macOS are read, not run. The region follows the
window's language only, so a Japanese user with the window in English
still sees the system's pick for their folder names. The watermark has
its own font collection and is not covered.
