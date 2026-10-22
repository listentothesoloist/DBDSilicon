import Foundation
import Observation

/// Each library game's artwork as its PICS app info names it, which loads in
/// batches after the library. Steam's artwork reads it from the environment.
@MainActor
@Observable
final class SteamLibraryArtwork {
    /// A game PICS has not told of yet has none.
    private(set) var games: [UInt32: SteamGameArtwork] = [:]

    @ObservationIgnored private var loadID: UUID?
    @ObservationIgnored private var task: Task<Void, Never>?

    deinit {
        task?.cancel()
    }

    /// Loads the games' artwork in `language`, in batches. What is already
    /// known of them, such as the library cache's, shows until their batch
    /// replaces it; a batch Steam does not answer keeps it.
    func load(appIds: [UInt32], language: String, session: any SteamSessionProtocol) {
        cancel()
        var seen: Set<UInt32> = []
        let appIds = appIds.filter { seen.insert($0).inserted }
        let kept = games.filter { seen.contains($0.key) }
        if kept.count != games.count { games = kept }
        guard !appIds.isEmpty else { return }

        let loadID = UUID()
        self.loadID = loadID
        task = Task { [weak self] in
            defer {
                if self?.loadID == loadID {
                    self?.loadID = nil
                    self?.task = nil
                }
            }
            for start in stride(from: 0, to: appIds.count, by: 50) {
                guard !Task.isCancelled, self?.loadID == loadID else { return }
                let batch = Array(appIds[start..<min(start + 50, appIds.count)])
                let result = try? await session.platforms(appIds: batch, language: language)
                // UniFFI can return after cancellation or account replacement.
                guard !Task.isCancelled, let self, self.loadID == loadID else { return }
                guard let result else { continue }
                let merged = self.games.merging(
                    result.compactMap { game in game.artwork.map { (game.appId, $0) } }
                ) { _, loaded in loaded }
                if merged != self.games { self.games = merged }
            }
        }
    }

    /// Shows what the library cache knew of the games, until a load replaces it.
    func show(_ artwork: [UInt32: SteamGameArtwork]) {
        reset()
        games = artwork
    }

    /// Stops loading, keeping what loaded so far: the same account's next
    /// load refreshes it.
    func cancel() {
        task?.cancel()
        task = nil
        loadID = nil
    }

    /// Forgets every game, as another account's library replaces them.
    func reset() {
        cancel()
        games.removeAll()
    }
}
