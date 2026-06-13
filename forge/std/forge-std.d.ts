declare module "@forge/std" {
  /**
   * JSON-compatible data used for M0a records and applet results.
   * PRD refs: CR-8, DL-15.
   */
  export type JsonValue =
    | null
    | boolean
    | number
    | string
    | JsonValue[]
    | { [key: string]: JsonValue };

  /**
   * M0a database record shape. Full schema-generated types arrive later with
   * the complete @forge/std package.
   * PRD refs: CR-3, DL-15, T002.
   */
  export type DbRecord = { [key: string]: JsonValue };

  /**
   * Result returned by a script/automation entrypoint.
   * PRD refs: CR-8, UI-11.
   */
  export type AppResult =
    | { ok: true; value?: JsonValue; ui?: Node }
    | { ok: false; error: string; details?: JsonValue };

  /**
   * M0a applet/script context. This is the only host surface exposed to
   * TypeScript code.
   * PRD refs: CR-3, CR-10.
   */
  export interface AppContext {
    db: Db;
    storage: Storage;
    ui: Ui;
    time: TimeApi;
    random: RandomApi;
  }

  /**
   * Script/automation entrypoint.
   * PRD refs: CR-8.
   */
  export type Main = (ctx: AppContext, input: unknown) => Promise<AppResult>;

  /**
   * M0a row-query plan subset accepted by `ctx.db.query` (DL-15). The runtime
   * supports both `query(plan)` and `query(collection, plan)`; in both forms the
   * trusted host capability-checks the collection being read.
   * PRD refs: CR-3, DL-15, DL-16, T023.
   */
  export type QueryOperator =
    | "="
    | "!="
    | "<"
    | "<="
    | ">"
    | ">="
    | "in"
    | "like"
    | "eq"
    | "ne"
    | "lt"
    | "le"
    | "gt"
    | "ge";
  export type QueryDirection = "asc" | "desc";
  export type QueryFieldRef = string | { field: string } | { field_id: string };
  export type QueryWhere =
    | [string, QueryOperator, JsonValue]
    | { field: string; op: QueryOperator; value?: JsonValue }
    | { field_id: string; op: QueryOperator; value?: JsonValue }
    | { and: QueryWhere[] }
    | { or: QueryWhere[] };
  export type QueryOrderBy =
    | [string, QueryDirection]
    | { field: string; dir?: QueryDirection }
    | { field_id: string; dir?: QueryDirection };
  export interface QueryPlan {
    from: string;
    where?: QueryWhere | QueryWhere[];
    orderBy?: QueryOrderBy | QueryOrderBy[];
    order_by?: QueryOrderBy | QueryOrderBy[];
    limit?: number;
    offset?: number;
    includeDeleted?: boolean;
    include_deleted?: boolean;
    includeDeprecated?: boolean;
    include_deprecated?: boolean;
  }

  /**
   * Minimal M0a relational surface plus the DL-15 structured query.
   * PRD refs: CR-3, DL-15, T023.
   */
  export interface Db {
    insert(collection: string, record: DbRecord): Promise<{ id: string }>;
    get(collection: string, id: string): Promise<DbRecord | null>;
    list(collection: string): Promise<DbRecord[]>;
    query(query: QueryPlan): Promise<DbRecord[]>;
    query(collection: string, query: QueryPlan): Promise<DbRecord[]>;
  }

  /**
   * Per-applet KV storage surface.
   * PRD refs: CR-3.
   */
  export interface Storage {
    get(key: string): Promise<string | null>;
    set(key: string, value: string): Promise<void>;
    delete(key: string): Promise<void>;
    list(prefix: string): Promise<string[]>;
  }

  /**
   * Deterministic time seam. In deterministic mode, values are recorded or
   * seeded by the runtime replay harness.
   * PRD refs: CR-8, CR-11.
   */
  export interface TimeApi {
    now(): number;
  }

  /**
   * Deterministic random seam. In deterministic mode, values are recorded or
   * seeded by the runtime replay harness.
   * PRD refs: CR-8, CR-11.
   */
  export interface RandomApi {
    next(): number;
  }

  /**
   * Declarative UI host surface.
   * PRD refs: CR-3, UI-1, UI-2.
   */
  export interface Ui {
    render(tree: Node): void;
  }

  export type Node = StackNode | TextNode | ButtonNode | TextFieldNode | ListNode;

  export interface BaseNode {
    id?: string;
    testId?: string;
  }

  export interface StackNode extends BaseNode {
    type: "Stack";
    direction?: "h" | "v";
    gap?: "none" | "xs" | "sm" | "md" | "lg";
    children: Node[];
  }

  export interface TextNode extends BaseNode {
    type: "Text";
    text: string;
    variant?: "body" | "caption" | "title" | "monospace";
  }

  export interface ButtonNode extends BaseNode {
    type: "Button";
    label: string;
    variant?: "primary" | "secondary" | "destructive";
    onTap?: ActionRef;
  }

  export interface TextFieldNode extends BaseNode {
    type: "TextField";
    value: string;
    label?: string;
    placeholder?: string;
    onChange?: ActionRef;
  }

  export interface ListNode extends BaseNode {
    type: "List";
    items: Node[];
  }

  /**
   * M0a uses serializable action refs in rendered trees. Renderers send the
   * referenced action back through the core event queue.
   * PRD refs: UI-4, UI-12.
   */
  export type ActionRef = string;

  export type UiEvent =
    | { type: "tap"; targetId: string; action: ActionRef }
    | { type: "change"; targetId: string; action: ActionRef; value: string };
}
