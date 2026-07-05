# Capability: `native` v2 — the next operation set

An **extension of the existing `terrane-cap-native` crate**, not a new
namespace. v1 shipped the machinery — operations catalog
(common/desktop/mobile groups), app-callable request commands committed as
`native.requested`, trusted-host `native.complete`/`fail`/`cancel`, the
`native.supports` query, keep-last-100 terminal retention. v2 adds six
operations to that registry; no new event kinds, no lifecycle changes. This
doc is the operations table plus per-host notes and grant wording.

Camera/microphone capture is **not** here — it's `cap-capture.md` (also
native-cap operations, split out because its blob plumbing deserves its own
spec). `screen.capture` *is* here: the desktop catalog already stubs it, and
`cap-capture.md` cross-references this as its home.

## Operations table

| Operation id | Group | Input (`input_json`) | Completion `result_json` | Result size | Hosts |
| --- | --- | --- | --- | --- | --- |
| `clipboard.readText` | common | `{}` | `{ "text": … }` (≤ 256 KiB, truncated flag if clipped) | inline-small | mac, web (async `navigator.clipboard.readText`), CLI (`pbpaste`) |
| `dialog.saveFile` | common | `{ "suggestedName", "blobName" }` — bytes come from the CAS, never the event | `{ "saved": bool, "path"?: str }` | inline-small | mac (NSSavePanel), web (download via `blobUrl`), CLI n/a |
| `screen.capture` | desktop | `{ "target": "screen"\|"window"? }` | `{ "hash", "size", "mime": "image/png", "width", "height", "blobName": "__capture__/<request_id>" }` | blob-ref | mac (ScreenCaptureKit + TCC screen-recording), web (`getDisplayMedia` + picker) |
| `tray.setMenu` | desktop | `{ "title", "items": [{ "id", "label" }] }` (≤ 20 items) | `{ "installed": true }` | inline-small | mac app; web shell n/a |
| `shortcut.registerGlobal` | desktop | `{ "accelerator": "cmd+shift+K", "verb" }` | `{ "registered": true }` or fail `{ "code": "conflict" }` | inline-small | mac app; web shell n/a |
| `window.control` | desktop | `{ "action": "focus"\|"minimize"\|"setTitle", "title"? }` — **the app's own shell window only** | `{ "ok": true }` | inline-small | mac shell, web shell (setTitle/focus best-effort) |

Safety/policy per catalog conventions: `clipboard.readText` and
`screen.capture` are `sensitive`; the rest `safe-request`/`user-mediated`
(dialog.saveFile is user-mediated by the panel itself). All grant-gated. Each
op gets the usual command/resource constants, `input_json` validation arm,
and `result_size_for_operation` arm — mechanical extensions of
`operations/common.rs` / `desktop.rs` and `commands.rs`.

`screen.capture` follows the `cap-capture.md` bytes-to-CAS rule exactly: the
executing host writes PNG bytes to the blob CAS, completes with the hash, and
emits `blob.stored` for `__capture__/<request_id>` (**depends on
`cap-blob.md`**). Replay folds metadata; no screen is re-captured.

## Durable registrations: tray + global shortcuts

`tray.setMenu` and `shortcut.registerGlobal` are not fire-and-forget — they
install *standing* host chrome. Model: the request completes immediately once
installed (`{installed: true}`); the **registration itself lives in folded
state** (`NativeState` gains `app → TrayMenu` / `app → shortcuts` from the
completion fold), and hosts re-install all live registrations from state on
startup — replay-safe by construction. A menu click or hotkey press then
dispatches the app's declared **verb** through the ordinary `host.run` path,
recording only ordinary app/kv events (Option A) — no new event kind for "user
clicked the tray". Re-issuing `tray.setMenu` replaces the menu; an empty
`items` array removes it. `app.removed` (already subscribed) drops
registrations and the host tears down the chrome.

## Grant wording (permission-prompt honesty)

- `clipboard.readText` — **reading the clipboard is spying-adjacent** (it
  holds passwords and whatever the user copied last). It must not ride the
  generic write-side grant: it becomes the first operation-level grant
  selector (`native:clipboard.readText`), prompt: *"<app> wants to READ your
  clipboard contents"*. The existing catalog policy vocabulary
  (`refuse-until-selector`) already anticipates this — v2 builds the selector.
- `screen.capture` — same selector treatment: *"<app> wants to capture your
  screen"*; macOS TCC screen-recording consent stacks on top.
- Remaining four ride the `native` namespace grant with the description
  updated to enumerate them (save dialogs, tray menu, global shortcut, own
  window control) so approval is informed.

## Replay story

Unchanged from v1: `native.requested` + terminal events fold; no OS surface is
touched on replay. New wrinkles: `screen.capture` bytes live in the CAS
(hash-verified, `cap-blob.md` contract); tray/shortcut registrations rebuild
from folded state, and re-installation is a host-startup edge action, not a
replay action.

## Limits

- Clipboard read ≤ 256 KiB text (larger → truncated + flag; images are a
  non-goal until `cap-media.md` gives them a shape).
- Tray ≤ 20 items, labels ≤ 64 chars; ≤ 5 global shortcuts per app.
- `window.control` strictly scoped to the requesting app's own window — a
  cross-app `title` arg is a typed error, not a lookup.

## Implementation plan

1. **Catalog + commands:** six entries across `operations/common.rs` /
   `desktop.rs` (promote the existing `screen.capture` and `tray.setMenu`
   stubs from `planned` to `v1`), constants, validation arms, result-size
   arms, `doc.rs` summaries.
2. **Grant selectors:** operation-level selector support in the auth grant
   spec for `clipboard.readText` + `screen.capture`; prompt wording in the
   shells' elicitation UI.
3. **State:** tray/shortcut registration maps in `NativeState` (folded from
   completions), replay-identity covered.
4. **Mac host:** pump arms — pasteboard read, NSSavePanel, ScreenCaptureKit →
   CAS (depends on `cap-blob.md` step 3), NSStatusItem + menu → verb dispatch,
   hotkey registration, window actions; startup re-install pass.
5. **Web shell:** clipboard read, `blobUrl` download for saveFile,
   `getDisplayMedia` → CAS, `document.title`/focus for window.control;
   tray/shortcut stay out of the web host's `platform.observe` list.
6. **Docs + tests:** `APP_API.md` op table; engine tests extend
   `terrane-core/tests/cap/native.rs` (validation, registration fold/replace/
   removal, replay identity); e2e extends `terrane-host/tests/cap/native.rs`
   with stub-executor lifecycles default-run and real-OS cases
   `#[ignore = "drives real macOS chrome/TCC"]`.

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`

## Non-goals (v2)

Clipboard images/files, clipboard *watching* (change notifications), arbitrary
window management of other apps' windows (that's `cap-applescript.md`
territory on the Mac, with its scarier grant), dock badges, mobile-group ops
(share sheet, haptics — the group exists; ops come with a mobile host),
`shell.openPath` (stays `refuse-until-selector` until path policy exists).

## Decisions to confirm

- **Verb dispatch identity for tray/shortcut** — *recommendation:* dispatch as
  the host on the app's behalf through the normal `host.run` gate, so the
  app's own grants still bound what the verb can do; *alternative:* a
  dedicated `native.activated` event apps subscribe to (new event kind,
  pub-sub machinery — heavier).
- **`dialog.saveFile` recording the chosen path** — *recommendation:* record
  it (`path` in result_json; it's bounded and auditable); *alternative:*
  record only `{saved: true}` — less filesystem-layout leakage into the log.
- **Selector mechanism scope** — *recommendation:* build the minimal
  operation-level selector now for the two sensitive ops (it also unblocks
  `cap-capture.md`'s camera/mic wish); *alternative:* defer selectors and gate
  both ops behind their own namespace-v1 grant resources as a stopgap.
