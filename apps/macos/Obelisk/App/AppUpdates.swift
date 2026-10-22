import Foundation
import Observation
import Sparkle

/// Obelisk's own updates, through Sparkle. Once a day Obelisk reads the latest
/// release's appcast.xml and, by default, downloads a newer version and
/// installs it when Obelisk really quits; otherwise Sparkle offers it in its
/// own window.
@MainActor
@Observable
final class AppUpdates {
    /// Whether a check may start now: not while one runs, nor in a build that
    /// never updates.
    private(set) var canCheck = false
    @ObservationIgnored private let relaunch: UpdateRelaunch
    @ObservationIgnored private let controller: SPUStandardUpdaterController
    @ObservationIgnored private var observation: NSKeyValueObservation?

    /// Debug builds and the test host never start: the latest release would
    /// replace them.
    init(starts: Bool = AppUpdates.startsByDefault) {
        let relaunch = UpdateRelaunch()
        self.relaunch = relaunch
        controller = SPUStandardUpdaterController(startingUpdater: starts, updaterDelegate: relaunch,
                                                  userDriverDelegate: nil)
        // Sparkle changes it on the main thread, as it does everything.
        observation = controller.updater.observe(\.canCheckForUpdates, options: [.initial, .new]) { [weak self] _, change in
            let canCheck = change.newValue ?? false
            MainActor.assumeIsolated { self?.canCheck = canCheck }
        }
    }

    /// Whether Obelisk downloads a new version by itself and installs it when
    /// it quits. Sparkle keeps the choice; its update window can change it too.
    var installsAutomatically: Bool {
        get {
            access(keyPath: \.installsAutomatically)
            return controller.updater.automaticallyDownloadsUpdates
        }
        set {
            withMutation(keyPath: \.installsAutomatically) {
                controller.updater.automaticallyDownloadsUpdates = newValue
            }
        }
    }

    /// How Obelisk quits when the player installs a new version from Sparkle's
    /// window, before Sparkle opens the new one: quitting otherwise only hides
    /// Obelisk. It calls `install` once Obelisk may end.
    var quitToInstall: (_ install: @escaping () -> Void) -> Void {
        get { relaunch.quit }
        set { relaunch.quit = newValue }
    }

    /// Looks for a new version now, in Sparkle's window.
    func check() {
        controller.checkForUpdates(nil)
    }

    private static var startsByDefault: Bool {
        #if DEBUG
        false
        #else
        true
        #endif
    }
}

/// Holds Sparkle's relaunch until the lifecycle lets Obelisk quit.
@MainActor
private final class UpdateRelaunch: NSObject, SPUUpdaterDelegate {
    var quit: (_ install: @escaping () -> Void) -> Void = { $0() }

    func updater(_ updater: SPUUpdater, shouldPostponeRelaunchForUpdate item: SUAppcastItem,
                 untilInvokingBlock installHandler: @escaping () -> Void) -> Bool {
        quit(installHandler)
        return true
    }
}
