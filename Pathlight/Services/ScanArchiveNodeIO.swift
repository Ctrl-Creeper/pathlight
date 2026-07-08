//
//  ScanArchiveNodeIO.swift
//  Pathlight
//

import CryptoKit
import Foundation

private enum ScanArchiveNodeIOConstants {
    static let readChunkSize = 1024 * 1024
    static let maxNodeLineByteCount = 1024 * 1024
    static let newlineData = Data([0x0A])
}

nonisolated struct ScanArchiveNodePayload: Sendable {
    let nodesByID: [String: FileNodeRecord]
    let orderedNodeIDs: [String]
}

extension ScanArchiveService {
    func writeNodes(
        _ treeStore: FileTreeStore,
        to url: URL,
        progressReporter: ScanArchiveProgressReporter?
    ) async throws -> String {
        guard FileManager.default.createFile(atPath: url.path, contents: nil) else {
            throw ScanArchiveError.nodes("could not create nodes section")
        }

        let fileHandle = try FileHandle(forWritingTo: url)
        defer { try? fileHandle.close() }

        var hasher = SHA256()
        let encoder = Self.makeJSONLineEncoder()
        let totalNodeCount = treeStore.nodeCount
        var processedNodeCount = 0

        for nodeID in treeStore.indexedNodeIDs() {
            try Task.checkCancellation()
            guard let node = treeStore.node(id: nodeID) else {
                throw ScanArchiveError.nodes("node \(nodeID) disappeared while exporting")
            }
            var lineData = try encoder.encode(ScanArchiveNode(node))
            lineData.append(ScanArchiveNodeIOConstants.newlineData)
            hasher.update(data: lineData)
            try fileHandle.write(contentsOf: lineData)
            processedNodeCount += 1

            if ScanArchiveProgressReporting.shouldReportProgress(processedNodeCount) || processedNodeCount == totalNodeCount {
                progressReporter?.report(ScanArchiveProgress(
                    phase: .writingNodes,
                    completedUnitCount: processedNodeCount,
                    totalUnitCount: totalNodeCount,
                    message: "Writing node records"
                ))
                await Task.yield()
            }
        }

        return Data(hasher.finalize()).base64EncodedString()
    }

    func readNodes(
        from url: URL,
        expectedChecksum: String,
        expectedNodeCount: Int,
        progressReporter: ScanArchiveProgressReporter?
    ) async throws -> ScanArchiveNodePayload {
        let fileHandle: FileHandle
        do {
            fileHandle = try FileHandle(forReadingFrom: url)
        } catch {
            throw ScanArchiveError.nodes(error.localizedDescription)
        }
        defer { try? fileHandle.close() }

        let decoder = Self.makeJSONDecoder()
        var nodesByID: [String: FileNodeRecord] = [:]
        var orderedNodeIDs: [String] = []
        orderedNodeIDs.reserveCapacity(expectedNodeCount)
        var buffer = Data()
        var hasher = SHA256()
        var decodedNodeCount = 0

        while true {
            try Task.checkCancellation()
            let chunk: Data
            do {
                chunk = try fileHandle.read(upToCount: ScanArchiveNodeIOConstants.readChunkSize) ?? Data()
            } catch let error as ScanArchiveError {
                throw error
            } catch {
                throw ScanArchiveError.nodes(error.localizedDescription)
            }
            guard !chunk.isEmpty else { break }
            hasher.update(data: chunk)
            buffer.append(chunk)

            while let newlineRange = buffer.firstRange(of: ScanArchiveNodeIOConstants.newlineData) {
                let lineData = Data(buffer[..<newlineRange.lowerBound])
                buffer.removeSubrange(..<newlineRange.upperBound)
                try validateNodeLineSize(lineData)
                if try decodeNodeLine(
                    lineData,
                    decoder: decoder,
                    nodesByID: &nodesByID,
                    orderedNodeIDs: &orderedNodeIDs
                ) {
                    decodedNodeCount += 1
                    try validateDecodedNodeCount(decodedNodeCount, expectedNodeCount: expectedNodeCount)
                    if ScanArchiveProgressReporting.shouldReportProgress(decodedNodeCount) ||
                        decodedNodeCount == expectedNodeCount {
                        progressReporter?.report(ScanArchiveProgress(
                            phase: .readingNodes,
                            completedUnitCount: decodedNodeCount,
                            totalUnitCount: expectedNodeCount,
                            message: "Reading node records"
                        ))
                        await Task.yield()
                    }
                }
            }
            try validateNodeLineSize(buffer)
        }

        if !buffer.isEmpty {
            try validateNodeLineSize(buffer)
            if try decodeNodeLine(
                buffer,
                decoder: decoder,
                nodesByID: &nodesByID,
                orderedNodeIDs: &orderedNodeIDs
            ) {
                decodedNodeCount += 1
                try validateDecodedNodeCount(decodedNodeCount, expectedNodeCount: expectedNodeCount)
            }
        }

        progressReporter?.report(ScanArchiveProgress(
            phase: .readingNodes,
            completedUnitCount: decodedNodeCount,
            totalUnitCount: expectedNodeCount,
            message: "Reading node records"
        ))

        let actualChecksum = Data(hasher.finalize()).base64EncodedString()
        guard actualChecksum == expectedChecksum else {
            throw ScanArchiveError.integrity("nodes checksum mismatch")
        }

        return ScanArchiveNodePayload(nodesByID: nodesByID, orderedNodeIDs: orderedNodeIDs)
    }

    private func validateNodeLineSize(_ lineData: Data) throws {
        guard lineData.count <= ScanArchiveNodeIOConstants.maxNodeLineByteCount else {
            throw ScanArchiveError.nodes("node record is too large")
        }
    }

    private func validateDecodedNodeCount(_ decodedNodeCount: Int, expectedNodeCount: Int) throws {
        guard decodedNodeCount <= expectedNodeCount else {
            throw ScanArchiveError.nodes("node payload contains more nodes than manifest expected")
        }
    }

    private func decodeNodeLine(
        _ lineData: Data,
        decoder: JSONDecoder,
        nodesByID: inout [String: FileNodeRecord],
        orderedNodeIDs: inout [String]
    ) throws -> Bool {
        guard !lineData.isEmpty else { return false }
        let node: FileNodeRecord
        do {
            node = try decoder.decode(ScanArchiveNode.self, from: lineData).modelNode()
        } catch let error as ScanArchiveError {
            throw error
        } catch {
            throw ScanArchiveError.nodes("invalid JSONL node: \(error.localizedDescription)")
        }
        guard nodesByID[node.id] == nil else {
            throw ScanArchiveError.nodes("duplicate node ID \(node.id)")
        }
        nodesByID[node.id] = node
        orderedNodeIDs.append(node.id)
        return true
    }
}
