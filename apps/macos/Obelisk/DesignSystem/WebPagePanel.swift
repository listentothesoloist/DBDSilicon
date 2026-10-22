import SwiftUI
import WebKit

/// Where a web page opens, signed in as the player when its storefront can:
/// cookies set before it loads, or an address that signs it in.
@MainActor
struct WebPageStart {
    var url: URL
    var cookies: [HTTPCookie] = []
    /// Nothing the page stores outlives it, unless its storefront keeps a
    /// sign-in there.
    var dataStore: WKWebsiteDataStore = .nonPersistent()
}

/// A web page under its toolbar: back, its title and host, reload, and open
/// in the browser. Back leaves the page once its history is empty, unless a
/// close button ends it.
struct WebPagePanel: View {
    let start: @MainActor () async -> WebPageStart
    let close: () -> Void
    var showsCloseButton = false
    /// What the toolbar names before the page tells its title.
    var placeholderTitle: String?
    var identifier = "web-page"
    /// Whether the panel draws its own glass; a sheet draws its own instead.
    var isPanel = true
    /// The storefront whose page it is, its tile before the title.
    var store: LibrarySection?
    @State private var browser: WebPageBrowser?

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                if !showsCloseButton || browser?.canGoBack == true {
                    StorefrontGlassButton(systemName: "chevron.left", label: L10n.string("ui.overlay.back"),
                                          identifier: "\(identifier)-back") {
                        if browser?.canGoBack == true { browser?.webView.goBack() } else { close() }
                    }
                }
                if let store { StoreTile(section: store, size: 36) }
                VStack(alignment: .leading, spacing: 2) {
                    Text(browser?.title ?? placeholderTitle ?? "")
                        .font(.system(size: 13, weight: .semibold))
                    Text(browser?.url?.host() ?? "")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }
                .lineLimit(1)
                Spacer(minLength: 12)
                if browser?.isLoading != false { ProgressView().controlSize(.small) }
                StorefrontGlassButton(systemName: "arrow.clockwise", size: 13,
                                      label: L10n.string("ui.overlay.web.reload"), identifier: "\(identifier)-reload") {
                    browser?.webView.reload()
                }
                StorefrontGlassButton(systemName: "safari", label: L10n.string("ui.overlay.web.openInBrowser"),
                                      identifier: "\(identifier)-open-in-browser") {
                    // The page's own address: the one that signed it in spent its code.
                    if let url = browser?.url { NSWorkspace.shared.open(url) }
                }
                .disabled(browser?.url == nil)
                if showsCloseButton {
                    StorefrontGlassButton(systemName: "xmark", size: 13, label: L10n.string("ui.common.close"),
                                          identifier: "\(identifier)-close", action: close)
                        .keyboardShortcut(.cancelAction)
                }
            }
            .padding(.horizontal, 14)
            .frame(height: 60)
            Group {
                if let browser { WebPageView(webView: browser.webView) } else { Color.clear }
            }
            .clipShape(UnevenRoundedRectangle(bottomLeadingRadius: isPanel ? 24 : 0,
                                              bottomTrailingRadius: isPanel ? 24 : 0))
        }
        .modifier(PanelGlass(isPanel: isPanel))
        .task {
            let page = await start()
            // Closed while it signed in: a code it spent would load nowhere.
            guard !Task.isCancelled else { return }
            browser = WebPageBrowser(page)
        }
        .accessibilityIdentifier(identifier)
    }
}

private struct PanelGlass: ViewModifier {
    let isPanel: Bool

    func body(content: Content) -> some View {
        if isPanel { content.modifier(StorefrontPanelGlass(cornerRadius: 24)) } else { content }
    }
}

/// The page's web view and what its toolbar shows of it.
@MainActor @Observable
final class WebPageBrowser: NSObject, WKNavigationDelegate, WKUIDelegate, NSWindowDelegate {
    @ObservationIgnored let webView: WKWebView
    /// Windows a script of the page opened, such as a sign-in through another
    /// site's account: they keep their opener, which reads their answer.
    @ObservationIgnored private var popups: [NSWindow] = []
    private(set) var title: String?
    private(set) var url: URL?
    private(set) var canGoBack = false
    private(set) var isLoading = true

    init(_ start: WebPageStart) {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = start.dataStore
        webView = WKWebView(frame: .zero, configuration: configuration)
        super.init()
        webView.navigationDelegate = self
        webView.uiDelegate = self
        let store = configuration.websiteDataStore.httpCookieStore
        Task { [webView] in
            for cookie in start.cookies { await store.setCookie(cookie) }
            webView.load(URLRequest(url: start.url))
        }
    }

    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
        isLoading = true
    }

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) { update() }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        update()
        isLoading = false
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        isLoading = false
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        isLoading = false
    }

    /// A link that opens a new window opens in the page instead; a script's
    /// window opens beside it, above the page's own window.
    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                 for navigationAction: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        guard navigationAction.targetFrame == nil else { return nil }
        guard navigationAction.navigationType != .linkActivated, popups.count < 4 else {
            webView.load(navigationAction.request)
            return nil
        }
        let size = NSSize(width: windowFeatures.width?.doubleValue ?? 540,
                          height: windowFeatures.height?.doubleValue ?? 720)
        let popup = NSWindow(contentRect: NSRect(origin: .zero, size: size),
                             styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        let content = WKWebView(frame: NSRect(origin: .zero, size: size), configuration: configuration)
        content.uiDelegate = self
        popup.contentView = content
        popup.isReleasedWhenClosed = false
        popup.delegate = self
        if let opener = webView.window {
            popup.level = opener.level
            popup.collectionBehavior = opener.collectionBehavior
        }
        popup.center()
        popups.append(popup)
        popup.makeKeyAndOrderFront(nil)
        return content
    }

    func webViewDidClose(_ webView: WKWebView) {
        popups.first { $0.contentView === webView }?.close()
    }

    func windowWillClose(_ notification: Notification) {
        guard let closed = notification.object as? NSWindow else { return }
        (closed.contentView as? WKWebView)?.stopLoading()
        popups.removeAll { $0 === closed }
        webView.window?.makeKeyAndOrderFront(nil)
    }

    isolated deinit {
        for popup in popups {
            popup.delegate = nil
            popup.close()
        }
    }

    private func update() {
        title = webView.title.flatMap { $0.isEmpty ? nil : $0 }
        url = webView.url
        canGoBack = webView.canGoBack
    }
}

private struct WebPageView: NSViewRepresentable {
    let webView: WKWebView

    func makeNSView(context: Context) -> WKWebView { webView }
    func updateNSView(_ nsView: WKWebView, context: Context) {}
}
