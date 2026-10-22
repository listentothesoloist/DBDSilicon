import AppKit
import SwiftUI

/// A storefront page's scroll view. Its scrollbar floats over the content as
/// an overlay scroller, whatever the system setting, so artwork reaches the
/// window's edge and the content width never changes with the scrollbar. Its
/// content starts below the storefront's floating bar.
struct StorefrontScrollView<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        ScrollView {
            content
                .padding(.top, ObeliskStyle.titleBarHeight)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background {
                    StorefrontOverlayScroller()
                        .frame(width: 0, height: 0)
                }
        }
    }
}

private struct StorefrontOverlayScroller: NSViewRepresentable {
    func makeNSView(context: Context) -> ScrollerConfigurationView {
        ScrollerConfigurationView()
    }

    func updateNSView(_ nsView: ScrollerConfigurationView, context: Context) {
        nsView.configureScroller()
    }

    final class ScrollerConfigurationView: NSView {
        override func viewDidMoveToSuperview() {
            super.viewDidMoveToSuperview()
            configureScroller()
        }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            configureScroller()
        }

        override func layout() {
            super.layout()
            configureScroller()
        }

        func configureScroller() {
            guard let scrollView = enclosingScrollView else { return }
            if scrollView.scrollerStyle != .overlay { scrollView.scrollerStyle = .overlay }
            if !scrollView.hasVerticalScroller { scrollView.hasVerticalScroller = true }
            // The scrollbar starts below the floating bar.
            if scrollView.scrollerInsets.top != ObeliskStyle.titleBarHeight {
                scrollView.scrollerInsets.top = ObeliskStyle.titleBarHeight
            }
        }
    }
}
