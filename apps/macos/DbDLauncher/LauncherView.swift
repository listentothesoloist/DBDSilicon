import SwiftUI

enum DeadByDaylight {
    static let appId: UInt32 = 381210
    /// How the game runs on this Mac: Obelisk's Linux engine, where its Easy Anti-Cheat loads its Linux module,
    /// DXMT for Direct3D 11, and the game's own Direct3D 11 renderer.
    static let choices = LaunchChoices(arguments: "-dx11", engine: .linux, graphics: .dxmt)

    /// The account's Windows installation of the game.
    @MainActor static func receipt(in installations: SteamInstallations) -> InstallationReceipt<UInt32>? {
        installations.receipts.first { $0.gameID == appId && $0.platform == .windows }
    }

    /// Keeps the game on the setup it runs with on this Mac: a fresh installation starts with Obelisk's defaults.
    /// `performanceHud` changes whether Metal's HUD shows over the game; nil keeps it as it is.
    @MainActor static func keepChoices(of receipt: InstallationReceipt<UInt32>, in installations: SteamInstallations,
                                       performanceHud: Bool? = nil) async -> LaunchSettings? {
        let current = try? await installations.launchSettings(receipt)
        let choices = current?.choices
        let hud = performanceHud ?? choices?.performanceHud ?? false
        if choices?.engine == .linux, choices?.graphics == .dxmt,
           choices?.arguments?.split(separator: " ").contains("-dx11") == true, choices?.performanceHud == hud {
            return current
        }
        var wanted = Self.choices
        wanted.performanceHud = hud
        return (try? await installations.setLaunchSettings(receipt, choices: wanted)) ?? current
    }
}

/// The launcher's one small window: the game, what it is doing, and the one button that matters.
struct LauncherView: View {
    let model: AppModel
    private var steam: SteamModel { model.steam }

    /// The account's copy of the game, once its library loaded.
    private var game: SteamGame? { steam.library.game(DeadByDaylight.appId) }
    private var isWaiting: Bool { steam.isRestoring || steam.isConnecting }

    var body: some View {
        content
            .frame(width: 380, height: phase == .signIn ? 500 : 400)
            .animation(.smooth(duration: 0.25), value: phase)
            // The desktop shows through the window, so the glass above it has something to bend.
            .containerBackground(.thinMaterial, for: .window)
            .toolbar {
                ToolbarItem(placement: .primaryAction) {
                    SettingsLink {
                        Label("Settings", systemImage: "gearshape")
                    }
                    .help("Settings")
                }
            }
            .task { await steam.start(signIn: true) }
            .task(id: game?.appId) { openGame() }
            .task(id: steam.isConnected) { openGame() }
    }

    private enum Phase: Equatable { case waiting, signIn, library, notOwned, game }

    private var phase: Phase {
        if isWaiting { return .waiting }
        if !steam.isConnected { return .signIn }
        if game == nil { return steam.isLoadingGames ? .library : .notOwned }
        return .game
    }

    /// Shows the game's page state, as Obelisk's game page does, so its button reads the right offer.
    private func openGame() {
        guard steam.isConnected, let game, steam.selectedGame?.appId != game.appId else { return }
        steam.open(game)
    }

    @ViewBuilder private var content: some View {
        switch phase {
        case .waiting:
            Waiting(text: steam.isRestoring ? "Restoring your Steam session…" : "Connecting to Steam…")
        case .signIn:
            LauncherSignIn(steam: steam)
        case .library:
            Waiting(text: "Loading your Steam library…")
        case .notOwned:
            ContentUnavailableView {
                Label("Not in Your Library", systemImage: "exclamationmark.triangle")
            } description: {
                Text("Sign in with the Steam account that owns Dead by Daylight.")
            } actions: {
                Button("Use Another Account", action: steam.signOut).buttonStyle(.glass)
            }
        case .game:
            GamePane(steam: steam)
        }
    }
}

private struct Waiting: View {
    let text: String

    var body: some View {
        VStack(spacing: 12) {
            ProgressView()
            Text(text).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

/// The app's own icon over a title and subtitle.
struct PaneHeader: View {
    let title: String
    let subtitle: String
    var iconSize: CGFloat = 72

    var body: some View {
        VStack(spacing: 6) {
            Image(nsImage: NSApp.applicationIconImage)
                .resizable()
                .frame(width: iconSize, height: iconSize)
            Text(title).font(.title2.weight(.semibold))
            Text(subtitle)
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
                .contentTransition(.opacity)
        }
        .frame(maxWidth: .infinity)
    }
}

// MARK: - Game

private struct GamePane: View {
    let steam: SteamModel

    private var installations: SteamInstallations { steam.installations }
    private var receipt: InstallationReceipt<UInt32>? { DeadByDaylight.receipt(in: installations) }

    var body: some View {
        VStack(spacing: 20) {
            Spacer(minLength: 0)
            PaneHeader(title: "Dead by Daylight", subtitle: statusTitle, iconSize: 112)
            if let action = installations.selectedAction {
                ActionProgress(action: action).frame(maxWidth: 280)
                ActionButtons(action: action, forceQuit: installations.forceQuitRunningGameNow)
            } else {
                ProgressView().controlSize(.small)
            }
            if let message = steam.gameSessionMessage ?? steam.errorMessage {
                Label(message, systemImage: "exclamationmark.triangle.fill")
                    .font(.callout)
                    .foregroundStyle(.orange)
                    .textSelection(.enabled)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .glassEffect(.regular.tint(.orange.opacity(0.15)), in: .rect(cornerRadius: 16, style: .continuous))
            }
            Spacer(minLength: 0)
        }
        .padding(24)
        // Centred on the whole window, title bar included, as the eye reads it.
        .padding(.bottom, 52)
        .animation(.smooth(duration: 0.25), value: statusTitle)
        .task(id: receipt?.id) {
            guard let receipt else { return }
            _ = await DeadByDaylight.keepChoices(of: receipt, in: installations)
        }
    }

    private var statusTitle: String {
        guard let action = installations.selectedAction else { return "Getting the game ready…" }
        switch action.kind {
        case .launch: return "Ready to play"
        case .download(.install): return "Not installed"
        case .download(.resume): return "Download paused"
        case .download(.update): return "Update available"
        case .queued: return "Waiting to download"
        case .transferring(.files): return receipt == nil ? "Downloading…" : "Updating…"
        case .transferring(.launch): return "Verifying files…"
        case .waiting(.launch): return "Starting…"
        case .waiting(.closing): return "Closing…"
        case .waiting: return "Please wait…"
        case .step(.cloudSync): return "Syncing Steam Cloud…"
        case .step(.runtime): return "Downloading engine components…"
        case .running: return "Playing"
        }
    }
}

private struct ActionButtons: View {
    let action: GameAction
    let forceQuit: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            if action.isCancellable {
                Button("Cancel", action: action.cancel)
                    .buttonStyle(.glass)
            }
            if case let .running(stage) = action.kind {
                // Stop always forces the game to quit at once.
                Button(action: forceQuit) {
                    Label("Force Quit", systemImage: "xmark.octagon.fill").frame(maxWidth: .infinity)
                }
                .buttonStyle(.glassProminent)
                .tint(.red)
                .disabled(stage == nil)
            } else {
                Button(action: action.perform) {
                    Label(title, systemImage: symbol).frame(maxWidth: .infinity)
                }
                .buttonStyle(.glassProminent)
                .disabled(!action.isAvailable)
                .keyboardShortcut(.defaultAction)
            }
        }
        .controlSize(.large)
        .frame(maxWidth: action.isCancellable ? 240 : 150)
    }

    private var title: String {
        switch action.kind {
        case .launch: "Play"
        case .download(.install): "Download"
        case .download(.resume): "Resume"
        case .download(.update): "Update"
        default: action.title
        }
    }

    private var symbol: String {
        switch action.kind {
        case .launch: "play.fill"
        case .download(.update): "arrow.triangle.2.circlepath"
        case .download: "arrow.down"
        case .queued: "clock"
        default: "hourglass"
        }
    }
}

/// The running Transfer's progress, read in its own body so each tick redraws this view alone.
private struct ActionProgress: View {
    let action: GameAction
    @State private var speed = TransferSpeed()

    var body: some View {
        if let transfer = action.transfer(), transfer.totalBytes > 0 {
            let fraction = min(1, Double(transfer.completedBytes) / Double(transfer.totalBytes))
            VStack(spacing: 4) {
                ProgressView(value: fraction)
                HStack {
                    Text("\(bytes(transfer.completedBytes)) of \(bytes(transfer.totalBytes))")
                    if let rate = speed.rate { Text("· \(bytes(UInt64(rate)))/s") }
                    Spacer()
                    Text(fraction.formatted(.percent.precision(.fractionLength(0))))
                }
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
            }
            .onChange(of: transfer.networkBytes, initial: true) { _, network in speed.sample(network) }
        } else if let fraction = action.stepFraction() {
            ProgressView(value: fraction)
        }
    }

    private func bytes(_ count: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(count), countStyle: .file)
    }
}

/// Download speed over the last few seconds of network bytes.
private struct TransferSpeed {
    private var samples: [(time: ContinuousClock.Instant, bytes: UInt64)] = []

    var rate: Double? {
        guard let first = samples.first, let last = samples.last, last.time > first.time else { return nil }
        let seconds = Double((last.time - first.time).components.attoseconds) / 1e18
            + Double((last.time - first.time).components.seconds)
        guard seconds > 0.5, last.bytes >= first.bytes else { return nil }
        return Double(last.bytes - first.bytes) / seconds
    }

    mutating func sample(_ bytes: UInt64) {
        let now = ContinuousClock.now
        if let last = samples.last, bytes < last.bytes { samples.removeAll() }
        samples.append((now, bytes))
        samples.removeAll { now - $0.time > .seconds(4) }
    }
}
