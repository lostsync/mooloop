# 11 — Plugin GUIs in their own windows (#30)

Adam's answer 2: a plugin's GUI opens in a separate window first. Plugins
without a GUI keep the face from step 08, which is always available anyway.

## Platform facts that shape this step

- Enable Slint's `raw-window-handle-06` feature (root `Cargo.toml`). The
  default backend is winit 0.30 with femtovg.
- A CLAP plugin can be **embedded** into a parent window on X11, Cocoa and
  Win32. There is no Wayland embedding API, so under Wayland the options are
  the plugin's **floating** mode, or an X11 parent through XWayland.
- Most Linux plugin GUIs are X11 programs. Under a Wayland session they
  still run through XWayland.

## Policy

1. Ask the plugin for `is_api_supported` with embedding first: X11 on
   Linux, Cocoa on macOS.
2. **Embedded:** create a bare Slint window (`PluginWindow`, titled with the
   plugin and track name), get its handle, and call `set_parent`. On a
   Wayland session, that window has to be an X11 window for embedding to
   work. Find out whether winit can be forced onto X11 for **this one
   window**. If it can't, go to 3.
3. **Floating:** `create(api, is_floating = true)`, then `set_transient` to
   the main window where the platform supports it, and `suggest_title`.
4. **Failure:** show the badge from step 08 with the reason. The face stays.

Record which path Surge XT, LSP and the test plugin take under GNOME
Wayland, KDE Wayland and X11.

## Lifetime

- Every GUI call happens on the control thread, and `HostedGui` enforces it.
- Closing the window (with the close button or `request_closed`) calls
  `hide` and `destroy`. **The processor is never stopped or reclaimed** when
  a GUI closes.
- Removing the device, closing the project, or exiting the app destroys the
  GUI before the instance is dropped (step 04's order: GUI, then processor,
  then instance).
- Resizing follows `request_resize` / `can_resize` / `adjust_size`, with
  window-scale from the Slint window's scale factor.
- Parameter changes and gestures from the GUI arrive on step 07's ring and
  follow step 07's path. There is nothing to add here.

## Tests

- The test plugin with a GUI: open, close, reopen, resize, and remove while
  the GUI is open. A drop counter shows no leaked GUI.
- Audio keeps running while the GUI opens and closes: no xrun in the
  engine's counter over 100 open/close cycles.

## Done when

- [ ] #30's checklist on Linux (X11 and Wayland). macOS if Adam has the Mac
      at hand, and not required otherwise.
