# Outcome: done

Closed August 2026. Thirteen of the fourteen plans that came out of
`docs/EFFECTS_FEEDBACK.md` landed; the status audit for each is preserved in
the files here. Two notes that outlive the individual plans:

- The display dim/darken option deferred from `12-reverb-scope-rework.md` is
  an appearance-prefs item, not a reverb item. The prefs mechanism it should
  reuse landed with `03-antialias-splines-with-a-prefs-toggle.md` as the
  `DisplayPrefs` global in `ui/theme.slint`; a dim toggle is a small
  follow-up there.
- `05-draggable-graph-points.md` extracted the shared `DraggablePoint`
  component in `ui/controls.slint`; any future device scope that needs a
  draggable marker on a curve should use it rather than a bespoke
  `TouchArea`.

`13-mod-device-shrink.md` is the one plan still open and lives in the active
`docs/plans/effects-feedback/` directory.
