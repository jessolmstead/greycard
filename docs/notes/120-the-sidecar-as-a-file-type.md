# 120. The sidecar as a file type (2026-09-20)

§86 decided the `.gcd` stays beside its raw, not hidden and not in a
subfolder, and took the cost that a file manager would show it as a
generic document between the frames. The desktop side of that cost is
now paid on Linux: `packaging/linux/greycard.xml` declares
`application/x-greycard-edit` for `*.gcd` as a subclass of JSON, by
glob only, since a sidecar starts with the same brace as every other
JSON file and a magic rule would have nothing to hold; an icon named
for the type, two of the app tile's cards on a document (three collapse
to a smudge at 16 px), goes under the theme's `mimetypes`; the `.desktop` entry lists the type first among the ones
the editor opens; the tarball carries the XML and the icon, and the
installer runs `update-mime-database` beside the icon cache it
already rebuilt. Checked against a fresh data directory: the type is
found, and the icon reads at 16 and 22 px once its inner mark was
made large enough to survive them.

Opening a sidecar from the file manager launches the editor with the
`.gcd` path, so a single-file argument maps `IMG.CR3.gcd` to
`IMG.CR3` and refuses with both names when the raw is not there. The
directory listing does not map: sidecars were never listed there,
and mapping them would have shown every edited frame twice, which
the review of the first version caught. The Mac bundle's `Info.plist`
and Windows registry entries for the same type are not done and can
join their packaging lines when those are exercised.
