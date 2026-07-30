import Foundation
import Testing
@testable import PathlightCore

@Suite("Activity storage line codec")
struct ActivityStorageLineCodecTests {
    @Test("encrypts new lines and decodes them")
    func encryptsNewLinesAndDecodesThem() throws {
        let codec = makeEncryptedActivityStorageLineCodec()
        let plaintext = Data(#"{"path":"/Users/example/Downloads/private.zip"}"#.utf8)

        let encoded = try codec.encode(plaintext)
        let decoded = try codec.decode(encoded)

        #expect(encoded.hasPrefix("pathlight:v1:aes-gcm:"))
        #expect(!encoded.contains("private.zip"))
        #expect(decoded == plaintext)
    }

    @Test("keeps plaintext lines readable for migration")
    func keepsPlaintextLinesReadableForMigration() throws {
        let codec = makeEncryptedActivityStorageLineCodec()
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

    @Test("round-trips encryption through the caching provider")
    func roundTripsEncryptionThroughCachingProvider() throws {
        let codec = ActivityStorageLineCodec(
            preferencesStore: FixedActivityStoragePreferencesStore(encryptNewData: true),
            cryptor: AESGCMActivityStorageCryptor(
                keyProvider: CachingActivityStorageKeyProvider(
                    wrapping: ScriptedActivityStorageKeyProvider(results: [.success(Data(repeating: 3, count: 32))])
                )
            )
        )
        let plaintext = Data(#"{"path":"/Users/example/Downloads/cached.zip"}"#.utf8)

        let decoded = try codec.decode(try codec.encode(plaintext))

        #expect(decoded == plaintext)
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
