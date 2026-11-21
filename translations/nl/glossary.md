# Dutch (nl) glossary for greycard's window

This is the glossary of a draft translation that no native speaker has
read yet. Corrections to it and to the catalog are welcome.

Source of the terms: the Dutch Lightroom Classic (LrC) first, Capture
One (C1) and darktable (dt) where LrC has none or they differ. Where
Dutch photographers keep the English word, the table says so and why.

## Conventions

- **Address.** Buttons, menu items, and commands: the infinitive, object
  first ("Map openen...", "Exporteren", "Masker kopiëren"), as
  Microsoft's, Apple's, and Adobe's Dutch editions write them. A
  sentence that speaks to the user: informal "je" (Microsoft and Apple
  use it; "u" reads as dated in software). A hint on the status line
  is the plain imperative ("klik", "sleep", "kies"). Status lines:
  neutral, past participle or infinitive ("{n} foto's geïmporteerd",
  "de catalogus lezen..."). One exception for width: "Bewaar..." (see
  the table).
- **Capitalization.** Sentence case, as the English. ALL CAPS section
  headings stay ALL CAPS. Status-line fragments that start lowercase in
  English start lowercase.
- **Product and data words.** greycard, Lightroom, RawTherapee,
  darktable, lensfun, GitHub, JPEG, TIFF, PNG, DNG, ICC, EXIF, XMP,
  ONNX Runtime, file names, units (MB, KB, GiB, px, EV, K, s, mired),
  and the search's tokens (`camera:`, `iso>=`) stay as they are.
  Compounds with them take a hyphen, as Dutch spelling wants:
  "Lightroom-catalogus", "XMP-sidecars", "ICC-profielen",
  "camera-JPEG". "raw" stays lowercase as a noun, as Dutch
  photographers write it; "Raw" as the filter chip.
- **Keys.** Key names stay as the keys sheet draws them (Ctrl, Shift,
  Alt, Space, Enter, Esc, Tab), as docs/translating.md asks; most Dutch
  keyboards print them so anyway. The pointer's word is translated
  ("Ctrl+klik", "rechtsklik").
- **Punctuation.** As the source: "..." stays three dots, "…" one
  character. No space before a colon. Leading and trailing spaces,
  commas, and semicolons of a tail kept; `\n` kept. Never an
  abbreviation with a period.
- **Numbers and dates** keep the English form (§616): "0.5 MB".
- **Plurals.** nplurals=2; plural=(n != 1). "foto" / "foto's".
- **Frame / picture.** A frame is "foto" (LrC NL, short, what Dutch
  photographers say); a picture (the rendered image, where the source
  says "picture") is "beeld", keeping the English distinction.
- **Pronouns.** A file, a look, a folder, a version: "hij/hem" for
  common-gender nouns; neuter nouns get "het". Where the referent is a
  {name} of unknown kind, "hij".

## Terms

| English | Dutch | Why / alternative |
| --- | --- | --- |
| Library | Bibliotheek | LrC |
| Develop | Ontwikkelen | LrC module name |
| Loupe | Loep | LrC "Loepweergave" |
| Lightbox | Lichtbak | the Dutch word for a light table; alt. "Lichttafel" |
| Filmstrip | Filmstrook | LrC |
| Grid | Raster | LrC "Rasterweergave" |
| Panel | Paneel | LrC; alt. "Deelvenster" (Microsoft) |
| Sheet (a modal pane) | Venster | Dutch has no word for a sheet; alt. "Blad" (Apple) |
| Cull / Culling | Schiften | the word Dutch photographers use for it; alt. "Selecteren" (collides with select), "Uitzoeken" |
| Exposure | Belichting | LrC |
| Contrast | Contrast | LrC |
| Highlights | Hooglichten | LrC; C1 "Hoge lichten" |
| Shadows | Schaduwen | LrC |
| Midtones | Middentonen | LrC Kleurgradatie |
| Whites / Blacks | Witte tinten / Zwarte tinten | clear and short; LrC NL may say "Witte kleuren / Zwarte kleuren" |
| Highlight fill | Hooglichtvulling | literal; alt. "Hooglichtreconstructie" (dt) |
| Display curve | Weergavecurve | none in LrC; alt. "Tonemapping" (de's choice) |
| Clarity | Helderheid | LrC |
| Texture | Textuur | LrC |
| Structure (Noise's band) | Structuur | literal; Texture is "Textuur", so no collision |
| Dehaze | Nevel verwijderen | LrC |
| Vibrance | Levendig | LrC "Levendigheid" clipped the Color section's slider label; alt. "Vibrance", which many Dutch users keep |
| Saturation | Verzadiging | LrC |
| White balance | Witbalans | LrC |
| Temperature / Tint (WB) | Temperatuur / Tint | LrC |
| As shot | Zoals opgenomen | LrC |
| Daylight, Cloudy, Shade | Daglicht, Bewolkt, Schaduw | LrC |
| Tungsten, Fluorescent, Flash | Gloeilamp, Tl-licht, Flitser | LrC; alt. "Tungsten" (kept in some Dutch editions), "Fluorescerend" |
| TINT (the section that lays a color over) | TINT | the WB tint's word, as the English shares it |
| Curves / Tone curve | Curven / Tooncurve | LrC "Tooncurve" |
| Lights / Darks (parametric) | Lichte tinten / Donkere tinten | LrC |
| Point / Parametric / Channel | Punt / Parametrisch / Kanaal | LrC |
| Color mixer | Kleurmixer | LrC |
| Hue / Luminance | Kleurtoon / Luminantie | LrC |
| Color grading | Kleurgradatie | LrC; many Dutch users say "color grading"; alt. "Kleurcorrectie" |
| Balance (grading) | Balans | LrC |
| Black & white | Zwart-wit | LrC |
| Weight (B&W filter) | Gewicht | literal |
| Sharpen (panel) | Verscherpen | LrC |
| Sharpen (export row) | Scherpte | "Verscherpen" clipped in the export sheet's label column |
| Output sharpen | Uitvoerverscherping | alt. "Exportverscherping" |
| Amount / Radius / Detail / Masking | Hoeveelheid / Straal / Detail / Maskeren | LrC |
| Threshold / Iterations | Drempel / Iteraties | |
| Noise / Denoise | Ruis / Ruis verwijderen | LrC "Ruis verwijderen" for AI Denoise |
| Luma / Chroma | Luma / Chroma | kept English as dt and C1 do; alt. "Luminantie / Kleur" (LrC NR) |
| Grain | Korrel | LrC |
| Vignette (effect) | Vignet | the lens's is "Vignettering", kept apart |
| Feather | Doezelaar | LrC; alt. "Zachte rand" |
| Roundness / Midpoint | Rondheid / Middelpunt | LrC |
| Demosaic | Demozaïek | alt. "Demosaicing", which many keep in English |
| Lens corrections | Lenscorrecties | LrC |
| Chromatic aberration | Chromatische aberratie | LrC |
| Defringe | Randen verwijderen | LrC "Kleurranden verwijderen", shortened |
| Distortion / Vignetting | Vervorming / Vignettering | LrC |
| Crop | Uitsnijden | LrC; alt. "Bijsnijden" (Microsoft, Apple) |
| Aspect | Verhouding | LrC "Hoogte-breedteverhouding" is too long |
| Guides | Hulplijnen | |
| Golden ratio / spiral, Thirds | Gulden snede / spiraal, Derden | LrC overlays |
| Rotate / Turn | Draaien; Linksom / Rechtsom | LrC "Linksom draaien" |
| Flip | Spiegelen | LrC |
| Level | Rechtzetten | LrC; alt. "Rechttrekken" |
| Perspective | Perspectief | |
| Mask | Masker | LrC |
| Linear / Radial | Lineair / Radiaal | LrC "Lineair verloop / Radiaal verloop", shortened as the English is |
| Brush / Paint | Penseel / Schilderen | LrC |
| Erase | Gummen | LrC "Gum"; alt. "Wissen" (collides with Clear) |
| Invert / Intersect / Subtract / Add | Omkeren / Snijden / Aftrekken / Toevoegen | LrC |
| Subject / Sky / Background / People / Object / Landscape | Onderwerp / Lucht / Achtergrond / Personen / Object / Landschap | LrC |
| Refine edge | Rand verfijnen | Photoshop NL |
| Color range / Luminance range | Kleurbereik / Luminantiebereik | LrC |
| Fade (range masks) | Overgang | alt. "Afvlakking" |
| Sample (range) | Monster | Photoshop NL "monster nemen"; alt. "Pipet" |
| Retouch | Retouche | the noun on the tab and the heading: "Retoucheren" clipped the tab |
| Heal / Clone / Fill | Herstellen / Klonen / Vullen | LrC |
| Patch (fill mode) | Lap | a short word for a narrow button; alt. "Opvullen" |
| Look | Look | photographers' word, kept by de and it; LrC "Profiel" means something else here |
| Camera profile | Cameraprofiel | LrC |
| Fit this camera's look... | Cameralook aanpassen... | "De look van deze camera aanpassen..." clipped its button |
| Preset | Voorinstelling | LrC |
| Preset (export row) | Preset | "Voorinstelling" clipped the export sheet's label column; C1 NL and many users keep "preset" |
| Save... (presets pane, filter bar) | Bewaar... | "Opslaan..." clipped in the presets pane; an imperative, against the infinitive rule, for width |
| Import... (File menu, presets pane) | Import... | "Importeren..." clipped in the presets pane; the noun reads well in a menu |
| Reset (the shared button word) | Reset | LrC "Opnieuw instellen" is too long; "Herstellen" is Heal |
| Snapshot | Momentopname | LrC |
| History | Historie | LrC |
| Virtual copy / Version | Virtuele kopie / Versie | LrC |
| Sidecar | sidecar | kept English: Dutch photographers say "sidecar(-bestand)"; alt. "XMP-bestand" is too narrow (greycard's are .gcd) |
| Edits go (sidecar place row) | Opslaan in | |
| Rating / stars | Waardering / sterren | LrC |
| Flag / Pick / Reject / Unflag | Vlag / Markeren / Afwijzen / Vlag verwijderen | LrC "Markering"; "gemarkeerd" for picked |
| Unflagged | Zonder vlag | |
| Label | Label | LrC "Kleurlabel" in menus; "Label" fits rows |
| Keyword | Trefwoord | LrC |
| Stack / Unstack / Burst | Stapel / Stapel opheffen / Reeks | LrC |
| Collection / Smart collection / Collection set | Verzameling / Slimme verzameling / Verzamelingsset | LrC |
| Root (a library root) | Bronmap | alt. "Hoofdmap" |
| + Add this folder / + Add a root... | + Deze map / + Bronmap... | the verb dropped: the pair sits side by side and clipped |
| Archive / Back up / Bring back | Archief / Back-up maken / Terughalen | "Terugzetten" is Move back (rejects) |
| Export / Import | Exporteren / Importeren | LrC |
| Export queue | Exportwachtrij | |
| Soft proof | Softproef | LrC |
| Gamut warning | Gamutwaarschuwing | LrC |
| Intent (rendering) | Methode | LrC "Rendermethode"; the short form fits the row |
| Long edge | Lange zijde | LrC |
| If it exists | Bestaat al | "Als hij bestaat" clipped the export sheet's label column |
| Increment / Overwrite / Skip | Ophogen / Overschrijven / Overslaan | |
| Metadata (export row) | Metadata | "Metagegevens" (LrC) clipped there; heading METAGEGEVENS elsewhere |
| Watermark | Watermerk | LrC |
| Capture time | Opnametijd | LrC |
| Navigator | Navigator | LrC |
| Scope | Grafiek | alt. "Scope" |
| Clipping | Clipping | LrC NL keeps "clipping" |
| Merge to HDR / Panorama / Focus stack | Samenvoegen tot HDR / panorama / focusstapel | LrC "Samenvoegen tot foto" |
| Focus (sort) | Scherpte | |
| Card | Kaart | memory card |
| Trash | Prullenbak | Windows, GNOME, and Apple NL |
| Settings | Instellingen | |
| Keys | Sneltoetsen | |
| Scroll bar size | Balkbreedte | "Schuifbalkgrootte" clipped |
| Canvas | Canvas | LrC "Achtergrond" clipped Settings > Bewerken's label column; "canvas" is the Dutch word too |
| Thumbnail / Preview | Miniatuur / Voorvertoning | LrC |
| Log (the log file) | Log | Dutch developers' word; "Logboek" is longer |
| Done | Gereed | Microsoft and Apple NL; alt. "Klaar" |
