import AppKit
import XCTest

/// What a page's scrolling must keep while its content changes: the same
/// scroll view and scrollbar, the overlay scroller style, and the content width.
@MainActor
struct StableScroller {
    let scroll: NSScrollView
    let scroller: NSScroller
    let width: CGFloat

    /// The first scroll view in `host` as it is now, whose content keeps
    /// `width`, else its current width.
    init(in host: NSView, width: CGFloat? = nil) throws {
        scroll = try XCTUnwrap(firstScrollView(in: host))
        scroller = try XCTUnwrap(scroll.verticalScroller)
        self.width = width ?? scroll.contentView.bounds.width
    }

    func assertKept(in host: NSView, during phase: String, file: StaticString = #filePath, line: UInt = #line) throws {
        let current = try XCTUnwrap(firstScrollView(in: host), file: file, line: line)
        XCTAssertTrue(current === scroll, "The scroll view changed during \(phase)", file: file, line: line)
        XCTAssertTrue(current.verticalScroller === scroller, "The scrollbar changed during \(phase)", file: file, line: line)
        XCTAssertTrue(current.hasVerticalScroller, file: file, line: line)
        XCTAssertEqual(current.scrollerStyle, .overlay, "Unexpected scroller style during \(phase)", file: file, line: line)
        XCTAssertEqual(current.contentView.bounds.width, width, accuracy: 0.5,
                       "The content width changed during \(phase)", file: file, line: line)
    }
}

@MainActor
func firstScrollView(in view: NSView) -> NSScrollView? {
    if let scroll = view as? NSScrollView { return scroll }
    return view.subviews.lazy.compactMap { firstScrollView(in: $0) }.first
}
