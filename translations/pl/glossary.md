# Polish (pl) glossary for greycard's window

This is the glossary of a draft translation that no native speaker has
read yet. Corrections to it and to the catalog are welcome.

Source of the terms: the Polish Lightroom Classic (LrC) first,
Photoshop's Polish edition (PS) and Capture One (C1) where LrC has
none, and the words Polish photographers use where the software has
none.

## Conventions

- **Address.** Buttons, menu items, and commands: the imperative, as
  Polish programs write them (Zapisz, Anuluj, Eksportuj..., Otwórz
  folder), LrC and Windows included; an infinitive on a button reads
  as old-fashioned. A question in a sheet's title: the infinitive
  ("Usunąć 3 zdjęcia?", "Pobrać profile obiektywów?"), the Polish
  software idiom. Status lines: impersonal past in -no/-to
  ("zaimportowano 3 zdjęcia", "nic nie usunięto") or a verbal noun
  ("eksportowanie 3 zdjęć..."); never "ja", never "Pan/Pani". A hint
  that tells the user to act uses the imperative ("kliknij",
  "przeciągnij", "Esc kończy").
- **Plurals.** nplurals=3: [0] n == 1 (1 zdjęcie), [1] 2-4 except
  12-14 (2, 22 zdjęcia), [2] the rest (0, 5, 12, 25 zdjęć). Each form
  in the case its sentence puts the noun in: accusative after a verb
  ("Eksportuj 5 zdjęć"), genitive after "z", "do", or a negation ("z 5
  zdjęć", "nie usunięto 2 zdjęć"), locative after "na" ("na 2
  zdjęciach"), instrumental after "z" meaning "with" ("z 2
  zdjęciami"). For 5 and up a subject takes a singular verb ("5 zdjęć
  jest", "2 zdjęcia są"), which the forms carry. The impersonal -no
  past ("przeniesiono") avoids gender agreement, so the status lines
  use it.
- **Placeholders that would need declension.** A name, a path, or a
  composed count never gets a case ending: the sentence is turned so
  it stands in the nominative, after a colon ("Usuń: {}", "Przenieś z
  powrotem do: {}", "pozostało w kolejce: {left}"), in apposition ("w
  folderze {}", "z kolekcji {}"), or in parentheses ("Przesuń czas
  wykonania (3 zdjęcia)..."). Where a count cannot be declined at all,
  a genitive-plural label is used ("folderów: {n}"), the usual Polish
  UI shape ("Plików: 3"). A filler that lands after a preposition is
  given in the case it lands in ("ich folderów", "jego stosu", "tym").
- **Capitalization.** Sentence case as the English has. ALL-CAPS
  headings stay ALL CAPS, diacritics included (BALANS BIELI, ŚWIATŁO,
  WYOSTRZANIE WYJŚCIOWE). Status fragments that start lowercase in
  English start lowercase.
- **Product words.** greycard, Lightroom, RawTherapee, darktable,
  lensfun, GitHub, ONNX Runtime, JPEG, TIFF, PNG, DNG, DCP, ICC, EXIF,
  XMP, file names, units (MB, KB, GiB, px, EV, K, mired, s), and the
  search's tokens (`camera:`, `iso>=`) stay as they are. "Lightroom"
  and "GitHub" take Polish case endings in running text (z Lightrooma,
  z GitHubem), as Polish writing does.
- **Keys.** Key names stay English (Ctrl, Shift, Alt, Space, Enter,
  Esc, Tab), as docs/translating.md asks: the keys sheet draws its
  chords from the English table. The pointer's word is translated
  ("Ctrl+klik").
- **Punctuation.** Three dots where the source has three dots. No
  space before a colon. No Oxford comma: Polish puts no comma before
  "i", "lub", or "oraz" in a list. Leading and trailing spaces, commas,
  and semicolons of a tail kept. ASCII quotes kept where the source
  has them (a Polish reviewer may want „...”).
- **No abbreviations with a period.** "wył.", "np.", "zob.", "min."
  avoided, and "wg" too; "Wyłączone", "według:", "szczegóły w
  dzienniku" written out.
- **Numbers and dates** keep the English form (0.5 MB, not 0,5 MB), as
  the rest of the window does for now.
- **Frame / picture.** Both "zdjęcie" (LrC's word); "obraz" where the
  English means the rendered image (the Picture row, the export's
  "making pictures").

## Terms

| English | Polish | Why / alternative |
| --- | --- | --- |
| Library | Biblioteka | LrC |
| Develop | Wywoływanie | LrC module name |
| Loupe | Lupa | LrC "Widok lupy" |
| Grid | Siatka | LrC "Widok siatki" |
| Lightbox | Podświetlarka | the photographer's light table; alt. "Stół podświetlany" |
| Filmstrip | Taśma filmowa | LrC |
| Cull / Culling | Selekcja | the photographers' word for a first pass; alt. "Przegląd" |
| Exposure / Contrast | Ekspozycja / Kontrast | LrC |
| Highlights / Shadows | Światła / Cienie | LrC |
| Whites / Blacks | Białe / Czarne | LrC |
| Midtones (grading) | Półcienie | PS; alt. "Tony średnie" |
| Highlight fill | Wypełnienie świateł | literal; alt. "Odtwarzanie świateł" (dt) |
| Display curve | Mapowanie tonów | what the row picks (AgX and kin); alt. "Krzywa wyświetlania" |
| Clarity / Texture | Klarowność / Tekstura | C1's Polish; LrC's "Przejrzystość" clipped the slider label |
| Dehaze | Odmglenie | fits the slider; LrC "Usuwanie zamglenia" and "Usuwanie mgły" clipped |
| Vibrance / Saturation | Jaskrawość / Nasycenie | LrC |
| White balance | Balans bieli | LrC |
| Temperature / Tint (WB) | Temperatura / Odcień | LrC |
| As shot | Jak wykonano | LrC |
| Daylight, Cloudy, Shade, Tungsten, Fluorescent, Flash | Światło dzienne, Pochmurno, Cień, Żarówka, Świetlówka, Lampa błyskowa | LrC; alt. "Flesz" |
| TINT (the section that lays a color over) | TONOWANIE | the darkroom's word for toning; kept apart from WB "Odcień" |
| Tone curve / Curves | Krzywa tonalna / Krzywe | LrC |
| Point / Parametric | Punktowa / Parametryczna | LrC |
| Lights / Darks | Jasne tony / Ciemne tony | LrC |
| Color mixer | Mikser kolorów | LrC |
| Hue / Luminance | Barwa / Luminancja | LrC HSL |
| Aqua, Magenta, Purple | Akwamaryna, Karmazynowy, Fioletowy | LrC HSL; Purple is shared with the label menu, so "Fioletowy" |
| Color grading | Gradacja kolorów | LrC |
| Balance (grading) | Balans | LrC |
| Black & white | Czarno-białe | LrC |
| Sharpen (section) / Sharpen (export row, mask slider) | Wyostrzanie / Ostrość | the export label column fits 10 letters; LrC's mask slider is "Ostrość" |
| Amount | Wartość | LrC |
| Radius / Detail / Masking | Promień / Szczegóły / Maskowanie | LrC |
| Noise / Denoise | Szum / Odszumianie | LrC "Redukcja szumów"; "Odszumianie" for the learned one |
| Luma / Chroma | Luminancja / Kolor | LrC's noise pair; "Chrominancja" clipped the slider |
| Grain | Ziarno | LrC |
| Vignette / Vignetting | Winieta / Winietowanie | LrC keeps the effect and the lens fault apart the same way |
| Lens corrections | Korekcja obiektywu | LrC |
| Distortion | Dystorsja | photographers' word; LrC "Zniekształcenie" |
| Chromatic aberration | Aberracja chromatyczna | LrC |
| Defringe | Usuwanie obwódek | LrC |
| Crop | Kadrowanie | LrC |
| Aspect | Proporcje | LrC |
| Guides (crop overlay) | Prowadnice | PS; alt. "Linie pomocnicze" (long) |
| Thirds / Golden ratio / Golden spiral | Trójpodział / Złoty podział / Złota spirala | standard |
| Rotate / Turn | Obróć | one verb for both; the "not part of the edit" note tells them apart |
| Flip | Odbij | PS |
| Level | Poziomuj | LrC "Prostowanie"; a verb on the button |
| Perspective | Perspektywa | LrC |
| Mask | Maska | LrC |
| Adjustments | Korekty | LrC's mask panel shows "Dopasowania" in places; alt. "Ustawienia" |
| Linear / Radial | Liniowy / Radialny | LrC "Gradient liniowy / radialny" |
| Brush / Paint | Pędzel / Maluj | LrC |
| Feather | Wtapianie | LrC |
| Erase / Subtract / Intersect | Wymaż / Odejmij / Część wspólna | LrC |
| Invert | Odwróć | LrC |
| Subject / Object | Motyw / Obiekt | "Obiekt" is LrC's Subject; greycard has both, so Subject is "Motyw" (motyw główny) |
| Sky / Background / People / Landscape | Niebo / Tło / Osoby / Krajobraz | LrC |
| Color range / Luminance range | Zakres kolorów / Zakres luminancji | LrC |
| fade (range edges) | Zanik (Zanik dołu, góry, barwy, koloru) | short for the sliders; alt. "Przejście", "Wygładzanie" |
| Retouch | Retusz | standard |
| Heal / Clone / Fill | Napraw / Klonuj / Wypełnij | verbs on buttons; LrC "Koryguj / Klonuj" |
| Patch / Fast / Best | Łatka / Szybka / Najlepsza | the fill's methods |
| Look | Look | photographers say "look" ("look filmowy"); "Styl" is the picture style's, "Profil" the camera profile's; alt. "Wygląd" |
| Camera profile | Profil aparatu | LrC |
| Picture style | Styl obrazu | Canon's Polish |
| Preset | Preset | what Polish photographers call them; LrC's "Ustawienie predefiniowane" is too long for a button |
| Snapshot | Migawka | LrC |
| History | Historia | LrC |
| Virtual copy / Version | Kopia wirtualna / Wersja | LrC |
| Sidecar | Plik pomocniczy | Adobe's "plik pomocniczy XMP"; alt. "Plik towarzyszący" |
| Rating / Stars | Ocena / Gwiazdki | LrC |
| Flag / Pick / Reject / Unflag | Flaga / Wybrane (Wybierz) / Odrzucone (Odrzuć) / Usuń flagę | LrC |
| Label | Etykieta | LrC "Etykieta koloru"; the label colors are feminine (czerwona) to agree |
| Keyword | Słowo kluczowe | LrC |
| Stack / Unstack | Stos / Rozdziel stos | LrC "Ułóż w stos" |
| Collection / Smart collection / Set | Kolekcja / Inteligentna kolekcja / Zestaw kolekcji | LrC |
| Root | Folder główny | alt. "Źródło" |
| Archive / Back up / Bring back | Archiwum / Archiwizuj / Przywróć | "Archiwizuj" keeps the archive's copy apart from the import's "Kopia zapasowa" |
| Backup (import row) | Kopia zapasowa | standard |
| Export / Import | Eksportuj / Importuj | LrC |
| Export queue | Kolejka eksportu | |
| Soft proof | Próba kolorów (the Settings row: Próba) | PS; LrC keeps "Softproofing" |
| Gamut warning | Ostrzeżenie o gamie | PS |
| Intent / Perceptual / Relative | Metoda / Percepcyjna / Względna | PS "Metoda konwersji" shortened |
| Canvas | Tło | LrC |
| Long edge | Długi bok | "Dłuższa krawędź" (LrC) clipped the export sheet's label column |
| If it exists / Increment / Overwrite / Skip | Istniejący / Numeruj / Nadpisz / Pomiń | "Jeśli istnieje" clipped |
| Watermark / Border | Znak wodny / Ramka | LrC |
| Midpoint (vignette) | Środek | LrC "Punkt środkowy" clipped |
| Refine edge | Krawędź | PS "Popraw krawędź" clipped the mask's slider |
| Red / cyan, Blue / yellow (lens) | R / C, B / Y | the full words clipped; letters as the curve's R/G, B/Y channels, which stay as written |
| Cell size / Scroll bars (Settings) | Miniatury / Paski | the longer forms clipped the label column |
| Find a lens | Szukaj | "Znajdź obiektyw" clipped the button |
| Metadata / Title | Metadane / Tytuł | LrC |
| Capture time | Czas wykonania | LrC "Data wykonania" |
| Navigator | Nawigator | LrC |
| Demosaic | Demozaikowanie | dt |
| Merge to HDR / Panorama / Focus stack | Scal do HDR / Scal do panoramy / Scal do stosu ostrości | LrC "Scalanie zdjęć"; focus stacking has no LrC word |
| Card | Karta | memory card |
| Trash | Kosz | Windows and GNOME |
| Settings / Keys | Ustawienia / Skróty | |
| Log (the file) | Dziennik | "Historia" is History |
| Caches | Pamięć podręczna | |
| Store (the models') | Magazyn | |
| Scope | Wykres | histogram or a scope |
| Theme: Charcoal / Grey / Light | Grafitowy / Szary / Jasny | |
| learned (denoise, fill) | wyuczone | the model's; alt. "AI" |
