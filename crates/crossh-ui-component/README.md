# crossh-ui-component

Crossh's own reusable GPUI component layer.

The crate follows the useful design ideas from `gpui-component` without
depending on that crate at runtime:

- components are stateless `RenderOnce` values;
- builders describe visual variants, sizes, and event callbacks;
- feature views own state and pass callbacks into components;
- colors and dimensions come from Crossh's theme.

The public prelude includes the basic controls plus shared `TabStrip`/`TabItem`,
`StatusBar`, and text-only `StatusMetric` shells. Workspace and standalone
feature views provide their own state, content, and callbacks to these
components.
