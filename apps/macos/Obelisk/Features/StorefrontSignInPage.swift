import AppKit
import SwiftUI

/// A storefront's sign-in, shared by every store: its logo and title over a
/// wall of covers, then a glass card holding the store's own way to sign in.
struct StorefrontSignInPage<Card: View>: View {
    private let section: LibrarySection
    private let title: String
    private let subtitle: String
    /// How wide the card grows: wider for a store with two ways to sign in.
    private let cardWidth: CGFloat
    @Binding private var warning: String?
    private let card: Card

    init(
        section: LibrarySection,
        title: String,
        subtitle: String,
        cardWidth: CGFloat = 440,
        warning: Binding<String?>,
        @ViewBuilder card: () -> Card
    ) {
        self.section = section
        self.title = title
        self.subtitle = subtitle
        self.cardWidth = cardWidth
        self._warning = warning
        self.card = card()
    }

    var body: some View {
        SignInPage(section: section, title: title, subtitle: subtitle) {
            StorefrontWarningBanner(message: $warning)
                .frame(maxWidth: cardWidth)
            card
                .padding(30)
                .frame(maxWidth: cardWidth)
                .modifier(StorefrontPanelGlass(cornerRadius: 26))
        }
    }
}

/// A sign-in page: its section's emblem, title and subtitle over a wall of
/// covers in the section's colors, then what the page offers, centered while
/// it fits. Dark in both appearances, as a library's hero: a light veil turns
/// the covers grey.
struct SignInPage<Content: View>: View {
    let section: LibrarySection
    let title: String
    let subtitle: String
    @ViewBuilder let content: Content

    var body: some View {
        GeometryReader { geometry in
            ScrollView {
                VStack(spacing: 26) {
                    header
                    content
                }
                .padding(.horizontal, ObeliskStyle.contentPadding)
                // Clear of the title bar band, and centered while it fits.
                .padding(.vertical, ObeliskStyle.titleBarHeight)
                .frame(maxWidth: .infinity, minHeight: geometry.size.height)
            }
            .scrollBounceBehavior(.basedOnSize)
        }
        .background {
            CoverWallBackdrop(section: section)
                .backgroundExtensionEffect()
                .ignoresSafeArea()
        }
        .overlay(alignment: .top) {
            StorefrontWindowDragRegion()
                .frame(height: ObeliskStyle.titleBarHeight)
        }
        .ignoresSafeArea(.container, edges: .top)
        .environment(\.colorScheme, .dark)
    }

    private var header: some View {
        VStack(spacing: 16) {
            SignInEmblem(section: section)
            VStack(spacing: 6) {
                Text(title)
                    .font(.system(size: 30, weight: .bold))
                    .tracking(-0.6)
                Text(subtitle)
                    .font(.system(size: 14))
                    .foregroundStyle(.secondary)
            }
            .multilineTextAlignment(.center)
        }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isHeader)
    }
}

/// The store's logo on glass, or Home's prism.
struct SignInEmblem: View {
    let section: LibrarySection
    var size: CGFloat = 76

    private var logo: String? {
        section == .home ? "MenuBarIcon" : section.logoAssetName
    }

    var body: some View {
        Group {
            if let logo {
                Image(logo)
                    .renderingMode(.template)
                    .resizable()
                    .scaledToFit()
                    .frame(width: size / 2, height: size / 2)
            } else {
                Image(systemName: section.symbol)
                    .font(.system(size: size * 0.4, weight: .medium))
            }
        }
        .foregroundStyle(.primary)
        .frame(width: size, height: size)
        .modifier(StorefrontPanelGlass(cornerRadius: size * 0.29))
        .accessibilityHidden(true)
    }
}

/// A wall of covers under the store's colors, or Obelisk's own.
struct CoverWallBackdrop: View {
    let section: LibrarySection

    var body: some View {
        let base = Color(white: 0.07)
        let tint = section.tint
        SignInCoverWall(glow: base.mix(with: tint, by: 0.45), soft: base.mix(with: tint, by: 0.16), base: base, tintOpacity: 0.3)
    }
}

/// Covers on a tilted wall, drifting from the top right to the bottom left,
/// under the store's colors. The bundled tile repeats seamlessly on every edge
/// and the wall moves by a whole number of tiles per cycle, so the loop never
/// shows. Core Animation runs the drift, so the app does no work between
/// frames; the colors are layers too, as SwiftUI content would sit beneath.
private struct SignInCoverWall: NSViewRepresentable {
    /// The top right corner's color.
    let glow: Color
    /// The top edge's and the bottom left corner's color.
    let soft: Color
    /// The color at the center.
    let base: Color
    /// How much of the wall the colors cover.
    let tintOpacity: Float
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeNSView(context: Context) -> CoverWallView { CoverWallView() }

    func updateNSView(_ view: CoverWallView, context: Context) {
        let environment = context.environment
        view.tint(glow: glow.resolve(in: environment).cgColor, soft: soft.resolve(in: environment).cgColor,
                  base: base.resolve(in: environment).cgColor, opacity: tintOpacity)
        view.isDrifting = !reduceMotion
    }
}

private final class CoverWallView: NSView {
    /// The tile's size in points.
    private static let tile = CGSize(width: 1280, height: 940)
    /// Clockwise on screen.
    private static let tilt: CGFloat = 15 * .pi / 180
    /// How far the wall leans back, its top receding as a film's opening crawl.
    private static let pitch: CGFloat = 55 * .pi / 180
    /// The viewer's distance from the wall, in window heights.
    private static let distance: CGFloat = 1.2
    /// How much nearer the viewer the wall comes, as a share of that distance:
    /// a quarter larger at the window's center.
    private static let approach: CGFloat = 0.2
    /// What the wall moves by in one cycle, in tiles: with the tilt, down and
    /// to the left at about 45 degrees.
    private static let drift = CGSize(width: -1, height: 2)
    private static let period: CFTimeInterval = 120

    var isDrifting = true {
        didSet { if isDrifting != oldValue { rebuild(force: true) } }
    }
    /// Holds the perspective the leaning wall is seen through.
    private let stage = CALayer()
    /// Leans back from the window.
    private let plane = CALayer()
    /// Tilted, and as large as the window seen on the plane plus a cycle's drift.
    private let wall = CALayer()
    /// The tiles, drifting inside the wall.
    private let grid = CALayer()
    /// The store's colors, from the top right corner to the bottom left one.
    private let colors = CAGradientLayer()
    /// Calmer behind the title and the card, livelier toward the edges.
    private let vignette = CAGradientLayer()
    /// Fades the wall's far end into the dark, as it nears the horizon.
    private let horizon = CAGradientLayer()
    /// When the drift started, so a rebuilt wall carries on where it was.
    private let start = CACurrentMediaTime()
    private var built: (size: CGSize, scale: CGFloat)?

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.masksToBounds = true
        wall.addSublayer(grid)
        plane.addSublayer(wall)
        stage.addSublayer(plane)
        layer?.addSublayer(stage)
        colors.startPoint = CGPoint(x: 1, y: 1)
        colors.endPoint = CGPoint(x: 0, y: 0)
        colors.locations = [0, 0.35, 0.7, 1]
        layer?.addSublayer(colors)
        vignette.type = .radial
        vignette.startPoint = CGPoint(x: 0.5, y: 0.5)
        layer?.addSublayer(vignette)
        horizon.startPoint = CGPoint(x: 0.5, y: 1)
        horizon.endPoint = CGPoint(x: 0.5, y: 0.6)
        layer?.addSublayer(horizon)
    }

    func tint(glow: CGColor, soft: CGColor, base: CGColor, opacity: Float) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        colors.colors = [glow, soft, base, soft]
        colors.opacity = opacity
        vignette.colors = [base.copy(alpha: 0.55) ?? base, base.copy(alpha: 0) ?? base]
        horizon.colors = [base.copy(alpha: 0.7) ?? base, base.copy(alpha: 0) ?? base]
        CATransaction.commit()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    // Decoration only: clicks and scrolling belong to the page above.
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func layout() {
        super.layout()
        rebuild()
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        rebuild()
    }

    private func rebuild(force: Bool = false) {
        let size = bounds.size
        let scale = window?.backingScaleFactor ?? 2
        guard force || built?.size != size || built?.scale != scale, size.width > 0, size.height > 0 else { return }
        built = (size, scale)
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }

        let tile = Self.tile
        let tilt = Self.tilt
        let pitch = Self.pitch
        let distance = Self.distance * size.height
        let approach = Self.approach * distance
        // The window's corners on the leaning plane, then on the tilted wall,
        // from the plane's center with the y axis up. The far corners lie
        // well beyond the window: the plane recedes toward a horizon above it.
        let corners = [(-1.0, -1.0), (1, -1), (-1, 1), (1, 1)].map { sx, sy in
            let screen = CGPoint(x: sx * size.width / 2, y: sy * size.height / 2)
            let y = screen.y * (1 - approach / distance) / (cos(pitch) - screen.y * sin(pitch) / distance)
            let x = screen.x * (1 + (y * sin(pitch) - approach) / distance)
            return CGPoint(x: x * cos(tilt) - y * sin(tilt), y: x * sin(tilt) + y * cos(tilt))
        }
        let reach = CGSize(width: corners.map { abs($0.x) }.max() ?? 0, height: corners.map { abs($0.y) }.max() ?? 0)
        let width = 2 * reach.width + 2 * abs(Self.drift.width) * tile.width
        let height = 2 * reach.height + 2 * abs(Self.drift.height) * tile.height
        let columns = Int((width / tile.width).rounded(.up))
        let rows = Int((height / tile.height).rounded(.up))
        colors.frame = bounds
        horizon.frame = bounds
        // A circle 620 points wide, clear from 120 points.
        let radius: CGFloat = 620
        vignette.frame = bounds
        vignette.endPoint = CGPoint(x: 0.5 + radius / size.width, y: 0.5 + radius / size.height)
        vignette.locations = [NSNumber(value: Double(120 / radius)), 1]

        stage.frame = bounds
        var perspective = CATransform3DIdentity
        perspective.m34 = -1 / distance
        stage.sublayerTransform = perspective
        plane.bounds = CGRect(origin: .zero, size: size)
        plane.position = CGPoint(x: bounds.midX, y: bounds.midY)
        // A negative angle about the x axis takes the top away from the viewer;
        // the plane then comes nearer.
        plane.transform = CATransform3DConcat(CATransform3DMakeRotation(-pitch, 1, 0, 0),
                                              CATransform3DMakeTranslation(0, 0, approach))

        wall.bounds = CGRect(x: 0, y: 0, width: CGFloat(columns) * tile.width, height: CGFloat(rows) * tile.height)
        wall.position = CGPoint(x: plane.bounds.midX, y: plane.bounds.midY)
        // The layer's y axis points up: a negative angle turns it clockwise.
        wall.transform = CATransform3DMakeRotation(-tilt, 0, 0, 1)
        grid.frame = wall.bounds

        let contents = NSImage(named: "SignInCoverWall")?.layerContents(forContentsScale: scale)
        let tiles = grid.sublayers ?? []
        tiles.dropFirst(columns * rows).forEach { $0.removeFromSuperlayer() }
        for index in 0..<columns * rows {
            let layer = index < tiles.count ? tiles[index] : CALayer()
            if layer.superlayer == nil { grid.addSublayer(layer) }
            layer.contents = contents
            layer.contentsScale = scale
            // Mipmaps keep the far, tiny covers from shimmering as they move.
            layer.minificationFilter = .trilinear
            layer.frame = CGRect(x: CGFloat(index % columns) * tile.width, y: CGFloat(index / columns) * tile.height,
                                 width: tile.width, height: tile.height)
        }

        grid.removeAnimation(forKey: "drift")
        guard isDrifting else { return }
        let drift = CABasicAnimation(keyPath: "position")
        drift.fromValue = grid.position
        drift.toValue = CGPoint(x: grid.position.x + Self.drift.width * tile.width,
                                y: grid.position.y - Self.drift.height * tile.height)
        drift.duration = Self.period
        drift.repeatCount = .infinity
        drift.timingFunction = CAMediaTimingFunction(name: .linear)
        drift.timeOffset = (CACurrentMediaTime() - start).truncatingRemainder(dividingBy: Self.period)
        grid.add(drift, forKey: "drift")
    }
}
