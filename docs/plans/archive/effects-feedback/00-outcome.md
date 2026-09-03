# Outcome: done

Closed September 2026. All fourteen plans that came out of
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

`13-mod-device-shrink.md` was the last to land, on 2026-09-02, about a week
after the other thirteen. All four of its items are in: the redundant
mode/rate/depth text panel beside the display is gone, `ModulationDisplay`
went from a 330x108 banner to a 174x148 left column, the seven full-size
`ParameterKnob`s became eight compact ones at 60px/36px, and the device
dropped from 3U to 2U -- pinned by `effect_kind_units(EffectKind::Modulation)
== 2` rather than left to drift. The face was recomposed as an instrument
panel in the same pass (`87d18f8`, `5013ad9`).
