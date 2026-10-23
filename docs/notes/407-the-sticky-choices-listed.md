# §407. The sticky choices listed in Settings

*2026-10-23*

A tester could not find where greycard keeps its lasting choices:
the export's format on the export sheet, the canvas and the monitor
profile in the panel's MONITOR section, the proof beside them. Each
is where it is used, which is right, and nothing moves; the Settings
sheet's Editing category lists them as well, so one place answers
"where is that set".

Each row binds two-way to the same window property its original
control binds, so a change in either place shows in both and there
is no copy to drift; the settings file reads the window at the end
as it always did. Canvas, Monitor, and Soft proof with its intent
sit at the top of Editing; an EXPORT heading at the foot holds the
format and the color space. The reviewer checked the paths: the
Choose... buttons reach the desktop's profile chooser through the
panel's own callbacks, and the export's values save through the
window's existing change handlers once per change however many
bindings read them.

## Not done

- The Segmented controls in the sheet carry an accessible role and
  value so the tests can read them; the panel's own Segmented
  carries none. The role belongs on the control itself, for both.
