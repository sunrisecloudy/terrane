import AppKit

@MainActor
final class PlatformDialogs {
    private let openFileURLProvider: (() -> URL?)?
    private let saveFileURLProvider: ((String) -> URL?)?

    init(
        openFileURLProvider: (() -> URL?)? = nil,
        saveFileURLProvider: ((String) -> URL?)? = nil
    ) {
        self.openFileURLProvider = openFileURLProvider
        self.saveFileURLProvider = saveFileURLProvider
    }

    func openFile(_ request: BridgeRequest) -> BridgeResponse {
        guard let url = selectedOpenFileURL() else {
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
        let suggestedName = request.params["suggestedName"] as? String ?? "output.txt"
        guard let url = selectedSaveFileURL(suggestedName: suggestedName) else {
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

    private func selectedOpenFileURL() -> URL? {
        if let openFileURLProvider {
            return openFileURLProvider()
        }
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        let response = panel.runModal()
        guard response == .OK else {
            return nil
        }
        return panel.url
    }

    private func selectedSaveFileURL(suggestedName: String) -> URL? {
        if let saveFileURLProvider {
            return saveFileURLProvider(suggestedName)
        }
        let panel = NSSavePanel()
        panel.nameFieldStringValue = suggestedName
        let response = panel.runModal()
        guard response == .OK else {
            return nil
        }
        return panel.url
    }
}
