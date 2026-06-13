declare module "@forge/std/ui-catalog" {
  export type JsonValue =
    | null
    | boolean
    | number
    | string
    | JsonValue[]
    | { [key: string]: JsonValue };

  export type ActionRef = string;
  export type Size = "s" | "m" | "l";
  export type Intent = "neutral" | "accent" | "success" | "warning" | "danger";
  export type Alignment = "start" | "center" | "end" | "stretch";

  export interface BaseNode {
    id?: string;
    testId?: string;
    ariaLabel?: string;
    disabled?: boolean;
    visible?: boolean;
  }

  export type Node = KnownNode;
  export type RenderableNode = KnownNode | UnknownNode;
  export type KnownNode =
    | StackNode
    | GridNode
    | ScrollNode
    | SpacerNode
    | DividerNode
    | CardNode
    | TextNode
    | IconNode
    | ImageNode
    | BadgeNode
    | MarkdownNode
    | ButtonNode
    | TextFieldNode
    | TextAreaNode
    | SelectNode
    | MultiSelectNode
    | CheckboxNode
    | SwitchNode
    | SliderNode
    | DatePickerNode
    | ListNode
    | TableNode
    | ChartNode
    | StatNode
    | TabsNode
    | ModalNode
    | FormNode;

  export interface UnknownNode extends BaseNode {
    type: string;
    children?: RenderableNode[];
    props?: { [key: string]: JsonValue };
  }

  export interface StackNode extends BaseNode {
    type: "Stack";
    direction?: "h" | "v";
    gap?: "none" | "xs" | "sm" | "md" | "lg";
    align?: Alignment;
    children: RenderableNode[];
  }

  export interface GridNode extends BaseNode {
    type: "Grid";
    columns?: number | "auto";
    gap?: "none" | "xs" | "sm" | "md" | "lg";
    children: RenderableNode[];
  }

  export interface ScrollNode extends BaseNode {
    type: "Scroll";
    axis?: "x" | "y" | "both";
    child: RenderableNode;
  }

  export interface SpacerNode extends BaseNode {
    type: "Spacer";
    size?: "xs" | "sm" | "md" | "lg" | "xl";
  }

  export interface DividerNode extends BaseNode {
    type: "Divider";
    orientation?: "horizontal" | "vertical";
  }

  export interface CardNode extends BaseNode {
    type: "Card";
    variant?: "plain" | "outlined" | "elevated";
    intent?: Intent;
    children: RenderableNode[];
  }

  export interface TextNode extends BaseNode {
    type: "Text";
    text: string;
    variant?: "body" | "caption" | "title" | "subtitle" | "monospace";
    intent?: Intent;
  }

  export interface IconNode extends BaseNode {
    type: "Icon";
    name: string;
    size?: Size;
    intent?: Intent;
  }

  export interface ImageNode extends BaseNode {
    type: "Image";
    src: string;
    alt: string;
    fit?: "contain" | "cover" | "fill";
    aspectRatio?: "square" | "video" | "wide" | "auto";
  }

  export interface BadgeNode extends BaseNode {
    type: "Badge";
    label: string;
    intent?: Intent;
    size?: Size;
  }

  export interface MarkdownNode extends BaseNode {
    type: "Markdown";
    text: string;
    allowedElements?: ("p" | "em" | "strong" | "code" | "pre" | "ul" | "ol" | "li" | "a")[];
  }

  export interface ButtonNode extends BaseNode {
    type: "Button";
    label?: string;
    icon?: string;
    variant?: "primary" | "secondary" | "destructive" | "ghost";
    size?: Size;
    onTap?: ActionRef;
  }

  export interface TextFieldNode extends BaseNode {
    type: "TextField";
    value: string;
    label?: string;
    placeholder?: string;
    required?: boolean;
    validation?: ValidationState;
    onChange?: ActionRef;
  }

  export interface TextAreaNode extends BaseNode {
    type: "TextArea";
    value: string;
    label?: string;
    placeholder?: string;
    required?: boolean;
    minRows?: number;
    validation?: ValidationState;
    onChange?: ActionRef;
  }

  export interface SelectOption {
    value: string;
    label: string;
    disabled?: boolean;
  }

  export interface SelectNode extends BaseNode {
    type: "Select";
    value?: string;
    label?: string;
    placeholder?: string;
    required?: boolean;
    options: SelectOption[];
    validation?: ValidationState;
    onChange?: ActionRef;
  }

  export interface MultiSelectNode extends BaseNode {
    type: "MultiSelect";
    values: string[];
    label?: string;
    placeholder?: string;
    required?: boolean;
    options: SelectOption[];
    validation?: ValidationState;
    onChange?: ActionRef;
  }

  export interface CheckboxNode extends BaseNode {
    type: "Checkbox";
    checked: boolean;
    label?: string;
    onChange?: ActionRef;
  }

  export interface SwitchNode extends BaseNode {
    type: "Switch";
    checked: boolean;
    label?: string;
    onChange?: ActionRef;
  }

  export interface SliderNode extends BaseNode {
    type: "Slider";
    value: number;
    label?: string;
    min: number;
    max: number;
    step?: number;
    onChange?: ActionRef;
  }

  export interface DatePickerNode extends BaseNode {
    type: "DatePicker";
    value?: string;
    label?: string;
    min?: string;
    max?: string;
    required?: boolean;
    validation?: ValidationState;
    onChange?: ActionRef;
  }

  export interface ListNode extends BaseNode {
    type: "List";
    items: RenderableNode[];
    virtualized?: boolean;
    emptyState?: RenderableNode;
  }

  export interface TableColumn {
    id: string;
    label: string;
    field?: string;
    sortable?: boolean;
    align?: "start" | "end" | "center";
  }

  export interface TableNode extends BaseNode {
    type: "Table";
    columns: TableColumn[];
    rows: { id: string; cells: { [columnId: string]: JsonValue } }[];
    sort?: { columnId: string; direction: "asc" | "desc" };
    selection?: "none" | "single" | "multiple";
    selectedRowIds?: string[];
    onSort?: ActionRef;
    onSelect?: ActionRef;
  }

  export interface ChartSeries {
    id: string;
    label?: string;
    values: number[];
    intent?: Intent;
  }

  export interface ChartNode extends BaseNode {
    type: "Chart";
    chart: "line" | "bar" | "pie" | "scatter";
    labels?: string[];
    series: ChartSeries[];
    summary: string;
  }

  export interface StatNode extends BaseNode {
    type: "Stat";
    label: string;
    value: string | number;
    delta?: { value: string | number; intent?: Intent };
    intent?: Intent;
  }

  export interface TabsNode extends BaseNode {
    type: "Tabs";
    active: string;
    tabs: { id: string; label: string; child: RenderableNode; disabled?: boolean }[];
    onChange?: ActionRef;
  }

  export interface ModalNode extends BaseNode {
    type: "Modal";
    title: string;
    open: boolean;
    child: RenderableNode;
    onClose?: ActionRef;
  }

  export interface FormNode extends BaseNode {
    type: "Form";
    children: RenderableNode[];
    submitLabel?: string;
    validation?: ValidationState;
    onSubmit?: ActionRef;
  }

  export interface ValidationState {
    status: "none" | "valid" | "warning" | "error";
    message?: string;
  }
}
