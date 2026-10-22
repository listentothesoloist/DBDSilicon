import SwiftUI

/// DBDSilicon's Settings window (⌘,): the FPS overlay, the installation, and the account.
struct LauncherSettings: View {
    let model: AppModel
    @State private var performanceHud: Bool?

    private var steam: SteamModel { model.steam }
    private var installations: SteamInstallations { steam.installations }
    private var receipt: InstallationReceipt<UInt32>? { DeadByDaylight.receipt(in: installations) }

    var body: some View {
        VStack(spacing: 18) {
            if let receipt {
                GlassGroup(title: "Game") {
                    GlassRow(label: "Show FPS") {
                        Toggle("Show FPS", isOn: Binding(
                            get: { performanceHud ?? false },
                            set: { shown in setPerformanceHud(shown, receipt: receipt) }))
                            .toggleStyle(.switch)
                            .labelsHidden()
                            .disabled(performanceHud == nil)
                    }
                    GlassRow("Size", value: ByteCountFormatter.string(fromByteCount: Int64(receipt.totalBytes),
                                                                      countStyle: .file))
                    GlassRow(label: "Location") {
                        Button("Show in Finder") {
                            NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: receipt.directory)])
                        }
                        .buttonStyle(.glass)
                        .controlSize(.small)
                    }
                } footer: {
                    Text("Shows the frame rate and frame times over the game, from its next launch.")
                }
            }
            if steam.isConnected {
                GlassGroup(title: "Steam") {
                    GlassRow(label: steam.displayName ?? "Signed in") {
                        Button("Sign Out", action: steam.signOut)
                            .buttonStyle(.glass)
                            .controlSize(.small)
                    }
                }
            }
        }
        .padding(20)
        .frame(width: 400)
        .fixedSize(horizontal: false, vertical: true)
        .containerBackground(.thinMaterial, for: .window)
        .task(id: receipt?.id) {
            guard let receipt else { return }
            performanceHud = (try? await installations.launchSettings(receipt))?.performanceHud ?? false
        }
    }

    private func setPerformanceHud(_ shown: Bool, receipt: InstallationReceipt<UInt32>) {
        performanceHud = shown
        Task {
            let settings = await DeadByDaylight.keepChoices(of: receipt, in: installations, performanceHud: shown)
            performanceHud = settings?.performanceHud ?? shown
        }
    }
}
