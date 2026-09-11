import Foundation
import Testing
@testable import PathlightCore

@Suite("Activity diary")
struct ActivityDiaryTests {
    /// The same promises `the_diary_keeps_the_newest_lines_and_stays_under_its_cap`
    /// makes of `Storage::note` in `core/src/store.rs`: newest last, capped,
    /// and every surviving line whole.
    @Test("keeps the newest lines and stays under its cap")
    func keepsTheNewestLinesAndStaysUnderItsCap() throws {
        let directory = URL.temporaryDirectory.appending(path: UUID().uuidString)
        let diary = ActivityDiary(fileURL: directory.appending(path: "pathlight.log"))
        defer { try? FileManager.default.removeItem(at: directory) }

        diary.note("watch opened on /tmp/one")
        diary.note("watch closed on /tmp/one")
        #expect(diary.tail(lines: 1).hasSuffix("watch closed on /tmp/one"))
        #expect(diary.tail(lines: 9).split(separator: "\n").count == 2)

        let padding = String(repeating: "x", count: 400)
        for index in 0..<900 {
            diary.note("\(padding) \(index)")
        }
        let size = try FileManager.default.attributesOfItem(atPath: diary.fileURL.path)[.size] as? Int
        #expect((size ?? 0) < ActivityDiary.maximumBytes)
        #expect(diary.tail(lines: 1).hasSuffix("899"))
        // A trim that cut mid-line would leave a first line with no stamp.
        for line in diary.tail(lines: ActivityDiary.maximumBytes).split(separator: "\n") {
            #expect(line.hasPrefix("20"))
        }
    }

    @Test("says nothing when nothing has been written")
    func saysNothingWhenNothingHasBeenWritten() {
        let diary = ActivityDiary(fileURL: URL.temporaryDirectory.appending(path: UUID().uuidString))
        #expect(diary.tail(lines: 10).isEmpty)
    }
}
