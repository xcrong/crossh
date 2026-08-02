# Crossh Agent Instructions

## Icon Assets

- Every SVG under `assets/icons/` must be an unmodified official SVG downloaded from the pinned Lucide release `1.27.0`.
- The source release is https://github.com/lucide-icons/lucide/tree/1.27.0/icons and the raw source pattern is `https://raw.githubusercontent.com/lucide-icons/lucide/1.27.0/icons/<name>.svg`.
- Do not hand-write, redraw, simplify, reformat, or manually edit icon path data. Additions and replacements must use the canonical Lucide filename and the exact downloaded file.
- When a Lucide icon is renamed, update the local `IconName` mapping and asset loader together. The current `CircleX` mapping intentionally follows Lucide's canonical `circle-x.svg` filename.
- Keep the pinned release and third-party attribution in sync when updating the icon set. See `THIRD_PARTY_NOTICES.md`.
