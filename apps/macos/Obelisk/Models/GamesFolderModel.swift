import AppKit
import Foundation
import Observation

/// Obelisk's games folder, as Rust keeps it. Each change returns at once;
/// the moves it queues show in the download queue.
@MainActor
protocol GamesFolderBackend: AnyObject {
    func state() -> GamesFolderState
    /// Waits for a revision newer than `after`.
    func changed(after: UInt64) async -> GamesFolderState
    /// What choosing `folder` would do. It reads the disk.
    func inspect(_ folder: String) -> GamesFolderInspection
    /// What choosing Obelisk's own folder again would do. It makes that folder.
    func inspectDefault() -> GamesFolderInspection
    func choose(_ folder: String) throws
    func chooseDefault() throws
    /// Queues the move of the games still outside the games folder.
    func gather()
    /// A disk came or left.
    func volumesChanged()
}

/// The games folder every account's installations of the library share in Rust.
final class RustGamesFolder: GamesFolderBackend {
    private let core: GamesFolder

    init(library: URL) {
        core = GamesFolder(library: library.path)
    }

    func state() -> GamesFolderState { core.state() }
    func changed(after: UInt64) async -> GamesFolderState { await core.changed(after: after) }
    func inspect(_ folder: String) -> GamesFolderInspection { core.inspect(chosen: folder) }
    func inspectDefault() -> GamesFolderInspection { core.inspectDefault() }
    func choose(_ folder: String) throws { try core.choose(chosen: folder) }
    func chooseDefault() throws { try core.chooseDefault() }
    func gather() { core.gather() }
    func volumesChanged() { core.volumesChanged() }
}

/// A folder the player picked for the games, and what choosing it does,
/// before they agree.
struct GamesFolderProposal: Identifiable, Equatable {
    let id = UUID()
    /// The folder the player picked; nil for Obelisk's own on this Mac.
    let chosen: URL?
    /// Where the games go: Obelisk's folder inside the picked one.
    let path: String
    /// The installed games that move there.
    let games: Int
    /// What those of another disk copy there.
    let copiedBytes: UInt64
    /// The disk's name, as Finder shows it.
    let volume: String
    let availableBytes: UInt64?
    /// The disk's format when games may not all run from it, such as ExFAT.
    let format: String?
    let isReadOnly: Bool

    /// Whether the copies outgrow the disk's free space.
    var lacksSpace: Bool { availableBytes.map { $0 < copiedBytes } ?? false }
    var canChoose: Bool { !isReadOnly && !lacksSpace }

    /// The disk facts of `folder`, which it reads.
    static func volume(of folder: URL) -> (name: String, available: UInt64?, format: String?, readOnly: Bool) {
        let keys: Set<URLResourceKey> = [
            .volumeLocalizedNameKey, .volumeAvailableCapacityForImportantUsageKey, .volumeAvailableCapacityKey,
            .volumeIsReadOnlyKey, .volumeTypeNameKey, .volumeLocalizedFormatDescriptionKey,
        ]
        let values = try? folder.resourceValues(forKeys: keys)
        // An external disk answers zero for important usage: only the Mac's
        // own counts what it can purge.
        let important = values?.volumeAvailableCapacityForImportantUsage.map { UInt64(max(0, $0)) }
        let plain = values?.volumeAvailableCapacity.map { UInt64(max(0, $0)) }
        let available = [important, plain].compactMap { $0 }.max()
        // Only Mac formats keep everything a game holds, such as its links.
        let type = values?.volumeTypeName?.lowercased()
        let recommended = type.map { ["apfs", "hfs"].contains($0) } ?? true
        return (
            values?.volumeLocalizedName ?? folder.lastPathComponent,
            available,
            recommended ? nil : (values?.volumeLocalizedFormatDescription ?? values?.volumeTypeName),
            values?.volumeIsReadOnly ?? false
        )
    }
}

/// Where Obelisk installs games, and the player's change of it: the folder
/// they pick, what moving the games there does, then the moves, which run in
/// the download queue.
@MainActor @Observable
final class GamesFolderModel {
    private(set) var path: String
    /// Obelisk's own folder, inside its library on this Mac.
    private(set) var isDefault: Bool
    /// Whether the folder's disk is there: Obelisk's own folder always is.
    private(set) var isConnected: Bool
    /// The folder the player picked, waiting for them to agree.
    var proposal: GamesFolderProposal?
    /// Why the last folder the player picked was not taken.
    var failure: String?

    @ObservationIgnored private let backend: any GamesFolderBackend
    @ObservationIgnored private var revision: UInt64 = 0
    @ObservationIgnored private var observation: Task<Void, Never>?
    @ObservationIgnored private var volumes: [NSObjectProtocol] = []
    @ObservationIgnored private let center: NotificationCenter

    init(library: URL, backend: (any GamesFolderBackend)? = nil, workspace: NotificationCenter? = nil) {
        let backend = backend ?? RustGamesFolder(library: library)
        self.backend = backend
        let center = workspace ?? NSWorkspace.shared.notificationCenter
        self.center = center
        let state = backend.state()
        path = state.path
        isDefault = state.isDefault
        isConnected = state.isConnected
        revision = state.revision
        observation = Task { [weak self, backend] in
            var after = self?.revision ?? 0
            while !Task.isCancelled {
                let state = await backend.changed(after: after)
                guard let self else { return }
                after = state.revision
                apply(state)
            }
        }
        // A game on a disk that left shows as not installed until it comes back.
        volumes = [NSWorkspace.didMountNotification, NSWorkspace.didUnmountNotification,
                   NSWorkspace.didRenameVolumeNotification].map { name in
            center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.volumesChanged() }
            }
        }
    }

    isolated deinit {
        observation?.cancel()
        for volume in volumes { center.removeObserver(volume) }
    }

    private func apply(_ state: GamesFolderState) {
        guard state.revision > revision else { return }
        revision = state.revision
        if path != state.path { path = state.path }
        if isDefault != state.isDefault { isDefault = state.isDefault }
        if isConnected != state.isConnected { isConnected = state.isConnected }
    }

    /// The games folder's new state follows through `changed(after:)`.
    private func volumesChanged() { backend.volumesChanged() }

    /// Whether `directory`, an installation's folder, is inside the games folder.
    func holds(_ directory: String) -> Bool {
        directory.hasPrefix(path.hasSuffix("/") ? path : path + "/")
    }

    /// Asks the player to move the games to the folder they picked.
    func propose(_ folder: URL) {
        propose(chosen: folder, backend.inspect(folder.path))
    }

    /// Asks the player to move the games back to Obelisk's own folder.
    func proposeDefault() {
        propose(chosen: nil, backend.inspectDefault())
    }

    private func propose(chosen: URL?, _ inspection: GamesFolderInspection) {
        failure = nil
        switch inspection {
        case let .ready(path, games, copiedBytes):
            // Obelisk's own folder exists once inspected; the one inside a
            // picked folder only once chosen.
            let volume = GamesFolderProposal.volume(of: chosen ?? URL(filePath: path, directoryHint: .isDirectory))
            proposal = GamesFolderProposal(
                chosen: chosen, path: path, games: Int(games), copiedBytes: copiedBytes, volume: volume.name,
                availableBytes: volume.available, format: volume.format, isReadOnly: volume.readOnly)
        case .missing:
            failure = L10n.string("model.gamesFolder.missing")
        case .inside:
            failure = L10n.string("model.gamesFolder.inside", AppIdentity.name)
        }
    }

    /// Takes the folder the player agreed to: the games move there.
    func confirm() {
        guard let proposal, proposal.canChoose else { return }
        self.proposal = nil
        do {
            if let chosen = proposal.chosen {
                try backend.choose(chosen.path)
            } else {
                try backend.chooseDefault()
            }
            apply(backend.state())
        } catch {
            failure = L10n.string("model.gamesFolder.failed", L10n.message(Self.reason(error)))
        }
    }

    /// Moves the games still outside the games folder.
    func gather() { backend.gather() }

    private static func reason(_ error: Error) -> String {
        if case let GameSessionError.Failure(reason) = error { return reason }
        return error.localizedDescription
    }
}
