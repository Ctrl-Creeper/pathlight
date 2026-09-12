import Testing
@testable import PathlightCore

@Suite("Monitoring start configuration")
struct MonitoringStartConfigurationTests {
    @Test("new monitoring starts at one kilobyte and five seconds")
    func defaults() {
        let configuration = MonitoringStartConfiguration.default

        #expect(configuration.minimumRecordedByteDelta == 1_024)
        #expect(configuration.monitorLatency == 5)
        #expect(configuration.liveOptions.minimumRecordedByteDelta == 1_024)
    }

    @Test("applies startup choices without losing preset rules")
    func appliesToPreset() {
        let configuration = MonitoringStartConfiguration(
            minimumRecordedByteDelta: 8 * 1_024,
            monitorLatency: 12
        )
        let preset = LongTermWatchTargetOptions(
            minimumRecordedByteDelta: 1,
            aggregationWindow: 120,
            recordsFileNames: false,
            monitorLatency: 30,
            exclusionPatterns: ["Logs/"]
        )

        let options = configuration.longTermOptions(basedOn: preset)

        #expect(options.minimumRecordedByteDelta == 8 * 1_024)
        #expect(options.monitorLatency == 12)
        #expect(options.aggregationWindow == 120)
        #expect(options.recordsFileNames == false)
        #expect(options.exclusionPatterns == ["Logs/"])
    }

    @Test("invalid startup values are clamped to watcher limits")
    func clampsInvalidValues() {
        let configuration = MonitoringStartConfiguration(
            minimumRecordedByteDelta: -1,
            monitorLatency: 0
        )

        #expect(configuration.minimumRecordedByteDelta == 0)
        #expect(configuration.monitorLatency == 0.25)
    }

    @Test("kilobyte input cannot overflow the byte threshold")
    func clampsKilobyteInputBeforeConversion() {
        let configuration = MonitoringStartConfiguration(
            minimumKilobytes: .max,
            monitorLatency: 5
        )

        #expect(configuration.minimumRecordedByteDelta == 1_024_000_000)
    }
}
