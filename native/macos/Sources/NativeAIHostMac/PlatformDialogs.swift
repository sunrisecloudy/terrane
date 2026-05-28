import AppKit

@MainActor
final class PlatformDialogs {
    func openFile(_ request: BridgeRequest) -> BridgeResponse {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        let response = panel.runModal()
        guard response == .OK, let url = panel.url else {
            return .failure(id: request.id, code: "dialog_cancelled", message: "Open file was cancelled")
        }
        let text = (try? String(contentsOf: url, encoding: .utf8)) ?? ""
        return .success(id: request.id, result: [
            "files": [[
                "name": url.lastPathComponent,
                "mime": "text/plain",
                "size": text.utf8.count,
                "text": text
            ]]
        ])
    }

    func saveFile(_ request: BridgeRequest) -> BridgeResponse {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = request.params["suggestedName"] as? String ?? "output.txt"
        let response = panel.runModal()
        guard response == .OK, let url = panel.url else {
            return .failure(id: request.id, code: "dialog_cancelled", message: "Save file was cancelled")
        }
        let text = request.params["text"] as? String ?? ""
        do {
            try text.write(to: url, atomically: true, encoding: .utf8)
            return .success(id: request.id, result: ["ok": true])
        } catch {
            return .failure(id: request.id, code: "storage_error", message: error.localizedDescription)
        }
    }
}
