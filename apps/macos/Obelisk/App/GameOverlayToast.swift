import AppKit
import SwiftUI

/// What the toast tells over the game, such as a friend's invitation.
struct OverlayToast: Equatable {
    /// Whose avatar it shows, when the friends list knows them.
    let friend: FriendPresentation?
    let message: String
    let hint: String
}

/// Tells of something over the running game while the overlay is closed,
/// without taking the game's focus.
@MainActor
protocol OverlayToastPresenter: AnyObject {
    /// Shows `toast` on the screen of `game`'s window, or the main screen, for
    /// a few seconds; a click on it calls `open`. It replaces the one showing.
    func show(_ toast: OverlayToast, avatars: FriendsAvatarCache, over game: OverlayApplication?,
              open: @escaping @MainActor () -> Void)
    func hide()
}

/// The toast's panel, in the top-right corner of the game's screen. It never
/// activates Obelisk, so the game keeps its keyboard and pointer.
@MainActor
final class OverlayToastPanelPresenter: OverlayToastPresenter {
    static let duration: Duration = .seconds(8)
    static let margin: CGFloat = 24
    private var panel: OverlayToastPanel?
    private var expiry: Task<Void, Never>?

    func show(_ toast: OverlayToast, avatars: FriendsAvatarCache, over game: OverlayApplication?,
              open: @escaping @MainActor () -> Void) {
        let panel = self.panel ?? OverlayToastPanel()
        self.panel = panel
        panel.show(OverlayToastView(toast: toast, avatars: avatars) { [weak self] in
            self?.hide()
            open()
        })
        let size = panel.contentView?.fittingSize ?? NSSize(width: OverlayToastView.width, height: 72)
        if let screen = GameOverlayPanelPresenter.screen(showing: game?.pid) ?? NSScreen.main {
            panel.setFrame(NSRect(x: screen.frame.maxX - size.width - Self.margin,
                                  y: screen.frame.maxY - size.height - Self.margin,
                                  width: size.width, height: size.height), display: true)
        }
        panel.orderFrontRegardless()
        expiry?.cancel()
        expiry = Task { [weak self] in
            try? await Task.sleep(for: Self.duration)
            guard !Task.isCancelled else { return }
            self?.hide()
        }
    }

    func hide() {
        expiry?.cancel()
        expiry = nil
        panel?.orderOut(nil)
    }
}

@MainActor
final class OverlayToastPanel: NSPanel {
    init() {
        super.init(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        level = .gameOverlay
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .ignoresCycle, .stationary]
        hidesOnDeactivate = false
        isReleasedWhenClosed = false
        ignoresMouseEvents = false
        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        isMovable = false
        animationBehavior = .utilityWindow
        identifier = NSUserInterfaceItemIdentifier("game-overlay-toast")
    }

    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    func show(_ view: OverlayToastView) {
        let root = view
            .environment(\.locale, L10n.locale)
            .environment(\.colorScheme, .dark)
            .tint(ObeliskStyle.accent)
        if let host = contentView as? OverlayToastHostingView<AnyView> {
            host.rootView = AnyView(root)
        } else {
            contentView = OverlayToastHostingView(rootView: AnyView(root))
        }
    }
}

/// The game keeps the focus: the first click on the toast already counts.
private final class OverlayToastHostingView<Content: View>: NSHostingView<Content> {
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

struct OverlayToastView: View {
    static let width: CGFloat = 380
    let toast: OverlayToast
    let avatars: FriendsAvatarCache
    let open: @MainActor () -> Void

    var body: some View {
        Button(action: open) {
            HStack(spacing: 12) {
                if let friend = toast.friend {
                    FriendAvatar(friend: friend, avatars: avatars, size: 40)
                } else {
                    Image(systemName: "person.crop.circle.badge.plus")
                        .font(.system(size: 26))
                        .frame(width: 40, height: 40)
                        .accessibilityHidden(true)
                }
                VStack(alignment: .leading, spacing: 3) {
                    Text(toast.message)
                        .font(.system(size: 13, weight: .semibold))
                        .lineLimit(2)
                    Text(toast.hint)
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
            }
            .foregroundStyle(.primary)
            .padding(14)
            .frame(width: Self.width, alignment: .leading)
            .modifier(StorefrontPanelGlass(cornerRadius: 18))
            .contentShape(RoundedRectangle(cornerRadius: 18))
        }
        .obeliskButtonStyle(.plain)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("game-overlay-toast")
    }
}

@MainActor
final class InertOverlayToastPresenter: OverlayToastPresenter {
    func show(_ toast: OverlayToast, avatars: FriendsAvatarCache, over game: OverlayApplication?,
              open: @escaping @MainActor () -> Void) {}
    func hide() {}
}
