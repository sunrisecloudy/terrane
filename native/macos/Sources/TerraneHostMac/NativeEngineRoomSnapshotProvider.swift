import Foundation
import SQLite3

final class NativeEngineRoomSnapshotProvider {
    private let database: PlatformDatabase
    private let databaseURL: URL
    private let catalog: MacAppCatalog

    init(databaseURL: URL? = nil, catalog: MacAppCatalog = MacAppCatalog()) {
        let resolvedURL = databaseURL ?? Self.defaultDatabaseURL()
        self.databaseURL = resolvedURL
        self.database = PlatformDatabase(databaseURL: resolvedURL)
        self.catalog = catalog
    }

    func snapshot(appId: String? = nil, limit: Int? = nil) -> [String: Any] {
        let rowLimit = max(1, min(limit ?? 50, 500))
        let bridgeRows = tableRows(
            table: "bridge_calls",
            columns: ["bridge_call_id", "session_id", "app_id", "install_id", "method", "params_json", "result_json", "error_json", "duration_ms", "created_at"],
            orderBy: "created_at",
            filterColumn: "app_id",
            filterValue: appId,
            limit: rowLimit
        )
        let coreRows = tableRows(
            table: "core_events",
            columns: ["event_id", "session_id", "app_id", "install_id", "state_version_before", "event_json", "created_at"],
            orderBy: "created_at",
            filterColumn: "app_id",
            filterValue: appId,
            limit: rowLimit
        )
        let testRows = tableRows(
            table: "test_runs",
            columns: ["test_run_id", "micro_test_id", "session_id", "control_session_id", "app_id", "status", "started_at", "finished_at", "result_json", "diagnostics_json"],
            orderBy: "started_at",
            filterColumn: "app_id",
            filterValue: appId,
            limit: rowLimit
        )
        let runtimeSessions = tableRows(
            table: "runtime_sessions",
            columns: ["session_id", "target", "platform", "runtime_version", "active_app_id", "active_install_id", "started_at", "ended_at", "status", "capabilities_json", "resource_high_water_json", "metadata_json"],
            orderBy: "started_at",
            limit: rowLimit
        )
        let runtimeSnapshots = tableRows(
            table: "runtime_snapshots",
            columns: ["snapshot_id", "session_id", "app_id", "install_id", "type", "snapshot_json", "content_hash", "created_at"],
            orderBy: "created_at",
            filterColumn: "app_id",
            filterValue: appId,
            limit: rowLimit
        )
        let controlSessions = tableRows(
            table: "control_sessions",
            columns: ["control_session_id", "target", "runtime_session_id", "actor", "started_at", "ended_at", "status", "metadata_json"],
            orderBy: "started_at",
            limit: rowLimit
        )
        let controlCommands = tableRows(
            table: "control_commands",
            columns: ["command_id", "control_session_id", "runtime_session_id", "tool", "http_method", "path", "decision", "error_code", "args_json", "result_json", "error_json", "created_at", "duration_ms"],
            orderBy: "created_at",
            limit: rowLimit
        )
        let networkRows = bridgeRows.filter { ($0["method"] as? String) == "network.request" }
        let logRows = bridgeRows.filter { ($0["method"] as? String) == "app.log" }

        let catalog = ForgeDataCatalog.shared
        return [
            "generatedAt": Self.now(),
            "overview": [
                "source": catalog.engineRoomTables.source,
                "platform": catalog.runtimeConfig.platform,
                "target": catalog.runtimeConfig.target,
                "runtimeVersion": catalog.runtimeVersion,
                "devMode": true,
                "appId": appId.map { $0 as Any } ?? NSNull(),
                "activeAppId": latestActiveAppId() ?? NSNull(),
                "runtimeSession": latestRuntimeSession().map { $0 as Any } ?? NSNull(),
                "featureFlags": catalog.engineRoomTables.featureFlags,
                "capabilities": catalog.engineRoomTables.capabilities,
                "resourceLimits": resourceLimits(),
            ],
            "apps": [
                "rows": tableRows(
                    table: "apps",
                    columns: ["id", "name", "status", "active_install_id", "active_version", "data_version", "created_at", "updated_at"],
                    orderBy: "id",
                    filterColumn: "id",
                    filterValue: appId,
                    limit: rowLimit
                ),
                "versions": tableRows(
                    table: "app_versions",
                    columns: ["install_id", "app_id", "version", "runtime_version", "data_version", "manifest_json", "manifest_hash", "content_hash", "signature_json", "trust_level", "status", "created_at", "activated_at"],
                    orderBy: "created_at",
                    filterColumn: "app_id",
                    filterValue: appId,
                    limit: rowLimit
                ),
                "installed": bundledApps(appId: appId),
                "packageFiles": tableRows(
                    table: "app_files",
                    columns: ["install_id", "path", "content_hash", "size_bytes", "mime", "created_at"],
                    orderBy: "path",
                    limit: rowLimit
                ),
            ],
            "database": [
                "type": "sqlite",
                "path": databaseURL.path,
                "integrity": integrityStatus(),
                "tables": tableNames(),
                "tableCounts": tableCounts(),
            ],
            "storage": [
                "rows": tableRows(
                    table: "app_storage",
                    columns: ["app_id", "key", "value_json", "updated_at"],
                    orderBy: "updated_at",
                    filterColumn: "app_id",
                    filterValue: appId,
                    limit: rowLimit
                ),
            ],
            "bridgeCalls": ["rows": bridgeRows],
            "network": [
                "rows": networkRows,
                "mocks": tableRows(
                    table: "network_mocks",
                    columns: ["mock_id", "session_id", "app_id", "method", "url_pattern", "enabled", "created_at"],
                    orderBy: "created_at",
                    filterColumn: "app_id",
                    filterValue: appId,
                    limit: rowLimit
                ),
            ],
            "logs": [
                "appLogRows": logRows,
                "runtimeSessions": runtimeSessions,
                "telemetry": ["crashReporting": "not-configured"],
            ],
            "core": [
                "events": coreRows,
                "actions": tableRows(
                    table: "core_actions",
                    columns: ["action_id", "event_id", "session_id", "app_id", "action_json", "created_at"],
                    orderBy: "created_at",
                    filterColumn: "app_id",
                    filterValue: appId,
                    limit: rowLimit
                ),
                "snapshots": runtimeSnapshots,
            ],
            "permissions": [
                "rows": tableRows(
                    table: "app_permissions",
                    columns: ["install_id", "app_id", "permission", "requested", "approved", "approved_at", "reason"],
                    orderBy: "permission",
                    filterColumn: "app_id",
                    filterValue: appId,
                    limit: rowLimit
                ),
                "installReports": tableRows(
                    table: "app_install_reports",
                    columns: ["report_id", "app_id", "install_id", "status", "validation_json", "security_json", "permissions_json", "compatibility_json", "smoke_test_json", "content_hash", "created_at"],
                    orderBy: "created_at",
                    filterColumn: "app_id",
                    filterValue: appId,
                    limit: rowLimit
                ),
            ],
            "tests": [
                "runs": testRows,
                "controlSessions": controlSessions,
                "controlCommands": controlCommands,
            ],
            "crdt": [
                "notebooks": tableRows(table: "crdt_notebooks", columns: ["app_id", "notebook_id", "title", "status", "created_by", "created_at", "updated_at"], orderBy: "updated_at", filterColumn: "app_id", filterValue: appId, limit: rowLimit),
                "documents": tableRows(table: "crdt_documents", columns: ["app_id", "notebook_id", "version", "content_hash", "created_at"], orderBy: "created_at", filterColumn: "app_id", filterValue: appId, limit: rowLimit),
                "updates": tableRows(table: "crdt_updates", columns: ["update_id", "app_id", "notebook_id", "seq", "actor_id", "status", "error_code", "created_at"], orderBy: "created_at", filterColumn: "app_id", filterValue: appId, limit: rowLimit),
                "heads": tableRows(table: "crdt_heads", columns: ["app_id", "notebook_id", "version", "frontier_json", "content_hash", "updated_at"], orderBy: "updated_at", filterColumn: "app_id", filterValue: appId, limit: rowLimit),
                "actors": tableRows(table: "crdt_actors", columns: ["app_id", "actor_id", "actor_kind", "display_name", "created_at", "updated_at"], orderBy: "updated_at", filterColumn: "app_id", filterValue: appId, limit: rowLimit),
                "permissions": tableRows(table: "crdt_permissions", columns: ["app_id", "notebook_id", "actor_id", "permission", "granted", "granted_at"], orderBy: "granted_at", filterColumn: "app_id", filterValue: appId, limit: rowLimit),
                "proposals": tableRows(table: "crdt_proposals", columns: ["proposal_id", "app_id", "notebook_id", "actor_id", "status", "created_at", "updated_at"], orderBy: "created_at", filterColumn: "app_id", filterValue: appId, limit: rowLimit),
            ],
            "sync": [
                "cursors": tableRows(table: "crdt_sync_cursors", columns: ["app_id", "notebook_id", "actor_id", "last_seen_update_id", "frontier_json", "updated_at"], orderBy: "updated_at", filterColumn: "app_id", filterValue: appId, limit: rowLimit),
                "server": ["status": "not-attached"],
            ],
        ]
    }

    private static func defaultDatabaseURL() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        return base.appendingPathComponent("Terrane/platform.sqlite")
    }

    private func bundledApps(appId: String?) -> [[String: Any]] {
        let items = (try? catalog.loadBundledApps()) ?? []
        return items
            .filter { appId == nil || $0.id == appId }
            .map { item in
                [
                    "appId": item.id,
                    "name": item.name,
                    "version": item.version,
                    "description": item.description,
                    "contentRating": item.contentRatingLabel ?? NSNull(),
                    "manifest": bundledManifest(appId: item.id) ?? NSNull(),
                ] as [String: Any]
            }
    }

    private func bundledManifest(appId: String) -> Any? {
        guard let manifestURL = RuntimeResourceLocator.exampleManifestURL(for: appId),
              let data = try? Data(contentsOf: manifestURL)
        else {
            return nil
        }
        return try? JSONSerialization.jsonObject(with: data)
    }

    private func tableRows(
        table: String,
        columns: [String],
        orderBy: String,
        filterColumn: String? = nil,
        filterValue: String? = nil,
        limit: Int
    ) -> [[String: Any]] {
        guard let db = database.handle else { return [] }
        let filterSQL = filterColumn != nil && filterValue != nil ? " WHERE \(filterColumn!) = ?" : ""
        let sql = "SELECT \(columns.joined(separator: ", ")) FROM \(table)\(filterSQL) ORDER BY \(orderBy) DESC LIMIT ?"
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return []
        }
        defer { sqlite3_finalize(statement) }

        if let filterValue {
            sqlite3_bind_text(statement, 1, filterValue, -1, SQLITE_TRANSIENT_ENGINE_ROOM)
            sqlite3_bind_int(statement, 2, Int32(limit))
        } else {
            sqlite3_bind_int(statement, 1, Int32(limit))
        }

        var rows: [[String: Any]] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            var row: [String: Any] = [:]
            for (index, column) in columns.enumerated() {
                row[column] = Self.sqliteValue(statement, Int32(index))
            }
            rows.append(row)
        }
        return rows
    }

    private func tableNames() -> [String] {
        guard let db = database.handle else { return [] }
        var statement: OpaquePointer?
        let sql = "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return []
        }
        defer { sqlite3_finalize(statement) }

        var names: [String] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            names.append(Self.columnText(statement, 0))
        }
        return names
    }

    private func tableCounts() -> [String: Int] {
        var counts: [String: Int] = [:]
        for table in tableNames() {
            counts[table] = countRows(table: table)
        }
        return counts
    }

    private func countRows(table: String) -> Int {
        guard let db = database.handle else { return 0 }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, "SELECT COUNT(*) FROM \(table)", -1, &statement, nil) == SQLITE_OK else {
            return 0
        }
        defer { sqlite3_finalize(statement) }
        guard sqlite3_step(statement) == SQLITE_ROW else { return 0 }
        return Int(sqlite3_column_int64(statement, 0))
    }

    private func latestRuntimeSession() -> [String: Any]? {
        tableRows(
            table: "runtime_sessions",
            columns: ["session_id", "target", "platform", "runtime_version", "active_app_id", "active_install_id", "started_at", "ended_at", "status"],
            orderBy: "started_at",
            limit: 1
        ).first
    }

    private func latestActiveAppId() -> Any? {
        latestRuntimeSession()?["active_app_id"]
    }

    private func integrityStatus() -> String {
        guard let db = database.handle else { return "unavailable" }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, "PRAGMA integrity_check", -1, &statement, nil) == SQLITE_OK else {
            return "unavailable"
        }
        defer { sqlite3_finalize(statement) }
        guard sqlite3_step(statement) == SQLITE_ROW else { return "unavailable" }
        return Self.columnText(statement, 0)
    }

    private func resourceLimits() -> [String: Any] {
        let manifests = bundledApps(appId: nil).compactMap { $0["manifest"] as? [String: Any] }
        return [
            "bundledApps": manifests.compactMap { $0["resourceBudget"] as? [String: Any] },
            "note": "Per-app limits are read from manifest.resourceBudget.",
        ]
    }

    private static func now() -> String {
        ISO8601DateFormatter().string(from: Date())
    }

    private static func columnText(_ statement: OpaquePointer?, _ index: Int32) -> String {
        columnNullableText(statement, index) ?? ""
    }

    private static func columnNullableText(_ statement: OpaquePointer?, _ index: Int32) -> String? {
        guard sqlite3_column_type(statement, index) != SQLITE_NULL,
              let pointer = sqlite3_column_text(statement, index)
        else {
            return nil
        }
        return String(cString: pointer)
    }

    private static func sqliteValue(_ statement: OpaquePointer?, _ index: Int32) -> Any {
        switch sqlite3_column_type(statement, index) {
        case SQLITE_INTEGER:
            return Int(sqlite3_column_int64(statement, index))
        case SQLITE_FLOAT:
            return sqlite3_column_double(statement, index)
        case SQLITE_TEXT:
            return columnText(statement, index)
        case SQLITE_NULL:
            return NSNull()
        default:
            return columnText(statement, index)
        }
    }
}

private let SQLITE_TRANSIENT_ENGINE_ROOM = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
