# Translating greycard

greycard's window can be shown in any language there is a catalog
for. A catalog is one file, in gettext's `.po` format, that every
gettext editor opens (Poedit, Lokalize, GTranslator, or a text
editor). The builds carry the catalogs, so a new language reaches
users with the next release.

## Adding a language

1. Make `translations/<tag>/LC_MESSAGES/greycard.po`, where `<tag>`
   is the language's code: `de`, `es`, `ja`, or a language and region
   such as `pt_BR` when the region's wording differs. Either let
   gettext make it,

   ```sh
   msginit -l <tag> -i translations/greycard.pot \
       -o translations/<tag>/LC_MESSAGES/greycard.po
   ```

   or copy `translations/greycard.pot` there and delete the
   `#, fuzzy` line above its header.
2. Fill in the header: `Language: <tag>`, your name in
   `Last-Translator`, and the language's `Plural-Forms` line
   ([gettext's list](https://www.gnu.org/software/gettext/manual/html_node/Plural-forms.html)
   has every language's). French's is
   `nplurals=2; plural=(n > 1);`.
3. Translate. An entry left empty shows in English, so a catalog can
   be sent half done. An entry marked fuzzy also shows in English
   until the mark is cleared.
4. Build and run greycard, pick the language under Settings >
   Interface > Language, and start it again to see the window in it.
5. Open a pull request with the one file. If your language's name in
   itself is not yet in the Language list, say so in the pull request.

greycard bundles no font. Japanese, Chinese, and Korean take their
region's typeface from the system's fonts, so shared ideographs keep
their own forms. Windows and macOS ship these fonts; Linux needs Noto
Sans CJK installed, from your distribution's package for it
(`noto-fonts-cjk` on Arch, `fonts-noto-cjk` on Debian and Ubuntu,
`google-noto-sans-cjk-fonts` on Fedora), or the text shows as boxes.

To update a catalog after the window's wording changed, run
`scripts/translations.sh` (see [building.md](building.md)) or ask in
the pull request: new strings arrive empty and changed ones fuzzy.

To see how far a catalog is behind, run
`scripts/translations-check.py --list` (Python 3, nothing to
install). It lists each language's missing, empty, fuzzy, and
obsolete entries, and fails on a translation whose placeholders
differ from the English's. It also checks the template against the
source, and lists words the window may show that nothing marks for
translation; `translations/untranslated.txt` holds the ones that are
data or keys. CI runs it without `--list`.

## Glossaries

The German, Spanish, Italian, Dutch, Polish, Brazilian Portuguese,
Japanese, Korean, Simplified Chinese, and Traditional Chinese catalogs
are drafts no native speaker has read yet. Each has a glossary beside it,
`translations/<tag>/glossary.md`: the word chosen
for each of the editor's terms, where it came from, and the
alternatives, with the conventions the catalog keeps (how a button
speaks, capitals, keys, and plurals). A reviewer can see there why a
word was chosen, and correct it there and in the catalog together. A
new translator is welcome to add one too.

## Where a string shows

Each entry names the file it comes from (`#:`), and many carry a
note (`#.`) saying where in the window it shows and what it does.
The file alone places most of them:

| File | Where it shows |
| --- | --- |
| `ui/app.slint` | The window itself: the menus, the top bar, the asks |
| `ui/controls.slint` | Shared controls: thumbnails, sliders, scroll bars, mostly words a screen reader speaks |
| `ui/panel/edit.slint`, `color.slint`, `curve.slint`, `lens.slint`, `crop.slint`, `retouch.slint`, `mask.slint`, `assets.slint` | The develop panel's sections, in that order top to bottom; a heading in capitals is a section's title |
| `ui/panel/cull.slint` | The Culling section, in the Loupe's Cull mode |
| `ui/panel/deliver.slint` | The export sheet and the export queue |
| `ui/panel/settings-sheet.slint` | The Settings sheet, by category |
| `ui/panel/*-sheet.slint` | The other sheets, named after what they do |
| `ui/panel/grid.slint`, `strip.slint`, `filter.slint`, `folders.slint`, `roots.slint`, `collections.slint`, `recent.slint`, `navigator.slint` | The Library: the grid, the filmstrip, the filter bar, and the left pane's lists |
| `ui/panel/frame-menu.slint` | A frame's right-click menu |
| `ui/panel/history.slint`, `presets.slint`, `meta.slint`, `scopes.slint` | The other sections: history and snapshots, presets, metadata, and the scopes |
| `src/panel/*.rs` | The status line at the bottom and a sheet's own sentences, for the part the file is named after |
| `src/keys.rs` | The keys sheet: each action's words and the headings |
| `src/rows.rs`, `src/tags.rs`, `src/sort.rs`, `src/filter.rs` | The status line's words for a change of rating, flag, label, order, or filter |

A few shapes to know:

- A **heading in capitals** (`WHITE BALANCE`) is a section's title in
  the develop panel. Keep the capitals if your language uses them for
  that; otherwise write it as your language writes a heading.
- An entry starting with a **comma, semicolon, or space** (`, {}
  backed up`) is a tail joined onto another sentence. Keep the
  punctuation and the space.
- A note saying **spoken by a screen reader** is never drawn; it is
  what a screen reader says for a control.
- **Key names** (`Esc`, `Ctrl`, `Shift`) and the **search's own
  words** (`camera:`, `iso>=`) stay as they are.

## Placeholders

A `{}` in a string is a value the window fills in, in order;
`{name}` is one by name, which a translation may move:

```po
msgid "Exporting {done} of {total}"
msgstr "Sur {total}, {done} exportées"
```

Keep every placeholder of the English, spelled the same. A `{}` that
must change place in your language can be written `{0}`, `{1}` (the
first value, the second): `"{} of {}"` may be `"{1} sur {0}"`. A
doubled brace, `{{` or `}}`, is a brace.

## Plurals

A string with a count has an English singular and plural and one form
for each of your language's plural forms, chosen by the header's rule:

```po
msgid "{n} frame"
msgid_plural "{n} frames"
msgstr[0] "{n} vue"
msgstr[1] "{n} vues"
```

`{n}` is the count. Fill every form: a plural entry with one form
left empty shows in English on one side of the window and empty on
the other.

## What stays English

The log, the command line's messages, the user guide, and the release
notes stay English. So do names that are data rather than words: the
program's name, a file's or a camera's name, the keys' names, and
units' symbols. Dates and numbers keep their English form for now.
