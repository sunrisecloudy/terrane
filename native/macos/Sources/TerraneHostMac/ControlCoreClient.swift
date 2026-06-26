import Foundation

/// Thin adapter over [`ForgeCoreBridge`] for debug-gated `control.*` commands.
final class ControlCoreClient: @unchecked Sendable {
    private let core: ForgeCoreBridge

    init(core: ForgeCoreBridge = ForgeCoreBridge()) {
        self.core = core
    }

    var isAvailable: Bool {
        core.isAvailable
    }

    func invoke(name: String, payload: [String: Any]) -> [String: Any]? {
        guard core.isAvailable else {
            return nil
        }
        do {
            guard let result = try core.controlCommand(name: name, payload: payload) as? [String: Any] else {
                return nil
            }
            return result
        } catch {
            return nil
        }
    }
}