import Foundation
import Testing
@testable import PathlightCore

@Suite("Activity anomaly detector")
struct ActivityAnomalyDetectorTests {
    private let root = URL(filePath: "/Users/example/Documents", directoryHint: .isDirectory)
    private let now = Date(timeIntervalSince1970: 100_000)

    @Test("flags a mass deletion by item count or bytes inside the window")
    func flagsMassDeletion() {
        let manySmall = (0..<120).map { event(kind: .deleted, delta: -10, secondsAgo: 60 + $0) }
        let kinds = ActivityAnomalyDetector.anomalies(in: history(manySmall), now: now).map(\.kind)
        #expect(kinds == [.massDeletion])

        let oneHuge = [event(kind: .deleted, delta: -2_000_000_000, secondsAgo: 30)]
        #expect(ActivityAnomalyDetector.anomalies(in: history(oneHuge), now: now).first?.kind == .massDeletion)

        let old = (0..<120).map { event(kind: .deleted, delta: -10, secondsAgo: 3_600 + $0) }
        #expect(ActivityAnomalyDetector.anomalies(in: history(old), now: now).isEmpty)
    }

    @Test("flags a write burst and describes it")
    func flagsBurst() {
        let burst = [event(kind: .created, delta: 6_000_000_000, secondsAgo: 10)]
        let anomalies = ActivityAnomalyDetector.anomalies(in: history(burst), now: now)
        #expect(anomalies.map(\.kind) == [.burst])
        #expect(anomalies.first?.title == "Documents is filling up fast")
        #expect(anomalies.first?.identifier == "pathlight-anomaly-burst-/Users/example/Documents")
        #expect(ActivityAnomalyDetector.anomalies(in: history([event(kind: .created, delta: 100, secondsAgo: 10)]), now: now).isEmpty)
    }

    private func event(kind: DiskActivityEventKind, delta: Int64, secondsAgo: Int) -> DiskActivityEvent {
        DiskActivityEvent(
            kind: kind,
            path: root.appending(path: "file\(secondsAgo)"),
            rootPath: root,
            timestamp: now.addingTimeInterval(-TimeInterval(secondsAgo)),
            byteDelta: delta,
            confidence: .confirmed,
            previousPath: nil,
            affectedItemCount: 1
        )
    }

    private func history(_ events: [DiskActivityEvent]) -> ActivityHistorySnapshot {
        ActivityHistorySnapshot(
            rootPath: root,
            generatedAt: now,
            totalNetByteDelta: events.compactMap(\.byteDelta).reduce(0, +),
            eventCount: events.count,
            unknownSizeEventCount: 0,
            buckets: [],
            recentEvents: events
        )
    }
}
