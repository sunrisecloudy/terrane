# Forge UI Catalog

Source of record: prd-merged/05 UI-2, UI-3, UI-6, and UI-7. TS-facing names are camelCase to match forge/std/forge-std.d.ts and T015.

| Component | Category | Key props | Variants | Sizes | A11y role | UI-6 fallback | Milestone |
|---|---|---|---|---|---|---|---|
| Stack | Layout | direction, gap, align, children | h/v; gap tokens | n/a | group | unknown props ignored; unknown children rendered as fallback | M0a |
| Grid | Layout | columns, gap, children | auto or count | n/a | group | fallback box for unknown child | later |
| Scroll | Layout | axis, child | x/y/both | n/a | region when labelled | child fallback preserved | later |
| Spacer | Layout | size | semantic size tokens | n/a | presentation | ignored if unknown | later |
| Divider | Layout | orientation | horizontal/vertical | n/a | separator | presentation fallback | later |
| Card | Layout | variant, intent, children | plain/outlined/elevated | n/a | group/region if labelled | children fallback preserved | later |
| Text | Content | text, variant, intent | body/caption/title/subtitle/monospace | n/a | text | render plain text fallback | M0a |
| Icon | Content | name, size, intent, ariaLabel | catalog token | s/m/l | img if labelled; presentation otherwise | fallback announces unknown icon name | later |
| Image | Content | src, alt, fit, aspectRatio | contain/cover/fill | n/a | img | requires alt; invalid src becomes labelled fallback | later |
| Badge | Content | label, intent, size | intent color | s/m/l | status/text | label text fallback | later |
| Markdown | Content | text, allowedElements | safe subset only | n/a | document/text | render sanitized text fallback | later |
| Button | Input | label, icon, variant, size, onTap | primary/secondary/destructive/ghost | s/m/l | button | unknown action disabled with label | M0a |
| TextField | Input | value, label, placeholder, required, validation, onChange | validation state | n/a | textbox | requires accessible name | M0a |
| TextArea | Input | value, label, minRows, validation, onChange | validation state | n/a | textbox multiline | requires accessible name | later |
| Select | Input | value, label, options, validation, onChange | single choice | n/a | combobox/listbox | requires accessible name | later |
| MultiSelect | Input | values, label, options, validation, onChange | multi choice | n/a | listbox multiselect | requires accessible name | later |
| Checkbox | Input | checked, label, onChange | checked/unchecked | n/a | checkbox | requires label or ariaLabel | later |
| Switch | Input | checked, label, onChange | on/off | n/a | switch | requires label or ariaLabel | later |
| Slider | Input | value, min, max, step, label, onChange | range | n/a | slider | requires label and numeric range | later |
| DatePicker | Input | value, min, max, label, onChange | date | n/a | date input/combobox | requires label; invalid date fallback | later |
| List | Data | items, virtualized, emptyState | virtualized true/false | n/a | list | unknown item fallback preserved | M0a |
| Table | Data | columns, rows, sort, selection | sort/select | n/a | table/grid | unknown cell values stringify | later |
| Chart | Data | chart, labels, series, summary | line/bar/pie/scatter | n/a | img/group with summary | requires summary fallback | later |
| Stat | Data | label, value, delta, intent | intent/delta | n/a | status/text | label/value text fallback | later |
| Tabs | Structure | active, tabs, onChange | tab set | n/a | tablist/tab/tabpanel | unknown panel fallback preserved | later |
| Modal | Structure | title, open, child, onClose | dialog | n/a | dialog | title announced; child fallback preserved | later |
| Form | Structure | children, submitLabel, validation, onSubmit | validation state | n/a | form | type-level label rule for controls | later |

## Proposed Deviations And Open Shapes

- Chart keeps the initial shape to chart kind, labels, series, and required summary. Axis scales, legends, stacked bars, and time-series interpolation are underspecified by UI-2 and should be added deliberately.
- Table uses column ids plus row cell maps. Cell renderers, column widths, frozen columns, and server-side sort are not in the PRD yet.
- Image requires an alt string and a runtime-validated src. The exact source grammar needs the future files/capability policy.
- Icon uses a name token but the concrete icon catalog is not specified yet. Icon-only buttons must still provide label or ariaLabel per T014.
- Markdown is a safe subset. Links and code blocks need sanitizer decisions before GA.
- DatePicker is date-only for this catalog. Time zones, date ranges, and date-time values are deferred.

## Notes

- M0a components are the ones already represented by forge/std/forge-std.d.ts and the T005 golden trees: Stack, Text, Button, TextField, and List.
- Unknown components must not crash a renderer. Renderers should show a labelled fallback box that includes the component type, ignore unknown props, and keep rendering known descendants when present.
