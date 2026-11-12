# Fonts

Archivo by Omnibus-Type, from <https://github.com/Omnibus-Type/Archivo>
(`fonts/ttf`), under the SIL Open Font License 1.1 (`OFL.txt`). Archivo
declares no Reserved Font Name.

| File | Source | Changes |
| --- | --- | --- |
| `Archivo-Regular.ttf`, `-Medium`, `-SemiBold`, `-ExtraBold` | same names | none |
| `ArchivoDisplay-Regular.ttf` | `ArchivoExpanded-ExtraBold.ttf` | renamed |
| `ArchivoHeading-Regular.ttf` | `ArchivoSemiExpanded-ExtraBold.ttf` | renamed |

The two wide cuts are renamed to families of their own ("Archivo Display",
"Archivo Heading"), registered as Regular with normal width, and their DSIG
table is dropped. The glyphs are unchanged. The renaming is needed because
DirectWrite groups the wide cuts into the "Archivo" family by width, and
GPUI selects fonts by family and weight only, so the wide cuts were otherwise
unreachable.
