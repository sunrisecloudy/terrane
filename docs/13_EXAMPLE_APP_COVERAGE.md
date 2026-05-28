# Example App API Coverage

| Example app | Main use case | APIs covered |
|---|---|---|
| Notes Lite | CRUD notes | `storage.get`, `storage.set`, `storage.remove`, `notification.toast`, `app.log` |
| Task Workbench | Task workflow | `core.step`, `storage.get`, `storage.set`, `notification.toast`, `app.log` |
| File Transformer | File import/export | `dialog.openFile`, `dialog.saveFile`, `core.step`, `storage.set`, `notification.toast` |
| API Dashboard | Network request dashboard | `network.request`, `storage.get`, `storage.set`, `notification.toast`, `app.log` |
| Core Replay Lab | Event replay/debug | `core.step`, `storage.get`, `storage.set`, `dialog.saveFile`, `notification.toast` |

Together these examples exercise every v0.1 bridge method.
