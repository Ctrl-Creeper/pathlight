import XCTest
@testable import PathlightCore

final class FileSizeFormatterTests: XCTestCase {
    func testSmallSizesUseByteUnits() {
        XCTAssertTrue(PathlightFormatters.size(0).localizedCaseInsensitiveContains("byte"))
        XCTAssertEqual(PathlightFormatters.size(1), "1 byte")
        XCTAssertEqual(PathlightFormatters.size(512), "512 bytes")
        XCTAssertEqual(PathlightFormatters.size(1_024), "1 KB")
    }

    func testPercentageReturnsNilForNonPositiveTotal() {
        XCTAssertNil(PathlightFormatters.percentage(part: 1, total: 0))
        XCTAssertNil(PathlightFormatters.percentage(part: 1, total: -10))
    }

    func testPercentageFormatsRatioWithOneFractionDigit() {
        XCTAssertEqual(PathlightFormatters.percentage(part: 0, total: 10), "0.0%")
        XCTAssertEqual(PathlightFormatters.percentage(part: 1, total: 4), "25.0%")
        XCTAssertEqual(PathlightFormatters.percentage(part: 1, total: 3), "33.3%")
        XCTAssertEqual(PathlightFormatters.percentage(part: 1, total: 1), "100.0%")
    }

    func testPercentageDoesNotClampAboveOneHundredPercent() {
        // A child can exceed its container (e.g. hard-link dedup or
        // allocated-vs-logical accounting); the formatter reports the raw ratio.
        XCTAssertEqual(PathlightFormatters.percentage(part: 3, total: 2), "150.0%")
    }
}
