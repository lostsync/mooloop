# 11 — Plugin GUIs in their own windows (#30)

Linear: MOO-86.

Adam's answer 2: a plugin's GUI opens in a separate window first. Plugins
without a GUI keep the face from step 08, which is always available anyway.

## Adam's ruling, 2026-09-28: native Wayland by default, XWayland as a setting

Asked how the GUI window should work on a Wayland session, given the facts
below, Adam chose a default and a setting:

- **Default:** mooloop stays a native Wayland client. Each plugin GUI gets a
  bare X11 window of mooloop's own, which runs through XWayland like the
  plugin itself. Those windows hide while mooloop is not focused and float
  rather than tile.
- **Setting:** run the whole application under XWayland, which is what
  Bitwig does (checked on Adam's Hyprland session: every Bitwig window
  reports `xwayland: 1`). Then the plugin window can be transient for the
  main window and everything behaves as on X11.

> it feels weird to have my own app run in XWayland. that said, yeah now that
> ive seen that xwayland is used by professionals in the same situation i feel
> more ok with that. and word if we could make it an option thats cool.

## Platform facts that shape this step

- **Embedding is X11, Cocoa and Win32 only.** CLAP's `gui.h` says the Wayland
  API is floating-only: a host does not call `set_parent` with it. Wayland has
  no protocol for one client to place its surface inside another's.
- **Almost every Linux plugin GUI is an X11 program** and offers embedding
  only; few implement floating mode at all. Under a Wayland session they run
  through XWayland whatever the host does. So in practice a host has to hand
  the plugin an X11 window id.
- **winit chooses X11 or Wayland once per process.** The backend is fixed when
  the event loop is built, and winit allows one event loop per process, so
  Slint cannot make one window X11 while the rest are Wayland. The old
  question "can winit be forced onto X11 for this one window" is answered: no.
  Qt has the same shape (`QT_QPA_PLATFORM` is per process), so this is not a
  toolkit problem.
- **`x11rb` is already in `Cargo.lock`**, pulled in by winit, so a bare X11
  window costs no new dependency tree.
- **Linux plugin GUIs need two more host extensions.** Most JUCE and DPF
  plugins run their X11 event loop on callbacks from the host's
  `timer-support` and `posix-fd-support`. `mooloop-plugin-host` enables
  neither (its `clack-extensions` features stop at `thread-check`). Without
  them, a GUI can open and then never repaint or take input.

## Policy

1. **Host extensions first.** Implement `timer-support` and
   `posix-fd-support`, both driven from the 8 ms pump on the control thread:
   fire any timer whose period has elapsed, and `poll` the registered fds with
   a zero timeout and call `on_fd` for the ready ones. Neither waits, so the
   pump stays wait-free (see "Three things the tree already gives" in
   `00-status.md`).
2. **Native Wayland session (the default):**
   - Ask `is_api_supported(X11, is_floating = false)`. If yes, open a
     connection with `x11rb`, create a bare top-level window, title it with
     the plugin and track name, and call `set_parent` with its id. Its events
     (configure, `WM_DELETE_WINDOW`, focus) are drained in the pump with
     `poll_for_event`.
   - Mark the window `_NET_WM_WINDOW_TYPE_DIALOG` (or `UTILITY`), and give it
     fixed min and max size hints when the plugin cannot resize, so tiling
     compositors float it instead of tiling it.
   - It cannot be made transient for a Wayland window. **Hide it while
     mooloop is not focused** instead: unmap every plugin window when focus
     leaves both the main window and all plugin windows, and map them again
     when focus returns. Mapping is plain X11 and works under every
     compositor, where `_NET_WM_STATE_ABOVE` is honoured by some compositors
     and not others. Slint reports the main window's focus; X11 reports the
     plugin windows'.
3. **X11 session, or the XWayland setting:** the same bare window, and also
   `set_transient` to the main window's X11 id, from Slint's
   `raw-window-handle-06` feature (root `Cargo.toml`). Nothing hides on focus
   loss, because the window manager keeps it above its parent.
4. **The plugin only offers floating mode:** `create(api, is_floating =
   true)`, `suggest_title`, and `set_transient` where the session allows it.
5. **Failure:** show the badge from step 08 with the reason. The face stays.
6. **macOS:** the same shape with a Cocoa parent, if Adam has the Mac at hand.

### The XWayland setting

- A Preferences toggle, "Run under XWayland (full plugin window behaviour)",
  off by default. It needs a restart, because the backend is chosen before
  the first window. Say so next to the toggle.
- It has to be read before Slint's backend starts, which puts it in the
  settings-load policy (Platform & Release). How to force it is to be found
  out while building this step. The candidates are clearing `WAYLAND_DISPLAY`
  before the backend starts, or building Slint's winit backend with winit's
  `with_x11()`. Whichever works has to leave audio and MIDI untouched.

## Lifetime

- Every GUI call happens on the control thread, and `HostedGui` enforces it.
- Closing the window (with the close button or `request_closed`) calls
  `hide` and `destroy`. **The processor is never stopped or reclaimed** when
  a GUI closes.
- Removing the device, closing the project, or exiting the app destroys the
  GUI before the instance is dropped (step 04's order: GUI, then processor,
  then instance). The bare X11 window is destroyed after the plugin's
  `destroy`, never before.
- Resizing follows `request_resize` / `can_resize` / `adjust_size`, with
  window-scale from the Slint window's scale factor.
- Parameter changes and gestures from the GUI arrive on step 07's ring and
  follow step 07's path. There is nothing to add here.

## Tests

- The test plugin with a GUI: open, close, reopen, resize, and remove while
  the GUI is open. A drop counter shows no leaked GUI.
- Audio keeps running while the GUI opens and closes: no xrun in the
  engine's counter over 100 open/close cycles.
- A timer and an fd registered by the test plugin fire from the pump, and
  are gone after the plugin is destroyed.

Record which path Surge XT, LSP and the test plugin take, and whether the
window floats and hides on focus loss, under **Hyprland** (Adam's own
session), GNOME Wayland, KDE Wayland and X11, and with the XWayland setting
on.

## Done when

- [ ] #30's checklist on Linux (X11 and Wayland, Hyprland among the
      Wayland sessions). macOS if Adam has the Mac at hand, and not required
      otherwise.
- [ ] The XWayland setting exists, survives a restart, and changes nothing
      but the display backend.
