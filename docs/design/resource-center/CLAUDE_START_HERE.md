# Satellite NG / Design Resource Center — Claude, start here

**Role:** read-only UI reference. The HTML laboratory and `components.registry.json` are **not** new sources of domain truth, do not replace `docs/semantic/MAP.md`, and are **not** runtime plugin resource manifests under ADR 0011.

## Read in this order

1. `docs/semantic/MAP.md` from the **CURRENT** Satellite NG repo. Locate concept, `@sat:` owner, and `Not here` boundary.
2. The corresponding ADR and the current owner code. In this archive snapshot: ADR 0010 core, ADR 0012 card/flow (accepted), ADR 0016 workspace list (proposal), ADR 0009 windows/workspaces (proposal), ADR 0011 resource registry (accepted). Also ADR 0020 `adr-0020-interface-direction.md` from `satellite-ui-handoff.zip` is **proposed**, and describes the four UI control roles; refer to the latest repo copy before implementing.
3. `components.registry.json`: visual specimen, source, readiness, owner or boundary, related lab anchor.
   ADR 0020 is bundled as `adr-0020-interface-direction.md`, **a proposed snapshot**, not a substitute for current repo history.
4. `Satellite_Design_Resource_Center.html` for interactive behavior and the complete visual gallery.
5. `satellite-tokens.css`, `satellite-controls.css`, `satellite-controls.js` for **isolated** reference implementation only.

## Status codes

- `REFERENCE_CODE` (11): standalone `satui-*` reference markup and scoped CSS/JS. It was tested in the resource center, **not certified in Satellite NG**. Adapt into the actual UI owner; do not blindly paste the whole pack.
- `GALLERY_REFERENCE`: interactive/static design example in v7. No drop-in implementation contract.
- `DESIGN_CONCEPT`: may depend on unimplemented or proposed NG architecture. **Do not claim implemented**.

## Integration algorithm

1. Identify desired component and precise user interaction. Classify if it changes **canon, decoration, facts, decisions, settings, or view only**.
2. Locate actual owner and anchor in MAP and source. If an owner exists, modify it; no second UI state owner, no shadow CSS layer, no `v2/v3` wrappers.
3. Keep domain behaviors in NG commands/queries. The reference JS dispatches `satui:input` and `satui:change` with `{value,source}`. The adapter to `App::Command` belongs to the actual UI owner; this reference does not call any Satellite API.
4. Import reference tokens/controls once, or extract minimal declarations into the existing owner; when replacing an implementation remove the superseded code and update tests.
5. Verify working app at 390/900/1450px, both themes, pointer, keyboard, wheel, Shift+wheel, soft bounds, alpha, overlays, scroll containers, app data and DOM re-renders; assert no duplicate handlers and no regressions to canonical order.
6. If semantic ownership changes, amend MAP and anchored code in the **same commit**, and run mapped CI checks.

## Non-negotiable invariants

- Canon of mod load order: **top-to-bottom, and within each row left-to-right**. Row layout is computed by `@sat:order.flow` and exposed by `@sat:app.lines`; CSS never reorders domain entries.
- Focus cursor is an intention/viewport mechanism; selection changes only by its explicit interaction contract.
- Styling a mod does not change canon. `@sat:order.decor.style` and `@sat:order.card` remain distinct from order itself.
- A display change cannot change overwrite conflict facts or decision state.
- ION `#67FFBC` is used **rarely for verified positive evidence**. No blue in the 4 Satellite marks; base accent still the existing blue palette.
- Quiet hover-only surfaces for normal actions; segmented tooth for one-of-N view mode. Functional centrals use separate command semantics. ADR 0020 classifies four roles: mode, action, data, toggle. Data chips are dynamic and NOT action buttons; metadata block toggles are indicator controls.
- SMART 34×17 incl. popup arrow, alpha + full HSV panel. FAST 34×17, coarser choice, **no alpha and no popup**.
- No hard clamping in elastic numeric *UX*. Domain validation remains the authority and may reject values where appropriate. Soft min/max are hints supplied by context, not global constants.

## Package ownership

This package owns only `.satui-*` sample CSS and its `SatUIDesignKit` example runtime. It owns no Satellite NG source. The initial HTML preserves v7 lab sections below the resource center, which are **specimens**, not production components.

`source: Satellite_UI_Laboratory_v7.html; Satellite-main (1).zip NG snapshot; adr-0020-interface-direction.md from satellite-ui-handoff.zip; historical monolith V388 studied.`
