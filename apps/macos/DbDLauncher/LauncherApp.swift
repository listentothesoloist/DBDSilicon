import SwiftUI

/// DBDSilicon: Dead by Daylight on Apple silicon. It signs in to Steam, downloads the game and starts it on the
/// Linux engine, on Obelisk's open-source core (GPL-3.0).
@main
struct DbDLauncherApp: App {
    @State private var model: AppModel
    @NSApplicationDelegateAdaptor(AppLifecycle.self) private var lifecycle

    init() {
        // Nothing leaves this Mac but what Steam needs: no performance reports to getobelisk.app.
        UserDefaults.standard.register(defaults: [AppModel.sharePerformanceKey: false])
        _model = State(initialValue: AppModel(updates: AppUpdates()))
    }

    /// The legal notices GPL-3.0 asks an interactive program to show, and nothing else of where its core comes from.
    private static func showAbout() {
        let text = """
        Dead by Daylight on Macs with Apple silicon.

        Based on Obelisk (github.com/Zagrios/Obelisk) and Voile, free software under the GNU GPL v3. \
        DBDSilicon is distributed under the same terms, without any warranty; the license text is inside the app \
        (Contents/Resources/LICENSE.txt).

        Dead by Daylight is a trademark of Behaviour Interactive Inc. Steam is a trademark of Valve Corporation.
        """
        let credits = NSAttributedString(string: text, attributes: [
            .font: NSFont.systemFont(ofSize: 11),
            .foregroundColor: NSColor.secondaryLabelColor,
        ])
        NSApp.orderFrontStandardAboutPanel(options: [.credits: credits])
        NSApp.activate()
    }

    var body: some Scene {
        Window("DBDSilicon", id: "main") {
            LauncherView(model: model)
                .environment(\.locale, L10n.locale)
                .environment(model.steam.libraryArtwork)
        }
        .windowResizability(.contentSize)
        .commands {
            CommandGroup(replacing: .appInfo) {
                Button("About DBDSilicon", action: Self.showAbout)
            }
            CommandGroup(replacing: .newItem) {}
        }
        Settings {
            LauncherSettings(model: model)
                .environment(\.locale, L10n.locale)
        }
        // Obelisk's menu bar item: it reopens the window and stops the game before the launcher quits.
        MenuBarExtra {
            AppMenuBarMenu(lifecycle: lifecycle)
        } label: {
            AppMenuBarLabel(model: model, lifecycle: lifecycle)
        }
    }
}
