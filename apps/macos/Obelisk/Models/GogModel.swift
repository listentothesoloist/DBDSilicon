import Foundation

typealias GogModel = WebAccountModel<GogGames>

extension GogGames: WebStorefront {
    typealias Session = any GogSessionProtocol
    typealias Credentials = GogCredentials
    typealias Game = GogGame
    typealias Details = GogGameDetails

    static let storeID = "gog"
    /// The store through its site's own sign-in, as its Sign in button
    /// opens it: GOG's site takes no token of the app, but its sign-in page
    /// remembers the account Obelisk's sign-in signed in there, and then
    /// returns to the store signed in without asking.
    static let storeURL: URL = {
        let back = "https://www.gog.com/on_login_success?returnTo=/"
            .addingPercentEncoding(withAllowedCharacters: .alphanumerics)!
        return URL(string: "https://login.gog.com/auth?client_id=46755278331571209&response_type=code&redirect_uri=\(back)")!
    }()
    static let webSignIn = WebSignIn.gog
    static func keychain() -> any CredentialStore<GogCredentials> { GogKeychain() }

    static func signIn(code: String, library: LibraryCache?) async throws -> Session {
        try await GogSession.signIn(authorizationCode: code, library: library)
    }
    static func restore(_ credentials: GogCredentials, library: LibraryCache?) async throws -> Session {
        try await GogSession.restore(credentials: credentials, library: library)
    }
    static func accountID(_ credentials: GogCredentials) -> String { credentials.userId }
    static func accountName(_ credentials: GogCredentials) -> String { credentials.username }
    static func avatar(of session: Session) -> URL? { session.avatarUrl().flatMap(URL.init(string:)) }
    static func rotating(_ session: Session) -> RotatingSession<GogCredentials> {
        RotatingSession(
            identity: ObjectIdentifier(session),
            credentials: { try await session.credentials() },
            persist: { (saver: any CredentialSave) in await session.persist(credentials: saver) },
            shutdownObservers: { session.shutdownObservers() }
        )
    }
    /// GOG offers no revocation of a Galaxy token: this only forgets the session.
    static func disconnect(_ session: Session) async throws { try await session.disconnect() }

    static func games(of session: Session) async throws -> [GogGame] { try await session.games() }
    /// GOG answers each game's playtime on its own, in minutes.
    static func playtimes(of session: Session, games: [GogGame]) async throws -> [String: Duration] {
        let loaded = try await session.playtimes(productIds: games.map(\.productId))
        return Dictionary(loaded.map { ($0.productId, played(minutes: $0.minutes)) }, uniquingKeysWith: max)
    }
    /// GOG answers one game at a time.
    static func achievementShares(of games: [GogGame], session: Session, language: String) async throws -> [String: Double] {
        try await session.achievementShares(productIds: games.map(\.productId), locale: achievementLocale(language))
    }
    static func cachedLibrary(in cache: LibraryCache, account: String)
        -> [(game: GogGame, playtime: Duration?, achievements: Double?)] {
        cache.gogLibrary(userId: account).map {
            ($0.game, $0.playtimeMinutes.map(played(minutes:)), $0.achievementShare)
        }
    }
    /// `language` names the depots, such as "fr-FR".
    static func details(of game: GogGame, session: Session, language: String) async throws -> GogGameDetails {
        try await session.gameDetails(productId: game.productId, language: language)
    }
    static func achievements(of game: GogGame, session: Session, language: String,
                             fresh: Bool) async throws -> [GameAchievement] {
        try await session.achievements(productId: game.productId, locale: achievementLocale(language), fresh: fresh)
            .map(GameAchievement.init)
    }
    /// GOG names achievements in a full language tag, such as "fr-FR".
    private static func achievementLocale(_ language: String) -> String {
        Locale(identifier: language).identifier(.bcp47)
    }
    @MainActor static func installations(of session: Session, account: String, language: String,
                                          sessions: any StorefrontSessionsProtocol, receipts: URL,
                                          locations: ObeliskLocations) -> any InstallationsBackend<GogGames> {
        guard let session = session as? GogSession, let sessions = sessions as? StorefrontSessions else {
            return NoInstallations<GogGames>()
        }
        return RustInstallations<GogGames>(try! AccountInstallations.gog(
            session: session, userId: account, language: language, sessions: sessions,
            receipts: receipts.path, locations: locations
        ))
    }
    static func isUnauthorized(_ error: Error) -> Bool {
        if case GogError.Unauthorized = error { true } else { false }
    }
    @MainActor static func describe(_ error: Error) -> String {
        switch error {
        case GogError.Network: L10n.string("model.gog.network")
        case GogError.Unauthorized: L10n.string("model.gog.unauthorized")
        case GogError.InvalidCode: L10n.string("model.gog.invalidCode")
        case GogError.InvalidResponse: L10n.string("model.gog.invalidResponse")
        case let GogError.Service(message): L10n.message(message)
        case let browserError as WebAuthenticationError: browserError.localizedDescription
        case let failure as GameInstallationFailure: failure.localizedDescription
        default: L10n.string("model.gog.failure")
        }
    }

    static var sessionNotSaved: String { L10n.string("model.gog.sessionNotSaved") }
    static var revocationUnconfirmed: String { L10n.string("model.gog.revocationUnconfirmed") }
    static var otherGameRunning: String { L10n.string("model.gog.otherGameRunning") }
    /// GOG asks no Cloud question: Obelisk does not sync its saves.
    static var cloudTitle: String { L10n.string("ui.store.gog") }

    /// GOG counts playtime in minutes. A count too large to hold saturates.
    static func played(minutes: UInt64) -> Duration {
        let (seconds, overflow) = minutes.multipliedReportingOverflow(by: 60)
        return .seconds(Int64(clamping: overflow ? .max : seconds))
    }
}

extension GogGameDetails: WebGameDetails {}

extension GogGame: WebGame {
    public var id: String { productId }

    /// What GOG can install of the game. GOG picks the build as the Transfer
    /// starts.
    var offer: GameOffer? {
        guard macos || windows else { return nil }
        return .gog(productId: productId, title: title)
    }

    func libraryValue(for sort: StorefrontLibrarySort) -> Double? {
        sort == .dateAdded ? addedAt.map { Double($0) } : nil
    }
}
