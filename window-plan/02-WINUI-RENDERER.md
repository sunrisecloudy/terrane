# 02 — The WinUI 3 renderer for the UI tree protocol

**Project:** forge (Windows shell, milestone M6 per prd-merged/05 §3)
**Scope of this document:** the declarative component-tree renderer only — the C#/WinUI 3 layer that turns the core's UI tree + patch stream into a live `FrameworkElement` visual tree, routes events back to the core, and passes the renderer conformance kit (UI-14). It does **not** cover the FFI binding surface (see `01-FFI-AND-CORE-LIB.md`), the platform-app surfaces (editor/data-browser, prd-merged/05 §B), or packaging (see `04-MSIX-PACKAGING.md`).

**The contract this renderer obeys (read before implementing):**
- The catalog and node shapes: `/Users/vehasuwat/Project/terrane/forge/std/ui-catalog.d.ts`, `/Users/vehasuwat/Project/terrane/forge/spec/ui-catalog.md`.
- The patch vocabulary and exact wire shapes: `/Users/vehasuwat/Project/terrane/forge/crates/ui/src/patch.rs` (the `Patch` enum) and `.../node.rs` (the manual serde — **TS-facing camelCase field names** on the wire: `direction`, `onTap`, `onChange`, `text`, `testId`).
- The conformance corpus this renderer must pass: `/Users/vehasuwat/Project/terrane/forge/crates/ui/tests/golden/` (manifest + 20 fixtures) and the Rust harness `.../tests/golden.rs`.
- Normative requirements: **UI-1** (apply minimal patches), **UI-2** (full catalog), **UI-5** (shell-side virtualization via `ItemsRepeater`), **UI-6** (unknown → labeled fallback, never crash), **UI-8** (theming/semantic variants → XAML styles), **UI-12/UI-14** (golden + conformance kit).

**The cardinal rule (prd-merged/06 intro):** the shell contains *no business logic*. The renderer never decides *what* to show or *what* an action does — it only maps `Node → FrameworkElement`, applies `Patch`es, and marshals `ActionRef` strings back as events. All state lives in the core.

---

## Section index

1. Architecture & data flow
2. The wire model: deserializing nodes & patches in C#
3. The `Node → FrameworkElement` factory (full catalog)
4. The patch applier (the live-tree mutation loop)
5. Event routing: `ActionRef` → core events
6. UI-6: the unknown-node fallback (normative)
7. Theming (UI-8): semantic variants/sizes → XAML styles + dark mode
8. Virtualization & 60 fps for 100k rows (UI-5)
9. Accessibility (UI-7)
10. The renderer conformance kit (UI-14): running golden trees through C#
11. Project layout, dependencies & versions
12. Acceptance checklist

---

## 1. Architecture & data flow

```
 ┌──────────────────────────────── forge-core (Rust, native DLL) ─────────────┐
 │  WorkspaceCore::handle(Command) → state                                    │
 │  ctx.ui.render(tree)  →  forge_ui::diff(old, new)  →  Vec<Patch>           │
 │  Stream<Payload="ui.patch">  (CR-A1)                                        │
 └───────────────┬─────────────────────────────────────────────▲─────────────┘
                 │ patches (JSON, UTF-8)                         │ events (JSON)
                 │  via C-ABI/UniFFI callback (see 01-FFI)       │  command: runtime.run / ui.event
   ┌─────────────▼─────────────────────────────────────────────┴─────────────┐
   │  C# WinUI 3 shell — THIS DOCUMENT                                         │
   │                                                                          │
   │  PatchStreamSubscriber ──► UiTreeRenderer (UI thread, DispatcherQueue)   │
   │      • holds the live Node model (shadow tree) + the FrameworkElement    │
   │        visual tree, both keyed by the same index path                    │
   │      • NodeFactory.Create(node) → FrameworkElement                       │
   │      • PatchApplier.Apply(patch) mutates BOTH trees in lockstep          │
   │      • ActionDispatcher: onTap/onChange handlers → EventSink.Send(...)   │
   └──────────────────────────────────────────────────────────────────────────┘
```

Key invariants:

- **One UI thread.** All visual-tree mutation happens on the WinUI `DispatcherQueue`. The patch stream arrives on a core/FFI callback thread; the subscriber marshals each patch batch onto the dispatcher with `DispatcherQueue.TryEnqueue`.
- **Two parallel trees, one index path.** We keep a C# *shadow model* (`RNode`, a faithful 1:1 of the Rust `Node`) **and** the `FrameworkElement` visual tree. Patches address nodes by **index path** (`[]` = root, `[0,2]` = third child of first child) — identical to `forge_ui::patch::Path`. Both trees must stay structurally identical so a path resolves the same node in each. This is what lets us apply `update_text` / `update_prop` granularly without re-creating subtrees.
- **The shadow model is required, not optional.** `update_prop{key:"label"}` carries only the new value; to re-render correctly after a *later* `replace` we need the full current node. The shadow model is the renderer's source of truth for the current frame; the visual tree is its projection.
- **The renderer is pure projection.** It never originates a patch. It only consumes patches and emits *events*.

---

## 2. The wire model: deserializing nodes & patches in C#

We mirror the Rust `Node`/`Patch` shapes exactly. Use `System.Text.Json` (ships in the .NET runtime, no extra package) with a custom converter for `RNode` because the wire is **discriminated on `"type"` but must accept unknown tags** (UI-6) — `System.Text.Json` polymorphism attributes throw on unknown discriminators, so we hand-roll it exactly like the Rust `NodeVisitor` does.

```csharp
// Forge.Shell.Renderer/Model/RNode.cs
namespace Forge.Shell.Renderer.Model;

// Faithful shadow of forge_ui::node::Node. camelCase wire names.
public abstract record RNode
{
    public string? Id { get; init; }        // wire "id"
    public string? TestId { get; init; }     // wire "testId" (load-bearing for tests)
    public abstract string TypeName { get; } // wire "type"
}

public sealed record StackNode : RNode
{
    public override string TypeName => "Stack";
    public string Direction { get; init; } = "v";        // "h" | "v"
    public string? Gap { get; init; }                     // none|xs|sm|md|lg
    public string? Align { get; init; }                   // start|center|end|stretch
    public IReadOnlyList<RNode> Children { get; init; } = Array.Empty<RNode>();
}

public sealed record TextNode : RNode
{
    public override string TypeName => "Text";
    public string Text { get; init; } = "";
    public string? Variant { get; init; }                 // body|caption|title|subtitle|monospace
    public string? Intent { get; init; }
}

public sealed record ButtonNode : RNode
{
    public override string TypeName => "Button";
    public string Label { get; init; } = "";
    public string? Variant { get; init; }                 // primary|secondary|destructive|ghost
    public string? Size { get; init; }                    // s|m|l
    public string? OnTap { get; init; }                   // ActionRef
}

public sealed record TextFieldNode : RNode
{
    public override string TypeName => "TextField";
    public string Value { get; init; } = "";
    public string? Label { get; init; }
    public string? Placeholder { get; init; }
    public string? OnChange { get; init; }                // ActionRef
}

public sealed record ListNode : RNode
{
    public override string TypeName => "List";
    public IReadOnlyList<RNode> Items { get; init; } = Array.Empty<RNode>();
    public bool Virtualized { get; init; }
    public RNode? EmptyState { get; init; }
}

// ... one record per catalog member (Section 3 lists them all) ...

// UI-6: any object whose "type" is not a known catalog member.
public sealed record UnknownNode : RNode
{
    private readonly string _typeName;
    public override string TypeName => _typeName;
    // Verbatim original object (type key included) so we lose nothing and
    // can re-serialize round-trip-identically, exactly like Rust Node::Unknown.
    public JsonObject Raw { get; init; } = new();
    public UnknownNode(string typeName) => _typeName = typeName;
}
```

The converter (sketch) — note it **never throws on an unknown `type`**, matching `node.rs`:

```csharp
// Forge.Shell.Renderer/Model/RNodeJsonConverter.cs
public sealed class RNodeJsonConverter : JsonConverter<RNode>
{
    public override RNode Read(ref Utf8JsonReader reader, Type t, JsonSerializerOptions o)
    {
        using var doc = JsonDocument.ParseValue(ref reader);
        var obj = doc.RootElement;
        // No string "type" at all → Unknown fallback (UI-6), never an error.
        if (!obj.TryGetProperty("type", out var tEl) || tEl.ValueKind != JsonValueKind.String)
            return new UnknownNode(tEl.ValueKind == JsonValueKind.Undefined ? "" : tEl.ToString())
                   { Raw = JsonObject.Create(obj)! };

        string id   = Str(obj, "id");
        string tid  = Str(obj, "testId");
        return tEl.GetString() switch
        {
            "Stack"     => new StackNode     { Id=id, TestId=tid, Direction=Str(obj,"direction","v"),
                                               Gap=NullableStr(obj,"gap"), Align=NullableStr(obj,"align"),
                                               Children=Kids(obj,"children",o) },
            "Text"      => new TextNode      { Id=id, TestId=tid, Text=Str(obj,"text"),
                                               Variant=NullableStr(obj,"variant"), Intent=NullableStr(obj,"intent") },
            "Button"    => new ButtonNode    { Id=id, TestId=tid, Label=Str(obj,"label"),
                                               Variant=NullableStr(obj,"variant"), Size=NullableStr(obj,"size"),
                                               OnTap=NullableStr(obj,"onTap") },
            "TextField" => new TextFieldNode { Id=id, TestId=tid, Value=Str(obj,"value"),
                                               Label=NullableStr(obj,"label"), Placeholder=NullableStr(obj,"placeholder"),
                                               OnChange=NullableStr(obj,"onChange") },
            "List"      => new ListNode      { Id=id, TestId=tid, Items=Kids(obj,"items",o),
                                               Virtualized=Bool(obj,"virtualized") },
            // ... remaining catalog members ...
            // Unknown catalog member → forward-compatible fallback (UI-6).
            var name    => new UnknownNode(name!) { Raw = JsonObject.Create(obj)! }
        };
    }

    public override void Write(Utf8JsonWriter w, RNode n, JsonSerializerOptions o) { /* mirror node.rs Serialize, camelCase, fixed order */ }
}
```

Patches deserialize straight off `patch.rs`:

```csharp
// Forge.Shell.Renderer/Model/Patch.cs   (tag = "op", snake_case)
public abstract record Patch { public required int[] Path { get; init; } }
public sealed record ReplacePatch    : Patch { public required RNode  Node  { get; init; } }
public sealed record UpdateTextPatch : Patch { public required string Value { get; init; } }
public sealed record UpdatePropPatch : Patch { public required string Key { get; init; }
                                               public required string Value { get; init; } }
public sealed record InsertPatch     : Patch { public required RNode  Node  { get; init; } }
public sealed record RemovePatch     : Patch { /* path only */ }
```

A `PatchJsonConverter` reads `"op"` and dispatches to these five. These five are the **entire** patch vocabulary — see `patch.rs` lines 30–69. There is no `move` op (M0a has no keyed reconciliation; reorder shows up as `update_text`/`update_prop` per `diff_reordered_list_index_updates.json`).

---

## 3. The `Node → FrameworkElement` factory (full catalog)

`NodeFactory.Create(RNode)` returns a `FrameworkElement` and **tags it** with the node identity so events and accessibility work. Every produced element carries:

- `element.Tag = new NodeTag(node)` — the live shadow node (so the action dispatcher can read `OnTap`/`OnChange` at event time).
- `AutomationProperties.AutomationId = node.TestId ?? node.Id` (UI-7 + the conformance kit keys off `testId`).
- `AutomationProperties.Name` from the node's accessible name (label/text/ariaLabel).

### 3.1 Catalog → control map

| Node `type` | WinUI 3 control | Notes |
|---|---|---|
| `Stack` (h/v) | `StackPanel` (`Orientation`) | `gap` → `Spacing`; `align` → `HorizontalAlignment`/cross-axis |
| `Grid` | `Microsoft.UI.Xaml.Controls.Grid` | `columns` → uniform `ColumnDefinitions`; `auto` → `UniformGridLayout` in an `ItemsRepeater` |
| `Scroll` | `ScrollViewer` | `axis` → `Horizontal/VerticalScrollBarVisibility` |
| `Spacer` | `Border` (fixed size) | size token → `Width`/`Height` from theme |
| `Divider` | `Border` (1px) or `MenuFlyoutSeparator` | `orientation` |
| `Card` | `Border` with `CornerRadius` + `Background` | `variant` → style key; `intent` → border brush |
| `Text` | `TextBlock` | `variant` → style; `intent` → `Foreground`; `TextWrapping=Wrap` |
| `Icon` | `FontIcon` / `SymbolIcon` | `name` mapped via icon token table; unknown name → fallback glyph + ariaLabel |
| `Image` | `Image` (with `alt` → `AutomationProperties.Name`) | invalid `src` → labeled fallback `Border` |
| `Badge` | `Border` pill + `TextBlock` | `intent` → background brush |
| `Markdown` | `RichTextBlock` (sanitized subset) | only `allowedElements`; everything else → plain text |
| `Button` | `Button` | `variant`/`size` → style; `Click` → dispatch `OnTap` |
| `TextField` | `TextBox` (with `Header`) | controlled: `TextChanged` → dispatch `OnChange`; see §3.3 |
| `TextArea` | `TextBox` `AcceptsReturn=true` `TextWrapping=Wrap` | `minRows` → `MinHeight` |
| `Select` | `ComboBox` | `options` → items; `SelectionChanged` → `OnChange` |
| `MultiSelect` | `ListView` `SelectionMode=Multiple` in a flyout, or token box | `OnChange` carries selected `values[]` |
| `Checkbox` | `CheckBox` | `Checked/Unchecked` → `OnChange` |
| `Switch` | `ToggleSwitch` | `Toggled` → `OnChange` |
| `Slider` | `Slider` | `min/max/step`; `ValueChanged` → `OnChange` (debounced) |
| `DatePicker` | `CalendarDatePicker` | ISO `value`; `DateChanged` → `OnChange` |
| `List` | **`ItemsRepeater`** in a `ScrollViewer` | virtualized; see §8 |
| `Table` | **`ItemsRepeater`** rows + header `Grid` | virtualized; sort/select via `onSort`/`onSelect`; §8 |
| `Chart` | `Microsoft.UI.Xaml.Controls` canvas / community chart | `summary` → `AutomationProperties.Name`; deferred-detail OK |
| `Stat` | `StackPanel` (value + delta) | `delta.intent` → brush |
| `Tabs` | `TabView` or `Pivot` | `active` selects; `SelectionChanged` → `OnChange` |
| `Modal` | `ContentDialog` | `open` toggles `ShowAsync`/`Hide`; `onClose` |
| `Form` | `StackPanel` + submit `Button` | `validation` state; `onSubmit` |
| *anything else* | **labeled fallback box** (UI-6) | §6 |

### 3.2 Factory sketch (M0a subset spelled out; rest follow the same shape)

```csharp
// Forge.Shell.Renderer/NodeFactory.cs
public sealed class NodeFactory
{
    private readonly Theme _theme;
    private readonly IActionDispatcher _dispatch;

    public FrameworkElement Create(RNode node)
    {
        FrameworkElement fe = node switch
        {
            StackNode s     => CreateStack(s),
            TextNode t      => CreateText(t),
            ButtonNode b    => CreateButton(b),
            TextFieldNode f => CreateTextField(f),
            ListNode l      => CreateList(l),
            // ... full catalog ...
            UnknownNode u   => CreateFallback(u),     // UI-6, §6
            _               => CreateFallback(node)    // defensive; never throws
        };

        fe.Tag = new NodeTag(node);                    // event-time lookup
        ApplyAccessibility(fe, node);                  // §9
        if (node is BaseHasVisible { Visible: false }) fe.Visibility = Visibility.Collapsed;
        return fe;
    }

    private StackPanel CreateStack(StackNode s)
    {
        var panel = new StackPanel
        {
            Orientation = s.Direction == "h" ? Orientation.Horizontal : Orientation.Vertical,
            Spacing     = _theme.GapToPixels(s.Gap),          // none/xs/sm/md/lg → double
        };
        ApplyAlign(panel, s.Align);
        foreach (var child in s.Children)
            panel.Children.Add(Create(child));                // recursion; index == child index
        return panel;
    }

    private TextBlock CreateText(TextNode t) => new()
    {
        Text         = t.Text,
        Style        = _theme.TextStyle(t.Variant),           // body/caption/title/...
        Foreground   = _theme.IntentBrush(t.Intent),
        TextWrapping  = TextWrapping.Wrap,
    };

    private Button CreateButton(ButtonNode b)
    {
        var btn = new Button { Content = b.Label, Style = _theme.ButtonStyle(b.Variant, b.Size) };
        // UI-6 nuance (ui-catalog.md): if onTap is absent, the button is inert but still rendered.
        if (b.OnTap is { } action)
            btn.Click += (_, _) => _dispatch.Tap(((NodeTag)btn.Tag).Node, action);
        return btn;
    }

    private TextBox CreateTextField(TextFieldNode f)
    {
        var box = new TextBox { Text = f.Value, Header = f.Label, PlaceholderText = f.Placeholder };
        WireControlledTextBox(box, f);                         // §3.3
        return box;
    }

    private FrameworkElement CreateList(ListNode l) => ListVirtualizer.Build(l, this); // §8
}
```

### 3.3 Controlled inputs (UI-4)

Inputs are **controlled**: the source of truth is the node's `value`, not the `TextBox`. On edit we (a) fire `onChange` to the core with the typed value, and (b) the core re-renders and emits a patch that sets `value` — which we apply. To avoid feedback loops and caret jumps:

```csharp
private void WireControlledTextBox(TextBox box, TextFieldNode f)
{
    box.TextChanged += (_, _) =>
    {
        var tag = (NodeTag)box.Tag;
        if (tag.Suppress) return;                      // we're mid-patch, ignore echo
        if (((TextFieldNode)tag.Node).OnChange is { } action)
            _dispatch.Change(tag.Node, action, box.Text);   // payload carries the new string
    };
}
```

When a patch later sets `value` on this box (§4), the applier sets `tag.Suppress = true`, updates `box.Text` only if it actually differs (preserving the caret when possible), then clears `Suppress`. Input → patched frame target is **< 16 ms p95 desktop** (UI-4); the dispatch is async fire-and-forget so typing is never blocked on the core.

---

## 4. The patch applier (the live-tree mutation loop)

`PatchApplier.Apply(IReadOnlyList<Patch> batch)` runs on the dispatcher and mutates **both** the shadow model and the visual tree, addressing nodes by the same index path. It is the C# twin of `forge_ui::patch::apply` (`patch.rs` lines 379–435) and must reproduce its semantics exactly so the conformance kit passes.

Mapping of the five ops onto WinUI:

| Patch | Shadow model | Visual tree |
|---|---|---|
| `replace{path,node}` | swap node at path | rebuild subtree: `parent.Children[i] = NodeFactory.Create(node)` |
| `update_text{path,value}` | set `Text` on `TextNode` | set `TextBlock.Text = value` (no re-create) |
| `update_prop{path,key,value}` | set the keyed field | set the one control property; for `value` on `TextField`, suppress echo |
| `insert{path,node}` | insert child at last index | `parentPanel.Children.Insert(i, Create(node))` |
| `remove{path}` | remove child at last index | `parentPanel.Children.RemoveAt(i)` |

Two helpers mirror the Rust ones:

- `ResolveVisual(int[] path)` walks `Children` by index, like `resolve_mut` in `patch.rs`.
- `ContainerChildren(FrameworkElement)` returns the mutable child collection for `StackPanel.Children` / `ItemsRepeater`-backed list source (the only containers, matching `children_mut`: Stack→children, List→items). A leaf target for `insert`/`remove` is a renderer bug — log + skip, never throw (mirrors the Rust error but the shell must not crash).

```csharp
// Forge.Shell.Renderer/PatchApplier.cs
public sealed class PatchApplier
{
    private RNode _root;                       // shadow model
    private FrameworkElement _rootElement;     // visual tree root
    private readonly NodeFactory _factory;

    public void Apply(IReadOnlyList<Patch> batch)
    {
        // Single batch = one frame. Apply in order; the core guarantees order.
        foreach (var p in batch) ApplyOne(p);
    }

    private void ApplyOne(Patch p)
    {
        switch (p)
        {
            case ReplacePatch r:
            {
                _root = ReplaceInModel(_root, r.Path, r.Node);
                var (parent, i) = ResolveParent(r.Path);
                if (parent is null) { SwapRoot(r.Node); break; }     // path == []
                var newFe = _factory.Create(r.Node);
                ContainerChildren(parent)[i] = newFe;                // replace child
                break;
            }
            case UpdateTextPatch t:
            {
                _root = UpdateTextInModel(_root, t.Path, t.Value);
                var fe = ResolveVisual(t.Path);
                if (fe is TextBlock tb) tb.Text = t.Value;            // no subtree churn
                else LogMismatch("update_text", fe);                 // never crash
                break;
            }
            case UpdatePropPatch up:
            {
                _root = UpdatePropInModel(_root, up.Path, up.Key, up.Value);
                ApplyProp(ResolveVisual(up.Path), up.Key, up.Value);  // one property
                break;
            }
            case InsertPatch ins:
            {
                var parentPath = ins.Path[..^1];
                int i = ins.Path[^1];
                _root = InsertInModel(_root, parentPath, i, ins.Node);
                var parent = ResolveVisual(parentPath);
                ContainerChildren(parent).Insert(i, _factory.Create(ins.Node));
                break;
            }
            case RemovePatch rem:
            {
                var parentPath = rem.Path[..^1];
                int i = rem.Path[^1];
                _root = RemoveInModel(_root, parentPath, i);
                var parent = ResolveVisual(rem.Path[..^1]);
                ContainerChildren(parent).RemoveAt(i);
                break;
            }
        }
    }

    // ApplyProp maps the wire key → exactly one control property, mirroring
    // forge_ui apply_prop (patch.rs 438–496). Order matters: base keys first.
    private void ApplyProp(FrameworkElement fe, string key, string value)
    {
        switch (fe, key)
        {
            case (FrameworkElement el, "id"):     el.Tag = WithId((NodeTag)el.Tag, value);     break;
            case (FrameworkElement el, "testId"): AutomationProperties.SetAutomationId(el, value); break;
            case (StackPanel sp,  "gap"):         sp.Spacing = _theme.GapToPixels(value);        break;
            case (TextBlock tb,   "variant"):     tb.Style = _theme.TextStyle(value);            break;
            case (Button btn,     "label"):       btn.Content = value;                           break;
            case (Button btn,     "variant"):     btn.Style = _theme.ButtonStyle(value, null);   break;
            case (Button btn,     "onTap"):       RebindTap(btn, value);                         break;
            case (TextBox box,    "value"):       SetControlledText(box, value);                 break; // §3.3 suppress
            case (TextBox box,    "label"):       box.Header = value;                            break;
            case (TextBox box,    "placeholder"): box.PlaceholderText = value;                   break;
            case (TextBox box,    "onChange"):    RebindChange(box, value);                      break;
            default: LogUnknownProp(fe, key); break;     // ignore, never crash (UI-6 spirit)
        }
    }
}
```

**Why granular ops, not full re-render:** `update_text`/`update_prop` touch exactly one property of one existing control — no layout invalidation of siblings, no loss of focus/caret/scroll. `replace` is the only op that re-creates a subtree, and only when the node *type* changed (or a layout axis flipped, or a scalar was cleared — see `patch.rs` `any_scalar_cleared`). This is what keeps `update_text`-heavy frames (e.g. the reorder fixture) cheap.

**Worked example** — the append fixture `diff_child_append.json` produces `[{op:insert, path:[2], node:Button}]`. The applier: resolves parent at `[]` (the root `StackPanel`), inserts a freshly-built `Button` at index 2 of `Children`, and inserts the `ButtonNode` at index 2 of the shadow `StackNode.Children`. Result tree == new tree. The nested action change `diff_nested_button_action_change.json` produces `[{op:update_prop, path:[1,0], key:"onTap", value:"job.run.now"}]` → resolve `Children[1].Children[0]` (the `Button`), rebind its `Click` to dispatch the new action. Nothing else moves.

---

## 5. Event routing: `ActionRef` → core events

`onTap`/`onChange`/`onSubmit`/`onClose`/`onSort`/`onSelect` are **`ActionRef` strings** (`node.rs`: `pub type ActionRef = String`). The renderer never interprets them — it sends them back to the core as events. The shell owns no logic about what `"message.archive"` means.

```csharp
// Forge.Shell.Renderer/ActionDispatcher.cs
public interface IActionDispatcher
{
    void Tap(RNode node, string action);
    void Change(RNode node, string action, JsonNode payload);
}

public sealed class ActionDispatcher(IEventSink core) : IActionDispatcher
{
    // The core's event-queue command. Per CR-A1/CR-6 events route through commands.
    public void Tap(RNode node, string action) =>
        core.Send(new UiEvent {
            Action  = action,                    // the ActionRef verbatim
            NodeId  = node.TestId ?? node.Id,    // which element fired it
            Kind    = "tap",
            Payload = null
        });

    public void Change(RNode node, string action, JsonNode payload) =>
        core.Send(new UiEvent { Action = action, NodeId = node.TestId ?? node.Id,
                                Kind = "change", Payload = payload });
}
```

`IEventSink.Send` is the FFI command call (`01-FFI-AND-CORE-LIB.md`): it serializes the `UiEvent` to JSON and invokes the core command that enqueues the event (e.g. `runtime.run`/the UI event command per CR-A2). The core processes the event handler, may write storage, calls `ctx.ui.render(newTree)`, the core diffs, and a new patch batch arrives on the stream — closing the loop headlessly-validated in UI-12 (*simulate onTap → expect Modal in next patch*). The renderer is symmetric with the CLI harness: same event in, same patch out.

**Payload shape** (must match what the core's handler reads):
- `tap`: no payload.
- `change` on `TextField/TextArea`: `{ "value": "<string>" }`.
- `change` on `Checkbox/Switch`: `{ "checked": <bool> }`.
- `change` on `Select`: `{ "value": "<optionValue>" }`; `MultiSelect`: `{ "values": [...] }`.
- `change` on `Slider`: `{ "value": <number> }` (debounced ~50 ms so dragging doesn't flood the queue).
- `sort` on `Table`: `{ "columnId": "...", "direction": "asc|desc" }`.

**Inert actions (UI-6 / ui-catalog.md):** a `Button` with no `onTap` renders but is non-interactive — never wire a handler to `null`. An *unknown* action string is still sent verbatim; the core decides if it is a no-op.

---

## 6. UI-6: the unknown-node fallback (NORMATIVE)

> "unknown component types render as a labeled fallback box, never crash; unknown props ignored; a v1 client renders a v3 applet usably degraded." (UI-6)

This is **release-blocking and fuzz-tested** (prd-merged/05 §5: "Unknown-component/prop fuzz → zero crashes, 100% fallback rendering"). Three cases, all handled in the converter (§2) so they never reach the factory as exceptions:

1. **Unknown `type`** → `UnknownNode` carrying the verbatim `Raw` object. The factory renders a labeled box:

```csharp
private FrameworkElement CreateFallback(RNode node)
{
    var typeName = node.TypeName;
    var box = new Border {
        Style = _theme.FallbackBoxStyle,                  // dashed border, muted bg
        Child = new StackPanel { Children = {
            new TextBlock { Text = $"Unsupported component: {typeName}",
                            Style = _theme.TextStyle("caption") },
            // Render any Text-coercible props as labeled lines (UI-6: "Text-coercible props").
            CoercePropsToText(node)
        }}
    };
    AutomationProperties.SetName(box, $"Unsupported component {typeName}");
    // Critically: keep rendering KNOWN descendants if present (ui-catalog.md note).
    foreach (var child in KnownChildrenOf(node))
        ((StackPanel)box.Child).Children.Add(Create(child));
    return box;
}
```

2. **Unknown prop on a known node** (e.g. `sparkle:true` on a `Button`, fixture `unknown_button_extra_prop.json`) → the converter simply never reads it; the node renders normally. No error.

3. **Unknown node nested in a known container** (fixtures `unknown_future_widget_child.json`, `unknown_nested_in_list.json`) → the parent renders normally; the unknown child becomes a fallback box in place. Known siblings are unaffected.

**Round-trip guarantee:** because `UnknownNode.Raw` is verbatim, re-serializing reproduces the original object byte-for-shape — the renderer is lossless, so a `replace` that swaps an unknown for a future known type still works, and the self-diff of an unknown-bearing tree is empty (matching the Rust `run_unknown` assertion in `golden.rs`).

**Fuzz test (acceptance):** feed 10k randomly-mutated trees (random type names, random extra props, random nesting) through the C# converter + factory; assert **zero exceptions** and **every unknown rendered a fallback box**. Wire this as an xUnit theory seeded from a generator; it is the C# analog of UI-6's normative fuzz gate.

---

## 7. Theming (UI-8): semantic variants/sizes → XAML styles + dark mode

Applets are **semantic, never pixel-specified** (UI-3): `variant: "primary"`, `size: "l"`, `intent: "danger"`. Concrete pixels are the shell's job. We map each semantic token to a **named XAML `Style`** in a `ResourceDictionary`, and reuse WinUI's built-in light/dark theme resources so dark mode and high-contrast come for free.

```xml
<!-- Forge.Shell/Themes/ForgeTheme.xaml -->
<ResourceDictionary
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">

  <ResourceDictionary.ThemeDictionaries>
    <ResourceDictionary x:Key="Light">
      <SolidColorBrush x:Key="ForgeAccentBrush"  Color="#0B5FFF"/>
      <SolidColorBrush x:Key="ForgeDangerBrush"  Color="#C42B1C"/>
      <SolidColorBrush x:Key="ForgeSuccessBrush" Color="#0F7B0F"/>
    </ResourceDictionary>
    <ResourceDictionary x:Key="Dark">
      <SolidColorBrush x:Key="ForgeAccentBrush"  Color="#5B8DEF"/>
      <SolidColorBrush x:Key="ForgeDangerBrush"  Color="#FF99A4"/>
      <SolidColorBrush x:Key="ForgeSuccessBrush" Color="#6CCB5F"/>
    </ResourceDictionary>
    <ResourceDictionary x:Key="HighContrast">
      <SolidColorBrush x:Key="ForgeAccentBrush"  Color="{ThemeResource SystemColorHighlightColor}"/>
    </ResourceDictionary>
  </ResourceDictionary.ThemeDictionaries>

  <!-- Button variants -->
  <Style x:Key="Forge.Button.primary" TargetType="Button" BasedOn="{StaticResource AccentButtonStyle}"/>
  <Style x:Key="Forge.Button.secondary" TargetType="Button" BasedOn="{StaticResource DefaultButtonStyle}"/>
  <Style x:Key="Forge.Button.destructive" TargetType="Button" BasedOn="{StaticResource DefaultButtonStyle}">
    <Setter Property="Foreground" Value="{ThemeResource ForgeDangerBrush}"/>
  </Style>
  <Style x:Key="Forge.Button.ghost" TargetType="Button">
    <Setter Property="Background" Value="Transparent"/>
  </Style>

  <!-- Text variants -->
  <Style x:Key="Forge.Text.body"     TargetType="TextBlock" BasedOn="{StaticResource BodyTextBlockStyle}"/>
  <Style x:Key="Forge.Text.caption"  TargetType="TextBlock" BasedOn="{StaticResource CaptionTextBlockStyle}"/>
  <Style x:Key="Forge.Text.title"    TargetType="TextBlock" BasedOn="{StaticResource TitleTextBlockStyle}"/>
  <Style x:Key="Forge.Text.subtitle" TargetType="TextBlock" BasedOn="{StaticResource SubtitleTextBlockStyle}"/>
  <Style x:Key="Forge.Text.monospace" TargetType="TextBlock">
    <Setter Property="FontFamily" Value="Cascadia Code, Consolas, monospace"/>
  </Style>

  <!-- Fallback box (UI-6) -->
  <Style x:Key="Forge.FallbackBox" TargetType="Border">
    <Setter Property="BorderBrush" Value="{ThemeResource SystemControlForegroundBaseMediumBrush}"/>
    <Setter Property="BorderThickness" Value="1"/>
    <Setter Property="CornerRadius" Value="4"/>
    <Setter Property="Padding" Value="8"/>
  </Style>
</ResourceDictionary>
```

The `Theme` C# helper resolves tokens to these style keys, with a **safe default for unknown tokens** (forward-compat: a `variant:"v3-novel"` falls back to the default style, never throws):

```csharp
public Style ButtonStyle(string? variant, string? size) =>
    _res.TryGet($"Forge.Button.{variant ?? "secondary"}") ?? _res.Get("Forge.Button.secondary");
public Style TextStyle(string? variant) =>
    _res.TryGet($"Forge.Text.{variant ?? "body"}") ?? _res.Get("Forge.Text.body");
public Brush? IntentBrush(string? intent) => intent switch {
    "accent"  => _res.Brush("ForgeAccentBrush"),
    "danger"  => _res.Brush("ForgeDangerBrush"),
    "success" => _res.Brush("ForgeSuccessBrush"),
    "warning" => _res.Brush("ForgeWarningBrush"),
    _         => null   // neutral → inherit
};
public double GapToPixels(string? gap) => gap switch {
    "none" => 0, "xs" => 2, "sm" => 4, "md" => 8, "lg" => 16, _ => 8 };
```

**Dark mode / system theme:** set `rootElement.RequestedTheme = ElementTheme.Default` so WinUI follows the OS. `ThemeDictionaries` swap automatically. Workspace tokens (accent/radius/density from UI-8) override the brush resources at runtime by inserting an app-level `ResourceDictionary` — applets never define raw colors (the only escape hatch is `Chart` palettes, UI-8).

**Contrast (UI-7):** the built-in light/dark brush pairs are chosen to meet WCAG 2.1 AA; the a11y audit is a GA gate.

---

## 8. Virtualization & 60 fps for 100k rows (UI-5)

> "Lists/tables virtualize in the shell (native lazy lists)… 100k-row tables stay smooth without manual paging." (UI-5) — acceptance: "100k-row table at 60 fps desktop." (prd-merged/05 §5)

Use **`ItemsRepeater`** (the renderer table in prd-merged/05 §3 names it explicitly) inside a `ScrollViewer`. `ItemsRepeater` realizes only visible elements via UI virtualization. Two backing strategies:

### 8.1 List/Table with materialized items (small/medium)

When the core ships the full `items`/`rows` array, bind an `ItemsRepeater` to an `ObservableCollection<RNode>` (List) or row-model collection (Table). The element factory is our `NodeFactory`:

```csharp
// Forge.Shell.Renderer/ListVirtualizer.cs
public static FrameworkElement Build(ListNode list, NodeFactory factory)
{
    var source = new ObservableCollection<RNode>(list.Items);   // patches mutate THIS
    var repeater = new ItemsRepeater
    {
        ItemsSource = source,
        Layout      = new StackLayout { Spacing = 0 },          // linear; UniformGridLayout for Grid
        ItemTemplate = new ForgeNodeTemplate(factory)            // IElementFactory → FrameworkElement
    };
    var scroller = new ScrollViewer { Content = repeater, VerticalScrollBarVisibility = ScrollBarVisibility.Auto };
    // List/Table containers expose their item collection to the PatchApplier so
    // insert/remove/update target the ObservableCollection (which ItemsRepeater observes).
    scroller.Tag = new ContainerTag(source);
    if (list.Items.Count == 0 && list.EmptyState is { } empty)
        return factory.Create(empty);
    return scroller;
}
```

`ForgeNodeTemplate` implements `Microsoft.UI.Xaml.IElementFactory`; its `GetElement` calls `factory.Create(node)` and `RecycleElement` clears the recycled element's `Tag`. `ItemsRepeater` recycles elements as you scroll, so memory stays bounded regardless of row count. **Patch ops on a List target the `ObservableCollection`**, not raw `Children`: `insert`/`remove` map to `Insert`/`RemoveAt` on the collection; `ItemsRepeater` updates incrementally.

### 8.2 Table/List with a query handle (100k rows, UI-5)

For huge data the applet supplies a **query handle**, not the rows — "the shell pulls visible rows via the core" (UI-5). Back the `ItemsRepeater` with a **windowed/incremental data source** that fetches only visible ranges:

```csharp
// Implements ISupportIncrementalLoading + indexer that pulls a page from the core.
public sealed class CoreRowSource : IList, ISupportIncrementalLoading
{
    private readonly IRowQuery _query;     // FFI: query.execute with offset/limit (CR-A2)
    private readonly int _total;           // total count from the query handle
    private readonly LruCache<int, RowModel> _pages = new(capacity: 64);

    public object this[int index] => GetOrFetch(index);   // fetch page on demand, cache
    public int Count => _total;
    // ItemsRepeater asks only for realized indices → only visible pages fetched.
}
```

- Page size ~200 rows; LRU keeps a few screens of pages.
- Scrolling triggers `query.execute(offset,limit)` over FFI for the new window only; everything else is recycled. No manual paging UI.
- `sort`/`select` are `onSort`/`onSelect` `ActionRef`s sent as events (§5); the core re-runs the query and ships a `replace` on the table node (or, with stable row ids, granular item patches).

### 8.3 Hitting 60 fps

- **UI virtualization** (`ItemsRepeater`) realizes ~30 rows on screen, not 100k.
- **Recycling templates** via `IElementFactory` — no per-row allocation on scroll.
- **Patch batching:** apply a whole patch batch in one `DispatcherQueue.TryEnqueue`, between layout passes, so the compositor renders one frame, not N.
- **Off-thread JSON:** deserialize patches on the FFI/background thread; only the tree mutation runs on the UI thread.
- **No synchronous FFI on the UI thread** during scroll — page fetches are async; show a lightweight placeholder row until the page resolves.

**Measurement (acceptance):** drive a 100k-row `Table`, fling-scroll, and capture frame timing with WinAppSDK's composition/`AppCapture` or PresentMon. Gate: p95 frame time ≤ 16.6 ms (60 fps); no frame > 33 ms during a sustained scroll. Add a perf smoke to CI on a self-hosted Windows runner.

---

## 9. Accessibility (UI-7)

Every node maps to platform a11y primitives, set in `ApplyAccessibility(fe, node)`:

- `AutomationProperties.Name` ← `ariaLabel` ?? `label` ?? `text` ?? a sensible default. For `Form`, label presence on controls is enforced by std types at type-check (CR-15), so the renderer can assume names exist; it still defends with a default.
- `AutomationProperties.AutomationId` ← `testId` ?? `id` (also what the conformance kit keys on).
- Roles: WinUI controls carry correct `AutomationControlType` automatically (`Button`→Button, `TextBox`→Edit, `ItemsRepeater`→List). Custom fallback boxes set `AutomationProperties.AutomationControlType` explicitly to `Group`.
- Focus order follows visual-tree order (the index path), matching the declared tree.
- `Icon`-only buttons must carry `ariaLabel` (enforced by std types; renderer asserts a name exists or logs).

A11y audit (Accessibility Insights for Windows) is a **GA gate** (UI-7); run it against the catalog demo in CI.

---

## 10. The renderer conformance kit (UI-14): running golden trees through C#

> "Renderer conformance kit: golden trees + scripted-interaction + screenshot tests shared by all renderers; behavioral divergence is release-blocking (same bar as CR-12)." (UI-14)

The corpus already exists and is the seed: `/Users/vehasuwat/Project/terrane/forge/crates/ui/tests/golden/` — `manifest.json` + 20 fixtures across three kinds (`roundtrip`, `diff`, `unknown`). The Rust harness `golden.rs` already asserts the *core* side. The C# kit asserts the *renderer* side against the **same files** (no fork — the C# test project reads them from the Rust tree via a relative path or a copied build artifact), so divergence is impossible by construction.

The C# conformance test project (`Forge.Shell.Renderer.Conformance`, MSTest/xUnit running on WinUI `DispatcherQueue`) runs each fixture kind:

**(a) `roundtrip` cases** — parse `tree` with `RNodeJsonConverter`, re-serialize, assert byte-shape equality with the input (the C# twin of `run_roundtrip`). Then build the visual tree with `NodeFactory` and assert it does not throw and produces the expected element types (e.g. `roundtrip_nested_stack_list_button.json` → `StackPanel` whose children are `TextBlock`, list-backed `ScrollViewer`, `Button`).

**(b) `diff` cases** — load `old`, build the visual tree; deserialize `expect_patches`; run `PatchApplier.Apply`; then assert the resulting visual tree **matches the tree you'd build fresh from `new`**. Two assertion levels:

1. *Structural* (always): walk both trees and assert identical element types, `AutomationId`s, and key properties (`TextBlock.Text`, `Button.Content`, `TextBox.Text`). This catches the bulk of divergence and is deterministic/headless.
2. *Visual snapshot* (screenshot): render the element to a `RenderTargetBitmap`, compare to a committed PNG baseline with a small per-pixel tolerance. This is the "screenshot test" arm of UI-14; gate on perceptual diff. Baselines are theme- and DPI-pinned (light theme, 100% scale) to stay stable in CI.

```csharp
[TestMethod]
[DynamicData(nameof(DiffCases))]               // sourced from manifest.json kind=="diff"
public async Task DiffCase_AppliedTreeMatchesFreshNew(JsonObject fixture)
{
    var oldNode = Deserialize<RNode>(fixture["old"]!);
    var newNode = Deserialize<RNode>(fixture["new"]!);
    var patches = DeserializePatches(fixture["expect_patches"]!);

    var applier = await BuildRenderer(oldNode);          // builds shadow + visual tree
    await OnUi(() => applier.Apply(patches));

    var expected = await BuildRenderer(newNode);
    AssertVisualTreesEquivalent(applier.RootElement, expected.RootElement);   // structural
    await AssertSnapshotMatches(applier.RootElement, baseline: fixture.Name); // screenshot
}
```

**(c) `unknown` cases** — assert `must_not_error` is honored: parsing never throws, the factory renders a fallback `Border` for the unknown subtree, and known siblings still render (e.g. `unknown_future_widget_child.json`: the `Text` header renders, the `FutureWidget` becomes a labeled box). Assert zero exceptions and presence of the fallback `AutomationProperties.Name = "Unsupported component FutureWidget"`.

**(d) scripted interaction** — beyond the static corpus, add interaction scripts (the UI-12 loop): build a tree, simulate a `Button.Click` via UI Automation, assert the dispatcher sent the correct `UiEvent` (action ref + node id + payload). This proves event routing matches the headless harness (*simulate onTap → expect event → core emits next patch*). Use a fake `IEventSink` to capture sent events; assert exact JSON.

**CI wiring:** the kit runs on a Windows runner (`windows-latest` GitHub Actions or self-hosted for the perf gate). A new fixture added to the Rust `manifest.json` is *automatically* picked up by the C# `DynamicData` source — adding a renderer test costs zero C# edits, which is the point of UI-14 (one shared corpus, every renderer). Behavioral divergence (any fixture failing in C# but passing in Rust) is **release-blocking**.

**Manifest parity test:** a C# test re-implements `manifest_lists_every_golden_file` from `golden.rs` — every fixture file is in the manifest and vice versa — so the C# and Rust kits can never silently drift apart.

---

## 11. Project layout, dependencies & versions

```
forge/                         (existing Rust workspace — unchanged)
windows/                       (new; the Windows shell solution)
  Forge.Shell.sln
  src/
    Forge.Shell/                       WinUI 3 app (App.xaml, MainWindow, chrome)
      Forge.Shell.csproj
      Themes/ForgeTheme.xaml
    Forge.Shell.Renderer/              THIS DOCUMENT's library
      Forge.Shell.Renderer.csproj
      Model/  RNode.cs  Patch.cs  RNodeJsonConverter.cs  PatchJsonConverter.cs
      NodeFactory.cs  PatchApplier.cs  ListVirtualizer.cs  CoreRowSource.cs
      ActionDispatcher.cs  Theme.cs  Accessibility.cs  Fallback.cs
    Forge.Shell.Ffi/                   (other doc) generated UniFFI-C# / C-ABI bindings
  tests/
    Forge.Shell.Renderer.Conformance/  golden + screenshot + interaction tests
      (reads ../../forge/crates/ui/tests/golden/ at test time)
```

**Dependencies (pin these exact-or-newer in `.csproj`):**

| Package / SDK | Version | Why |
|---|---|---|
| .NET SDK | **8.0.x** (LTS) | target `net8.0-windows10.0.19041.0` (Win10 22H2 floor) |
| `Microsoft.WindowsAppSDK` | **1.5.x** (or current 1.x) | WinUI 3, `ItemsRepeater`, `ContentDialog`, `TabView` |
| `Microsoft.Windows.SDK.BuildTools` | matches WindowsAppSDK | MSIX tooling, manifest |
| `Microsoft.Graphics.Win2D` | **1.x** | `Chart` custom drawing (optional, deferred-detail) |
| `System.Text.Json` | in-box (net8) | node/patch (de)serialization |
| `xunit` / `MSTest.TestAdapter` | latest | conformance kit |
| `WinAppDriver` / `Microsoft.Windows.Apps.Test` | latest | UI Automation for scripted-interaction tests |

`TargetFramework`: `net8.0-windows10.0.19041.0`; `SupportedOSPlatformVersion`: `10.0.19041.0` (Windows 10 22H2 / build 19041+, per PS-14). Build for **x64 and arm64** (`<Platforms>x64;arm64</Platforms>`); the renderer is pure managed code so arm64 is free once the native FFI lib is built for arm64 (other doc).

**Build commands:**

```powershell
# from windows/
dotnet restore Forge.Shell.sln
dotnet build  Forge.Shell.sln -c Release -p:Platform=x64
dotnet build  Forge.Shell.sln -c Release -p:Platform=arm64

# run the renderer conformance kit (Windows runner)
dotnet test tests/Forge.Shell.Renderer.Conformance -c Release `
  -p:Platform=x64 --logger "trx;LogFileName=conformance.trx"
```

The renderer library has **no dependency on the FFI project** beyond the `IEventSink`/`IRowQuery` interfaces — keep those interfaces in `Forge.Shell.Renderer` and implement them in `Forge.Shell.Ffi`, so the conformance kit can run with fakes, fully headless of the native lib.

---

## 12. Acceptance checklist

A reviewer can tick these off on a real Windows machine:

- [ ] **Catalog coverage (UI-2):** every node type in `ui-catalog.d.ts` has a `NodeFactory` branch producing the control in §3.1's table; the M0a subset (Stack/Text/Button/TextField/List) is byte-faithful to `node.rs`.
- [ ] **Patch loop (UI-1):** all five ops (`replace/update_text/update_prop/insert/remove`) implemented in `PatchApplier`, mutating shadow + visual trees by index path; `update_text`/`update_prop` mutate one property with no subtree churn.
- [ ] **Golden conformance (UI-12/UI-14):** all 20 fixtures + `manifest.json` from `forge/crates/ui/tests/golden/` pass in the C# kit (roundtrip structural, diff applied==fresh-new, unknown no-throw); the C# kit reads the **same files** as `golden.rs`.
- [ ] **Screenshot tests:** committed PNG baselines (light, 100% DPI) match within tolerance for each diff fixture's `new` tree.
- [ ] **Scripted interaction:** simulating a `Button` click via UI Automation sends the exact `UiEvent` JSON (action ref + node id); a `TextField` edit sends `{value}`.
- [ ] **UI-6 fuzz (normative):** 10k mutated trees → zero exceptions, 100% fallback boxes; unknown props ignored (`unknown_button_extra_prop.json`); unknown nested in List survives (`unknown_nested_in_list.json`).
- [ ] **Theming (UI-8):** variant/size/intent tokens resolve to named styles; unknown tokens fall back safely; OS dark mode flips `ThemeDictionaries`; WCAG AA contrast on built-in brushes.
- [ ] **Virtualization (UI-5):** 100k-row `Table` via `ItemsRepeater` + `CoreRowSource`; only visible pages fetched over FFI; p95 frame ≤ 16.6 ms during fling-scroll (PresentMon).
- [ ] **A11y (UI-7):** `AutomationProperties.Name`/`AutomationId` set on every element; Accessibility Insights audit clean on the catalog demo.
- [ ] **No business logic in the shell (prd-merged/06):** grep the renderer for storage/CRDT/permission/schema access — there is none; the only outbound path is `IEventSink.Send` (commands) and `IRowQuery` (`query.execute`).
- [ ] **No-crash guarantee (CR-A4):** malformed patches/paths log + skip; the FFI boundary never panics into managed code; the app stays alive.
- [ ] **Threading:** all visual mutation on `DispatcherQueue`; JSON parse off-thread; no synchronous FFI on the UI thread during scroll.
- [ ] **Targets (PS-14):** builds and runs on Windows 11 and Windows 10 22H2+, x64 and arm64.
