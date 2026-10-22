import Foundation
import Observation

/// DBDSilicon never updates itself: its build is pinned to the runtime its fixes live in. This stands in for
/// Obelisk's Sparkle updater with the same surface, so the launcher ships without Sparkle.
@MainActor
@Observable
final class AppUpdates {
    let canCheck = false
    var installsAutomatically = false
    var quitToInstall: (_ install: @escaping () -> Void) -> Void = { $0() }

    init(starts: Bool = false) {}

    func check() {}
}
