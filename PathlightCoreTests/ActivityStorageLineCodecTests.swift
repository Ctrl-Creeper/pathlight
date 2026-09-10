import Foundation
import Testing
@testable import PathlightCore

@Suite("Activity storage line codec")
struct ActivityStorageLineCodecTests {
    @Test("sends new lines through the cryptor and decodes them back")
    func sendsNewLinesThroughTheCryptor() throws {
        let codec = makeSealedActivityStorageLineCodec()
        let plaintext = Data(#"{"path":"/Users/example/Downloads/private.zip"}"#.utf8)

        let encoded = try codec.encode(plaintext)
        let decoded = try codec.decode(encoded)

        #expect(encoded.hasPrefix("pathlight:v1:aes-gcm:"))
        #expect(!encoded.contains("private.zip"))
        #expect(decoded == plaintext)
    }

    @Test("keeps plaintext lines readable for migration")
    func keepsPlaintextLinesReadableForMigration() throws {
        let codec = makeSealedActivityStorageLineCodec()
        let plaintext = #"{"path":"/Users/example/Downloads/legacy.zip"}"#

        let decoded = try codec.decode(plaintext)

        #expect(decoded == Data(plaintext.utf8))
    }

    @Test("caches the key after the first successful load")
    func cachesKeyAfterFirstSuccessfulLoad() throws {
        let underlying = ScriptedActivityStorageKeyProvider(results: [.success(Data(repeating: 1, count: 32))])
        let caching = CachingActivityStorageKeyProvider(wrapping: underlying)

        let first = try caching.loadOrCreateKey()
        let second = try caching.loadOrCreateKey()

        #expect(first == Data(repeating: 1, count: 32))
        #expect(second == first)
        #expect(underlying.callCount == 1)
    }

    @Test("does not cache key load failures")
    func doesNotCacheKeyLoadFailures() throws {
        let underlying = ScriptedActivityStorageKeyProvider(results: [
            .failure(ActivityStorageLineCodecError.keychainReadFailed(-1)),
            .success(Data(repeating: 2, count: 32))
        ])
        let caching = CachingActivityStorageKeyProvider(wrapping: underlying)

        #expect(throws: (any Error).self) {
            try caching.loadOrCreateKey()
        }
        #expect(try caching.loadOrCreateKey() == Data(repeating: 2, count: 32))
        #expect(underlying.callCount == 2)
    }

    @Test("reports a stalled key load instead of blocking the caller")
    func reportsStalledKeyLoadInsteadOfBlocking() throws {
        let underlying = BlockingActivityStorageKeyProvider(key: Data(repeating: 4, count: 32))
        let caching = CachingActivityStorageKeyProvider(wrapping: underlying, timeout: .milliseconds(200))

        #expect(throws: ActivityStorageLineCodecError.self) {
            try caching.loadOrCreateKey()
        }
        // A second stalled call must not queue up another blocked load.
        #expect(throws: ActivityStorageLineCodecError.self) {
            try caching.loadOrCreateKey()
        }
        #expect(underlying.callCount == 1)

        underlying.unblock()
        #expect(try caching.loadOrCreateKey() == Data(repeating: 4, count: 32))
        #expect(underlying.callCount == 1)
    }

    @Test("reads a row it cannot decrypt as an error, not as json")
    func readsAnUnreadableRowAsAnError() throws {
        let codec = makeSealedActivityStorageLineCodec()

        #expect(throws: (any Error).self) {
            try codec.decode(SealedActivityStorageCryptor.marker + "!not base64!")
        }
    }
}

private final class ScriptedActivityStorageKeyProvider: ActivityStorageKeyProviding, @unchecked Sendable {
    private var results: [Result<Data, any Error>]
    private(set) var callCount = 0

    init(results: [Result<Data, any Error>]) {
        precondition(!results.isEmpty)
        self.results = results
    }

    func loadOrCreateKey() throws -> Data {
        callCount += 1
        let result = results.count > 1 ? results.removeFirst() : results[0]
        return try result.get()
    }
}

private final class BlockingActivityStorageKeyProvider: ActivityStorageKeyProviding, @unchecked Sendable {
    private let key: Data
    private let gate = DispatchSemaphore(value: 0)
    private let lock = NSLock()
    private var calls = 0

    init(key: Data) {
        self.key = key
    }

    var callCount: Int {
        lock.withLock { calls }
    }

    func unblock() {
        gate.signal()
    }

    func loadOrCreateKey() throws -> Data {
        lock.withLock { calls += 1 }
        gate.wait()
        return key
    }
}
