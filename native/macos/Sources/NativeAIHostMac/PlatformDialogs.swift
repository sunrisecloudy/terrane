import AppKit
import Foundation
import UniformTypeIdentifiers

@MainActor
final class PlatformDialogs {
    private enum OpenFileReadError: Error {
        case quotaExceeded
        case storage(Error)
    }

    private let openFileURLsProvider: ((Bool) -> [URL]?)?
    private let saveFileURLProvider: ((String) -> URL?)?

    init(
        openFileURLProvider: (() -> URL?)? = nil,
        openFileURLsProvider: ((Bool) -> [URL]?)? = nil,
        saveFileURLProvider: ((String) -> URL?)? = nil
    ) {
        if let openFileURLsProvider {
            self.openFileURLsProvider = openFileURLsProvider
        } else if let openFileURLProvider {
            self.openFileURLsProvider = { _ in
                openFileURLProvider().map { [$0] }
            }
        } else {
            self.openFileURLsProvider = nil
        }
        self.saveFileURLProvider = saveFileURLProvider
    }

    func openFile(_ request: BridgeRequest) -> BridgeResponse {
        guard let multiple = multipleSelection(request) else {
            return .failure(id: request.id, code: "invalid_request", message: "dialog.openFile multiple must be a boolean")
        }
        guard let accept = acceptValues(request) else {
            return .failure(id: request.id, code: "invalid_request", message: "dialog.openFile accept must be an array of strings")
        }
        guard let maxBytes = maxBytes(request) else {
            return .failure(id: request.id, code: "invalid_request", message: "dialog.openFile maxBytes must be a number")
        }

        let acceptedTypes = acceptedContentTypes(accept)
        guard let urls = selectedOpenFileURLs(multiple: multiple, acceptedTypes: acceptedTypes) else {
            return .failure(id: request.id, code: "dialog_cancelled", message: "Open file was cancelled")
        }
        guard !urls.isEmpty else {
            return .failure(id: request.id, code: "storage_error", message: "Open file results were empty")
        }

        var files: [[String: Any]] = []
        for url in urls {
            do {
                files.append(try selectedFileRecord(url: url, accept: accept, maxBytes: maxBytes))
            } catch OpenFileReadError.quotaExceeded {
                return .failure(id: request.id, code: "quota_exceeded", message: "Selected file exceeds maxBytes")
            } catch OpenFileReadError.storage(let error) {
                return .failure(id: request.id, code: "storage_error", message: error.localizedDescription)
            } catch {
                return .failure(id: request.id, code: "storage_error", message: error.localizedDescription)
            }
        }

        return .success(id: request.id, result: ["files": files])
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

    private func selectedOpenFileURLs(multiple: Bool, acceptedTypes: [UTType]) -> [URL]? {
        if let openFileURLsProvider {
            guard let urls = openFileURLsProvider(multiple) else {
                return nil
            }
            return multiple ? urls : Array(urls.prefix(1))
        }
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = multiple
        panel.canChooseDirectories = false
        panel.canChooseFiles = true
        if !acceptedTypes.isEmpty {
            panel.allowedContentTypes = acceptedTypes
        }
        let response = panel.runModal()
        guard response == .OK else {
            return nil
        }
        return multiple ? panel.urls : panel.url.map { [$0] } ?? []
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

    private func multipleSelection(_ request: BridgeRequest) -> Bool? {
        guard let value = request.params["multiple"] else {
            return false
        }
        return value as? Bool
    }

    private func acceptValues(_ request: BridgeRequest) -> [String]? {
        guard let value = request.params["accept"] else {
            return []
        }
        guard let accept = value as? [String] else {
            return nil
        }
        return accept
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
    }

    private func acceptedContentTypes(_ accept: [String]) -> [UTType] {
        accept.compactMap { acceptedContentType($0) }
    }

    private func acceptedContentType(_ value: String) -> UTType? {
        if value == "*/*" {
            return .data
        }
        if value == "text/*" {
            return .text
        }
        if value == "image/*" {
            return .image
        }
        if value == "audio/*" {
            return .audio
        }
        if value == "video/*" {
            return .movie
        }
        if value.hasPrefix(".") {
            return UTType(filenameExtension: String(value.dropFirst()))
        }
        return UTType(mimeType: value)
    }

    private func maxBytes(_ request: BridgeRequest) -> Int? {
        guard let value = request.params["maxBytes"] else {
            return 1_048_576
        }
        if value is Bool {
            return nil
        }
        if let intValue = value as? Int {
            return max(0, intValue)
        }
        if let doubleValue = value as? Double {
            return boundedMaxBytes(doubleValue)
        }
        if let numberValue = value as? NSNumber {
            return boundedMaxBytes(numberValue.doubleValue)
        }
        return nil
    }

    private func boundedMaxBytes(_ value: Double) -> Int? {
        guard value.isFinite else {
            return nil
        }
        if value <= 0 {
            return 0
        }
        if value >= Double(Int.max) {
            return Int.max
        }
        return Int(value)
    }

    private func selectedFileRecord(url: URL, accept: [String], maxBytes: Int) throws -> [String: Any] {
        let data = try readSelectedFileData(url: url, maxBytes: maxBytes)
        let text = String(data: data, encoding: .utf8) ?? ""
        return [
            "name": url.lastPathComponent,
            "mime": mime(for: url, accept: accept),
            "size": data.count,
            "text": text
        ]
    }

    private func readSelectedFileData(url: URL, maxBytes: Int) throws -> Data {
        do {
            let values = try url.resourceValues(forKeys: [.fileSizeKey])
            if let fileSize = values.fileSize, fileSize > maxBytes {
                throw OpenFileReadError.quotaExceeded
            }
            let data = try Data(contentsOf: url)
            guard data.count <= maxBytes else {
                throw OpenFileReadError.quotaExceeded
            }
            return data
        } catch let error as OpenFileReadError {
            throw error
        } catch {
            throw OpenFileReadError.storage(error)
        }
    }

    private func mime(for url: URL, accept: [String]) -> String {
        if let preferred = UTType(filenameExtension: url.pathExtension)?.preferredMIMEType {
            return preferred
        }
        if let first = accept.first, !first.hasSuffix("/*"), first != "*/*" {
            return first
        }
        return "text/plain"
    }
}
