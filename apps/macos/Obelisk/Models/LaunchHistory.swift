import Foundation
import Observation

/// When each account last launched each of its games from Obelisk, for every
/// storefront. The library cache keeps each launch in its game's entry; a
/// storefront that records no last played date, as Epic, learns it here.
@MainActor @Observable
final class LaunchHistory {
    private var launches: [GameIdentity: Date] {
        didSet { revision &+= 1 }
    }
    /// Changes with every launch recorded or forgotten.
    private(set) var revision = 0
    @ObservationIgnored private let cache: LibraryCache

    /// The history of the library Obelisk shows.
    static let live = LaunchHistory(cache: LibraryCache(directory: AppIdentity.libraryDirectory.path))

    init(cache: LibraryCache) {
        self.cache = cache
        launches = Dictionary(cache.launches().map { ($0.game, Date(timeIntervalSince1970: TimeInterval($0.at))) },
                              uniquingKeysWith: max)
    }

    func record<Store: GameStorefront>(_ game: Store.GameID, account: Store.AccountID, in store: Store.Type,
                                        at date: Date = .now) {
        guard let identity = Store.identity(game, account: account) else { return }
        launches[identity] = date
        cache.recordLaunch(launch: GameLaunch(game: identity, at: UInt64(max(0, date.timeIntervalSince1970))))
    }

    /// When `account` last launched `game` from Obelisk.
    func lastLaunch<Store: GameStorefront>(of game: Store.GameID, account: Store.AccountID,
                                            in store: Store.Type) -> Date? {
        Store.identity(game, account: account).flatMap { launches[$0] }
    }

    /// Drops the account's launches, as signing it out drops its library.
    func forget<Store: GameStorefront>(account: Store.AccountID, in store: Store.Type) {
        launches = launches.filter { Store.identity(of: $0.key)?.account != account }
    }
}
