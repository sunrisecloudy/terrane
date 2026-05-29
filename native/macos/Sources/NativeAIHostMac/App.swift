import AppKit
import SQLite3

final class AppDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow?
    private var hostView: WebHostView?
#if DEBUG
    private var controlPlane: DevControlPlane?
#endif

    func applicationDidFinishLaunching(_ notification: Notification) {
        if NativeProductionGuard.rejectDevOnlyFlagsIfNeeded() {
            NSApp.terminate(nil)
            return
        }
#if DEBUG
        if MacSmokeProbe.emitLaunchMarkerAndExitIfRequested() {
            return
        }
        do {
            if let controlPlane = try DevControlPlane.enabledFromProcess() {
                try controlPlane.start()
                self.controlPlane = controlPlane
                if let port = controlPlane.boundPort {
                    print("NATIVE_AI_MACOS_CONTROL_READY port=\(port)")
                }
            }
        } catch {
            fputs("macOS dev control plane failed to start: \(error)\n", stderr)
        }
#endif

        let hostView = WebHostView()
        self.hostView = hostView

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1200, height: 820),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Native AI Webapp Platform"
        window.center()
        window.contentView = hostView
        window.makeKeyAndOrderFront(nil)
        self.window = window
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }
}

enum NativeProductionGuard {
    private static let devOnlyFlags = [
        "--control-plane-port",
        "--allow-runtime-mismatch",
        "--allow-unsigned-dev",
    ]

    static func rejectDevOnlyFlagsIfNeeded(
        arguments: [String] = CommandLine.arguments,
        allowDevFlags: Bool = debugBuildAllowsDevFlags,
        databaseURL: URL? = nil
    ) -> Bool {
        guard let flag = rejectedDevOnlyFlag(in: arguments, allowDevFlags: allowDevFlags) else {
            return false
        }
        recordRejectedFlagAudit(flag: flag, databaseURL: databaseURL)
        fputs("fatal: production build rejects dev-only flag \(flag)\n", stderr)
        return true
    }

    static func rejectedDevOnlyFlag(in arguments: [String], allowDevFlags: Bool = debugBuildAllowsDevFlags) -> String? {
        guard !allowDevFlags else {
            return nil
        }
        return arguments.first { argument in
            devOnlyFlags.contains { flag in
                argument == flag || argument.hasPrefix("\(flag)=")
            }
        }
    }

    private static var debugBuildAllowsDevFlags: Bool {
#if DEBUG
        true
#else
        false
#endif
    }

    private static func recordRejectedFlagAudit(flag: String, databaseURL: URL?) {
        let database = PlatformDatabase(databaseURL: databaseURL)
        guard let db = database.handle else {
            return
        }
        let sessionId = "control_session_\(UUID().uuidString.lowercased())"
        let createdAt = now()
        insertControlSession(db: db, sessionId: sessionId, flag: flag, createdAt: createdAt)
        insertRejectedCommand(db: db, sessionId: sessionId, flag: flag, createdAt: createdAt)
    }

    private static func insertControlSession(
        db: OpaquePointer?,
        sessionId: String,
        flag: String,
        createdAt: String
    ) {
        var statement: OpaquePointer?
        let sql = """
        INSERT INTO control_sessions (control_session_id, target, actor, started_at, ended_at, status, metadata_json)
        VALUES (?, ?, ?, ?, ?, 'failed', ?)
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, sessionId)
        bind(statement, 2, "macos-production-guard")
        bind(statement, 3, "native-host")
        bind(statement, 4, createdAt)
        bind(statement, 5, createdAt)
        bind(statement, 6, jsonBody([
            "reason": "dev_only_flag",
            "flag": flag,
        ]))
        sqlite3_step(statement)
    }

    private static func insertRejectedCommand(
        db: OpaquePointer?,
        sessionId: String,
        flag: String,
        createdAt: String
    ) {
        var statement: OpaquePointer?
        let sql = """
        INSERT INTO control_commands (command_id, control_session_id, tool, http_method, path, decision, error_code, args_json, result_json, error_json, created_at, duration_ms)
        VALUES (?, ?, ?, NULL, NULL, 'rejected', ?, ?, NULL, ?, ?, 0)
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, "command_\(UUID().uuidString.lowercased())")
        bind(statement, 2, sessionId)
        bind(statement, 3, "native.production_guard")
        bind(statement, 4, "dev_only_flag")
        bind(statement, 5, jsonBody(["flag": flag]))
        bind(statement, 6, jsonBody([
            "code": "dev_only_flag",
            "message": "Production build rejects dev-only flag",
            "details": ["flag": flag],
        ]))
        bind(statement, 7, createdAt)
        sqlite3_step(statement)
    }

    private static func bind(_ statement: OpaquePointer?, _ index: Int32, _ value: String) {
        sqlite3_bind_text(statement, index, value, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self))
    }

    private static func jsonBody(_ object: [String: Any]) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]),
              let text = String(data: data, encoding: .utf8)
        else {
            return "{}"
        }
        return text
    }

    private static func now() -> String {
        ISO8601DateFormatter().string(from: Date())
    }
}

#if DEBUG
enum MacSmokeProbe {
    static let launchedMarker = "NATIVE_AI_MACOS_SMOKE_APP_LAUNCHED"
    static let markerFileName = "native-ai-macos-smoke-launched.txt"

    static func emitLaunchMarkerAndExitIfRequested() -> Bool {
        let args = CommandLine.arguments
        guard args.contains("--native-ai-smoke-launch") else { return false }
        print(launchedMarker)
        fflush(stdout)
        let markerURL = smokeMarkerURL()
        try? FileManager.default.createDirectory(
            at: markerURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try? launchedMarker.write(to: markerURL, atomically: true, encoding: .utf8)
        if args.contains("--native-ai-smoke-exit-after-launch") {
            DispatchQueue.main.async {
                NSApp.terminate(nil)
            }
            return true
        }
        return false
    }

    private static func smokeMarkerURL() -> URL {
        if let overridePath = ProcessInfo.processInfo.environment["NATIVE_AI_MACOS_SMOKE_MARKER_PATH"],
           !overridePath.isEmpty {
            return URL(fileURLWithPath: overridePath)
        }
        return URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent(markerFileName)
    }
}
#endif
