/**
 * A minimal, dependency-free, deterministic DOM shim sufficient for the
 * reference renderer and its conformance tests.
 *
 * Why a hand-rolled DOM instead of jsdom: the conformance suite only needs a
 * tree of elements with attributes/text/children plus a *stable* serialization
 * to diff render output against goldens. A tiny own-DOM keeps dependencies at
 * zero (no network install), keeps serialization byte-stable across Node
 * versions, and makes attribute ordering explicit (insertion order) so golden
 * comparisons are reproducible. The surface intentionally mirrors the subset of
 * the W3C DOM the renderer uses (`createElement`, `setAttribute`, `dataset`,
 * `textContent`, `appendChild`, `insertBefore`, `removeChild`,
 * `replaceChild`), so the renderer code reads like real DOM code.
 */

/** A DOM-like node: either an element or a text node. */
export type DomNode = DomElement | DomText;

/** A character-data text node. */
export class DomText {
  readonly nodeType = 3 as const;
  parent: DomElement | null = null;
  data: string;
  constructor(data: string) {
    this.data = data;
  }

  /** Mirror `Node.textContent`. */
  get textContent(): string {
    return this.data;
  }
  set textContent(value: string) {
    this.data = value;
  }
}

/**
 * A minimal element. Attributes preserve insertion order (a `Map`), which makes
 * serialization deterministic and golden-stable. `dataset` is a thin proxy over
 * `data-*` attributes mirroring the real `HTMLElement.dataset`.
 */
export class DomElement {
  readonly nodeType = 1 as const;
  readonly tagName: string;
  parent: DomElement | null = null;
  readonly children: DomNode[] = [];
  private readonly attrs = new Map<string, string>();

  /** `data-*` attribute accessor mirroring `HTMLElement.dataset`. */
  readonly dataset: Record<string, string>;

  constructor(tagName: string) {
    this.tagName = tagName.toLowerCase();
    const attrs = this.attrs;
    this.dataset = new Proxy(
      {},
      {
        get(_t, prop: string) {
          return attrs.get(`data-${camelToKebab(prop)}`);
        },
        set(_t, prop: string, value: string) {
          attrs.set(`data-${camelToKebab(prop)}`, String(value));
          return true;
        },
        has(_t, prop: string) {
          return attrs.has(`data-${camelToKebab(prop)}`);
        },
        deleteProperty(_t, prop: string) {
          return attrs.delete(`data-${camelToKebab(prop)}`);
        },
        ownKeys() {
          const keys: string[] = [];
          for (const k of attrs.keys()) {
            if (k.startsWith("data-")) keys.push(kebabToCamel(k.slice("data-".length)));
          }
          return keys;
        },
        getOwnPropertyDescriptor() {
          return { enumerable: true, configurable: true };
        },
      },
    ) as Record<string, string>;
  }

  setAttribute(name: string, value: string): void {
    this.attrs.set(name, value);
  }

  getAttribute(name: string): string | null {
    return this.attrs.has(name) ? (this.attrs.get(name) as string) : null;
  }

  hasAttribute(name: string): boolean {
    return this.attrs.has(name);
  }

  removeAttribute(name: string): void {
    this.attrs.delete(name);
  }

  /** Attribute entries in insertion order. */
  attributes(): [string, string][] {
    return [...this.attrs.entries()];
  }

  appendChild<T extends DomNode>(child: T): T {
    detach(child);
    child.parent = this;
    this.children.push(child);
    return child;
  }

  insertBefore<T extends DomNode>(child: T, ref: DomNode | null): T {
    detach(child);
    child.parent = this;
    if (ref === null) {
      this.children.push(child);
      return child;
    }
    const idx = this.children.indexOf(ref);
    if (idx < 0) throw new Error("insertBefore: reference node is not a child");
    this.children.splice(idx, 0, child);
    return child;
  }

  removeChild<T extends DomNode>(child: T): T {
    const idx = this.children.indexOf(child);
    if (idx < 0) throw new Error("removeChild: node is not a child");
    this.children.splice(idx, 1);
    child.parent = null;
    return child;
  }

  replaceChild(next: DomNode, prev: DomNode): DomNode {
    const idx = this.children.indexOf(prev);
    if (idx < 0) throw new Error("replaceChild: old node is not a child");
    detach(next);
    next.parent = this;
    this.children[idx] = next;
    prev.parent = null;
    return prev;
  }

  /** Mirror `Element.textContent`: concatenated descendant text on read; a
   * single text node replacing all children on write. */
  get textContent(): string {
    let out = "";
    for (const c of this.children) out += c.textContent;
    return out;
  }
  set textContent(value: string) {
    for (const c of this.children) c.parent = null;
    this.children.length = 0;
    if (value !== "") this.appendChild(new DomText(value));
  }

  /** Only the element children, in order (skips text nodes). */
  get childElements(): DomElement[] {
    return this.children.filter((c): c is DomElement => c.nodeType === 1);
  }
}

/** A document factory for elements/text nodes (mirrors the bits we use). */
export class DomDocument {
  createElement(tagName: string): DomElement {
    return new DomElement(tagName);
  }
  createTextNode(data: string): DomText {
    return new DomText(data);
  }
}

function detach(node: DomNode): void {
  if (node.parent) node.parent.removeChild(node);
}

function camelToKebab(s: string): string {
  return s.replace(/[A-Z]/g, (m) => `-${m.toLowerCase()}`);
}

function kebabToCamel(s: string): string {
  return s.replace(/-([a-z])/g, (_m, c: string) => c.toUpperCase());
}

const VOID_TAGS = new Set(["input", "br", "hr", "img"]);

function escapeText(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function escapeAttr(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
}

/**
 * Serialize a DOM node to deterministic HTML (attributes in insertion order,
 * no whitespace added). Used by the conformance suite as the stable rendering
 * fingerprint compared against goldens.
 */
export function serialize(node: DomNode): string {
  if (node.nodeType === 3) return escapeText(node.data);
  const el = node;
  let attrs = "";
  for (const [k, v] of el.attributes()) attrs += ` ${k}="${escapeAttr(v)}"`;
  if (VOID_TAGS.has(el.tagName)) return `<${el.tagName}${attrs}>`;
  let inner = "";
  for (const c of el.children) inner += serialize(c);
  return `<${el.tagName}${attrs}>${inner}</${el.tagName}>`;
}
