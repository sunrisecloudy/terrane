import Foundation
import SQLite3
import UIKit
import UniformTypeIdentifiers

@MainActor
final class PlatformDialogs: NSObject, UIDocumentPickerDelegate {
    var presenterProvider: (@MainActor () -> UIViewController?)?
    var databaseHandle: OpaquePointer?

    private enum Pending {
        case open(request: BridgeRequest, reply: BridgeReply)
        case save(request: BridgeRequest, reply: BridgeReply, tempURL: URL)
    }

    private var pending: Pending?

    func openFile(_ request: BridgeRequest, reply: @escaping BridgeReply) {
        if let mock = storedDialogMock(request: request, dialogType: "openFile") {
            reply(.success(id: request.id, result: mock))
            return
        }
        guard pending == nil else {
            reply(.failure(id: request.id, code: "capability_unavailable", message: "Another file dialog is already open"))
            return
        }
        guard let presenter = presenterProvider?() else {
            reply(.failure(id: request.id, code: "platform_unsupported", message: "dialog.openFile requires a presenting view controller"))
            return
        }

        let picker = UIDocumentPickerViewController(forOpeningContentTypes: acceptedTypes(request), asCopy: true)
        picker.allowsMultipleSelection = request.params["multiple"] as? Bool ?? false
        picker.delegate = self
        pending = .open(request: request, reply: reply)
        presenter.present(picker, animated: true)
    }

    func saveFile(_ request: BridgeRequest, reply: @escaping BridgeReply) {
        if let mock = storedDialogMock(request: request, dialogType: "saveFile") {
            reply(.success(id: request.id, result: mock))
            return
        }
        guard pending == nil else {
            reply(.failure(id: request.id, code: "capability_unavailable", message: "Another file dialog is already open"))
            return
        }
        guard let presenter = presenterProvider?() else {
            reply(.failure(id: request.id, code: "platform_unsupported", message: "dialog.saveFile requires a presenting view controller"))
            return
        }

        do {
            let tempURL = try writeTemporaryExportFile(request)
            let picker = UIDocumentPickerViewController(forExporting: [tempURL], asCopy: true)
            picker.delegate = self
            pending = .save(request: request, reply: reply, tempURL: tempURL)
            presenter.present(picker, animated: true)
        } catch {
            reply(.failure(id: request.id, code: "storage_error", message: error.localizedDescription))
        }
    }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        guard let pending else { return }
        self.pending = nil

        switch pending {
        case let .open(request, reply):
            reply(openResult(request, urls: urls))
        case let .save(request, reply, tempURL):
            removeTemporaryFile(tempURL)
            reply(.success(id: request.id, result: ["ok": true]))
        }
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        guard let pending else { return }
        self.pending = nil

        switch pending {
        case let .open(request, reply):
            reply(.failure(id: request.id, code: "dialog_cancelled", message: "Open file was cancelled"))
        case let .save(request, reply, tempURL):
            removeTemporaryFile(tempURL)
            reply(.failure(id: request.id, code: "dialog_cancelled", message: "Save file was cancelled"))
        }
    }

    private func openResult(_ request: BridgeRequest, urls: [URL]) -> BridgeResponse {
        let limit = maxBytes(request)
        var files: [[String: Any]] = []

        for url in urls {
            let didAccess = url.startAccessingSecurityScopedResource()
            defer {
                if didAccess {
                    url.stopAccessingSecurityScopedResource()
                }
            }

            do {
                let data = try Data(contentsOf: url)
                guard data.count <= limit else {
                    return .failure(id: request.id, code: "quota_exceeded", message: "Selected file exceeds maxBytes")
                }
                let text = String(data: data, encoding: .utf8) ?? ""
                files.append([
                    "name": url.lastPathComponent,
                    "mime": mime(for: url, request: request),
                    "size": data.count,
                    "text": text
                ])
            } catch {
                return .failure(id: request.id, code: "storage_error", message: error.localizedDescription)
            }
        }

        return .success(id: request.id, result: ["files": files])
    }

    private func acceptedTypes(_ request: BridgeRequest) -> [UTType] {
        guard let accept = request.params["accept"] as? [String] else {
            return [.plainText]
        }
        let types = accept.compactMap { UTType(mimeType: $0) }
        return types.isEmpty ? [.plainText] : types
    }

    private func maxBytes(_ request: BridgeRequest) -> Int {
        guard let value = request.params["maxBytes"] as? NSNumber else {
            return 1_048_576
        }
        return max(0, value.intValue)
    }

    private func mime(for url: URL, request: BridgeRequest) -> String {
        if let preferred = UTType(filenameExtension: url.pathExtension)?.preferredMIMEType {
            return preferred
        }
        if let accept = request.params["accept"] as? [String], let first = accept.first, !first.isEmpty {
            return first
        }
        return "text/plain"
    }

    private func writeTemporaryExportFile(_ request: BridgeRequest) throws -> URL {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("terrane-dialogs", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let suggestedName = (request.params["suggestedName"] as? String ?? "output.txt")
            .split(separator: "/")
            .last
            .map(String.init) ?? "output.txt"
        let url = directory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
            .appendingPathComponent(suggestedName)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        let text = request.params["text"] as? String ?? ""
        try text.write(to: url, atomically: true, encoding: .utf8)
        return url
    }

    private func removeTemporaryFile(_ url: URL) {
        try? FileManager.default.removeItem(at: url.deletingLastPathComponent())
    }

    private func storedDialogMock(request: BridgeRequest, dialogType: String) -> Any? {
        guard let db = databaseHandle,
              !request.context.appId.isEmpty
        else { return nil }
        let sql = """
        SELECT response_json FROM dialog_mocks
        WHERE enabled = 1 AND dialog_type = ? AND (app_id IS NULL OR app_id = ?) AND (session_id IS NULL OR session_id = ?)
        ORDER BY created_at DESC LIMIT 1
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return nil }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, dialogType)
        bind(statement, 2, request.context.appId)
        bind(statement, 3, runtimeSessionId(request))
        guard sqlite3_step(statement) == SQLITE_ROW else { return nil }
        return jsonValue(columnText(statement, 0))
    }

    private func runtimeSessionId(_ request: BridgeRequest) -> String {
        "runtime_ios_\(request.context.appId)_\(request.context.mountToken ?? "native")"
    }

    private func jsonValue(_ text: String) -> Any? {
        guard let data = text.data(using: .utf8) else { return nil }
        return try? JSONSerialization.jsonObject(with: data)
    }

    private func bind(_ statement: OpaquePointer?, _ index: Int32, _ value: String) {
        sqlite3_bind_text(statement, index, value, -1, SQLITE_TRANSIENT_DIALOGS)
    }

    private func columnText(_ statement: OpaquePointer?, _ index: Int32) -> String {
        guard sqlite3_column_type(statement, index) != SQLITE_NULL,
              let text = sqlite3_column_text(statement, index)
        else { return "" }
        return String(cString: text)
    }
}

private let SQLITE_TRANSIENT_DIALOGS = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
