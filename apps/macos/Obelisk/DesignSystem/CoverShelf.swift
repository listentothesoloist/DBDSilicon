import SwiftUI

/// Where each of a page's shelves was scrolled, and how far it can go, kept
/// while a game's page replaces it. Nothing observes it, so scrolling redraws
/// nothing.
@MainActor
final class ShelfScrollMemory {
    fileprivate struct Shelf {
        var offset: CGFloat = 0
        var maxOffset: CGFloat = 0
        /// The width the cards show in, between the margins.
        var width: CGFloat = 0
    }

    fileprivate var shelves: [String: Shelf] = [:]
}

/// A titled row of covers that scrolls sideways: a swipe moves it, and the
/// arrows beside its title page through it a screenful at a time.
struct CoverShelf<Item: Identifiable, Card: View>: View {
    let title: String
    let items: [Item]
    /// Names the shelf in its memory and its accessibility identifiers.
    let id: String
    let memory: ShelfScrollMemory
    @ViewBuilder let card: (Item) -> Card
    @State private var position: ScrollPosition
    @State private var edges = Edges()
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    static var cardWidth: CGFloat { 176 }
    private static var spacing: CGFloat { 16 }

    /// Which way the shelf can still scroll.
    private struct Edges: Equatable {
        var back = false
        var forward = false
    }

    init(title: String, items: [Item], id: String, memory: ShelfScrollMemory,
         @ViewBuilder card: @escaping (Item) -> Card) {
        self.title = title
        self.items = items
        self.id = id
        self.memory = memory
        self.card = card
        let offset = memory.shelves[id]?.offset ?? 0
        _position = State(initialValue: offset > 0 ? ScrollPosition(x: offset) : ScrollPosition())
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                Text(title)
                    .font(.system(size: 22, weight: .bold))
                    .accessibilityAddTraits(.isHeader)
                Spacer(minLength: 16)
                if edges.back || edges.forward {
                    StorefrontGlassButton(systemName: "chevron.left", size: 12, label: L10n.string("ui.home.shelf_previous"),
                                          identifier: "home-shelf-\(id)-previous") { page(by: -1) }
                        .disabled(!edges.back)
                    StorefrontGlassButton(systemName: "chevron.right", size: 12, label: L10n.string("ui.home.shelf_next"),
                                          identifier: "home-shelf-\(id)-next") { page(by: 1) }
                        .disabled(!edges.forward)
                }
            }
            .frame(height: 36)
            .padding(.horizontal, ObeliskStyle.contentPadding)

            ScrollView(.horizontal) {
                // Eager: a lazy stack measures its cards late and moves a
                // restored position as it does. A shelf holds a few dozen.
                HStack(alignment: .top, spacing: Self.spacing) {
                    ForEach(items) { item in
                        card(item).frame(width: Self.cardWidth)
                    }
                }
                .scrollTargetLayout()
                // Room for a card's lift and shadow under the pointer.
                .padding(.top, 10)
                .padding(.bottom, 26)
            }
            .contentMargins(.horizontal, ObeliskStyle.contentPadding, for: .scrollContent)
            .scrollIndicators(.never)
            .scrollTargetBehavior(.viewAligned)
            .scrollPosition($position)
            // Offsets count from the first card at the margin. The shelf
            // reaches under the sidebar, so its leading inset holds the
            // sidebar's width besides the margin; the container leaves both out.
            .onScrollGeometryChange(for: ShelfScrollMemory.Shelf.self, of: { geometry in
                ShelfScrollMemory.Shelf(
                    offset: geometry.contentOffset.x + geometry.contentInsets.leading,
                    maxOffset: max(0, geometry.contentSize.width - geometry.containerSize.width),
                    width: geometry.containerSize.width
                )
            }) { _, shelf in
                memory.shelves[id] = shelf
                let edges = Edges(back: shelf.offset > 1, forward: shelf.offset < shelf.maxOffset - 1)
                if edges != self.edges { self.edges = edges }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("home-shelf-\(id)")
    }

    /// Scrolls by as many whole cards as the shelf shows, onto a card's edge.
    private func page(by direction: CGFloat) {
        guard let shelf = memory.shelves[id] else { return }
        let stride = Self.cardWidth + Self.spacing
        let visible = max(1, ((shelf.width + Self.spacing) / stride).rounded(.down))
        let target = (((shelf.offset / stride).rounded() + direction * visible) * stride)
        withAnimation(reduceMotion ? nil : .smooth(duration: 0.45)) {
            position.scrollTo(x: min(max(target, 0), shelf.maxOffset))
        }
    }
}

extension ShelfScrollMemory.Shelf: Equatable {}
