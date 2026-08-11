# crossh-ui-component

Crossh's own reusable GPUI component layer.

The crate follows the useful design ideas from `gpui-component` without
depending on that crate at runtime:

- components are stateless `RenderOnce` values;
- builders describe visual variants, sizes, and event callbacks;
- feature views own state and pass callbacks into components;
- colors and dimensions come from Crossh's theme.

The public prelude currently includes `Button`, `Badge`, `Separator`, and the
`h_flex`/`v_flex` layout helpers.
