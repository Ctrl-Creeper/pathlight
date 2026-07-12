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
}
