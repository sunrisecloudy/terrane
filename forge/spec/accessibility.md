# Forge Accessibility Contract

Source of record: prd-merged/05 UI-7 plus legacy docs/23 accessibility checks, ported to the v1 declarative component catalog.

| Component | Role | Accessible name rule | Keyboard/focus behavior | WCAG note |
|---|---|---|---|---|
| Stack | group | optional ariaLabel when grouping needs announcement | children in source order | No contrast obligation beyond children |
| Grid | group/grid when interactive | optional label; required if region-like | row-major by child order | Ensure responsive reflow preserves logical order |
| Scroll | region when labelled | required when independently focusable | focus enters region then children | Keyboard scrolling required when focusable |
| Spacer | presentation | none | not focusable | No announcement |
| Divider | separator | optional label only when meaningful | not focusable | Visible separator must meet non-text contrast when meaningful |
| Card | group or region | optional; required when landmark-like | container then children | Built-in surfaces must meet AA contrast |
| Text | text/static text | text content is accessible name | not focusable | Text contrast WCAG 2.1 AA |
| Icon | img or presentation | required ariaLabel when informative; none when decorative | not focusable | Icon contrast AA for meaningful icons |
| Image | img | alt required, empty only for decorative image policy | not focusable unless interactive wrapper | Non-text contrast for essential content |
| Badge | status/text | label required | not focusable | Intent colors need text contrast AA |
| Markdown | document/text | content supplies names; links require text | links in document order | Sanitized links keyboard reachable |
| Button | button | label or ariaLabel required; icon-only must use ariaLabel | tab stop when enabled; Space/Enter activate | 44x44-ish hit target where platform supports |
| TextField | textbox | label required; placeholder is not enough | tab stop; text editing keys | Error text associated via describedby |
| TextArea | textbox multiline | label required | tab stop; multiline editing keys | Error text associated via describedby |
| Select | combobox/listbox | label required | tab stop; arrows navigate; Enter selects | Popup respects focus trap |
| MultiSelect | listbox multiselect | label required | tab stop; arrows navigate; Space toggles | Selected state announced |
| Checkbox | checkbox | label or ariaLabel required | tab stop; Space toggles | Checked state announced |
| Switch | switch | label or ariaLabel required | tab stop; Space toggles | On/off state announced |
| Slider | slider | label required plus min/max/value | tab stop; arrows adjust by step | Value text announced |
| DatePicker | combobox/date input | label required | tab stop; calendar popup traps focus while open | Date format announced or described |
| List | list | optional list label | items in order; virtualized item positions announced when known | Empty state announced |
| Table | table/grid | caption/ariaLabel required when standalone | cell navigation for grid mode; sort buttons keyboardable | Header associations required |
| Chart | img/group | summary required | not focusable unless interactive | Do not rely on color alone |
| Stat | status/text | label required | not focusable | Delta intent must have text/icon cue |
| Tabs | tablist/tab/tabpanel | tab labels required | arrows move tabs; Tab enters panel | Active tab state announced |
| Modal | dialog | title required | focus moves into dialog; Escape/onClose; trap focus | Backdrop contrast not sufficient as only cue |
| Form | form | label required for each control | source order; submit reachable last unless explicitly placed | Validation errors associated and summarized |

## Form Label-Presence Rule

Form enforces label presence for interactive descendants at type-check/render validation time. TextField, TextArea, Select, MultiSelect, Slider, and DatePicker require label. Checkbox, Switch, and Button require label or ariaLabel; icon-only Button must provide ariaLabel. Placeholder text never counts as a label.

## Focus Order

Stack follows child source order. Grid follows row-major child order after responsive layout but must preserve logical source order for assistive tech. Tabs exposes a tablist first, then the active panel; inactive panels are not in the tab order. Modal moves focus to the first focusable child or the dialog title, traps focus while open, and restores focus to the opener on close.

## Unknown Component Fallback

UI-6 fallback must be announced as a labelled group such as "Unsupported component Chart3D". It must not expose raw JSON as the accessible name, must not be focusable unless it contains focusable known children, and must keep rendering known descendants in source order.

## Ambiguous Name Sources To Decide

- Icon-only Button: require ariaLabel; do not infer a label from icon name.
- Icon: decorative vs informative needs an explicit ariaLabel or decorative policy.
- Chart: summary is required because graphical marks are not enough.
- Image: alt is required, with empty alt allowed only when the Image is explicitly decorative.
- Table: standalone tables need caption or ariaLabel; nested table-like summaries may derive the name from surrounding text only if the renderer can associate it explicitly.
