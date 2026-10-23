# §401. Settings in five categories

*2026-10-23*

The Settings sheet was one scroll of nine headed blocks, the keys
table (§390) longest of all, and more settings are coming: a theme,
the folder to open at start, the filter bar hidden. A tester asked
for the sticky choices gathered somewhere findable. The sheet gets
five categories down its left side, one shown at a time.

## The split

| Category | Holds |
|---|---|
| Interface | Scale, one section open |
| Library | Sidecars, RAW+JPEG, Thumbnails, Network folders |
| Editing | Crop aspect, Presets |
| Keys | the keys table, Reset to defaults |
| About | Updates, the version, where greycard keeps its files |

Every row moved as it was, with its wiring; the diff is a move, not
a rewrite, and the reviewer checked each block against master with
whitespace stripped. About's folders come from the Rust side, which
knows them, and are the ones the guide's table lists, so a tester
can read them off the sheet instead of the guide.

## Shape

The column is 132 px; the sheet grows by that only, to 572 px. Every
category is built and toggled with `visible`, so the sheet is as
tall as its tallest category (Keys) and keeps its size from one to
the next; a hidden category's rows are skipped by Tab and never fire
a callback, which the pinned Slint guarantees for an invisible
subtree. The keys table scrolls inside its category. The last
category shown is kept in settings.json, read leniently, so a bad
name costs nothing else.

## Focus

The column holds the keys: Up and Down step categories, Ctrl+Tab and
Ctrl+Shift+Tab do too, from the column or from inside a category's
rows, and every switch puts the focus back on the column, since a
focus left on a row that just turned invisible is dropped by Slint
on the next key and the keyboard goes dead until a click. The card's
scope does not take focus on a click, so a click on blank card
neither steals the column's keys nor stops a key row that is
listening for a chord. After a chord is bound the sheet, not the
window, takes the focus back, and swallows the bound key's repeats
itself; the window's own guard still covers a key held while the
sheet closes. Three review passes found each of these in turn.

## Not done

- The focus chain is not trapped in the sheet: Shift+Tab from the
  column or Tab past Done reaches the panels behind it. Escape still
  closes the sheet from there.
- On Windows the Library and Caches rows show the same folder, which
  is true.
