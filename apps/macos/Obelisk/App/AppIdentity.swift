import Foundation

enum AppIdentity {
    // Keep the display name independent of storage identifiers for a future rename.
    static let name = Bundle.main.object(forInfoDictionaryKey: "CFBundleDisplayName") as? String ?? "DBDSilicon"
    static let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0.1.0"
    static let storageIdentifier = "local.dbdsilicon"
    /// The library folder in Application Support.
    static let libraryFolderName = "DBDSilicon"

    static var libraryDirectory: URL {
        // The override is for isolated development and integration tests.
        if let path = ProcessInfo.processInfo.environment["OBELISK_LIBRARY_DIRECTORY"], !path.isEmpty {
            return URL(fileURLWithPath: path, isDirectory: true)
        }
        return URL.applicationSupportDirectory.appending(path: libraryFolderName, directoryHint: .isDirectory)
    }
}

extension ObeliskLocations {
    /// Every path of an Obelisk whose library lives in `library`. Tests pass their own.
    static func of(library: URL) -> ObeliskLocations {
        obeliskLocations(library: library.path, bundle: Bundle.main.bundleURL.path, home: NSHomeDirectory())
    }

    @MainActor static let live = of(library: AppIdentity.libraryDirectory)
}
