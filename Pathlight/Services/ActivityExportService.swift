import Foundation

/// Plain-text exports: CSV for spreadsheets, Markdown for a session report
/// (the "what did this installer touch" answer).
enum ActivityExportService {
    nonisolated static func csv(_ events: [DiskActivityEvent]) -> String {
        var lines = ["timestamp,kind,path,previousPath,byteDelta,confidence,affectedItemCount,process"]
        let formatter = ISO8601DateFormatter()
        for event in events.sorted(by: { $0.timestamp < $1.timestamp }) {
            lines.append([
                formatter.string(from: event.timestamp),
                event.kindName,
                event.path.path,
                event.previousPath?.path ?? "",
                event.byteDelta.map(String.init) ?? "",
                event.confidenceName,
                String(event.affectedItemCount),
                event.processName ?? ""
            ].map(csvField).joined(separator: ","))
        }
        return lines.joined(separator: "\n") + "\n"
    }

    nonisolated static func sessionReport(
        _ session: WatchSessionModel,
        endedAt: Date = Date(),
        eventLimit: Int = 500
    ) -> String {
        let summary = session.summary(endedAt: endedAt)
        let rootName = session.rootPath.lastPathComponent.isEmpty ? session.rootPath.path : session.rootPath.lastPathComponent
        var lines: [String] = []
        lines.append("# Pathlight session: \(rootName)")
        lines.append("")
        lines.append("- Folder: `\(session.rootPath.path)`")
        lines.append("- Started: \(PathlightFormatters.date(session.startedAt))")
        lines.append("- Ended: \(PathlightFormatters.date(endedAt))")
        lines.append("- Events: \(summary.eventCount.formatted()) (\(summary.unknownSizeEventCount.formatted()) with unknown size)")
        lines.append("- Net change: \(signedSize(summary.netByteDelta))")
        if session.droppedEventCount > 0 {
            lines.append("- Dropped: \(session.droppedEventCount.formatted()) older events past the retention cap")
        }

        let rootPrefix = session.rootPath.standardizedFileURL.path + "/"
        var byChild: [String: (bytes: Int64, count: Int)] = [:]
        var processes: [String: Int] = [:]
        for event in session.events {
            let path = event.path.standardizedFileURL.path
            let child = path.hasPrefix(rootPrefix)
                ? String(path.dropFirst(rootPrefix.count).split(separator: "/").first ?? "")
                : "(root)"
            byChild[child, default: (0, 0)].bytes += event.byteDelta ?? 0
            byChild[child, default: (0, 0)].count += event.affectedItemCount
            if let process = event.processName {
                processes[process, default: 0] += 1
            }
        }

        lines.append("")
        lines.append("## By top-level item")
        lines.append("")
        lines.append("| Item | Net change | Items |")
        lines.append("|------|-----------:|------:|")
        for (child, totals) in byChild.sorted(by: { abs($0.value.bytes) > abs($1.value.bytes) }) {
            lines.append("| \(child.isEmpty ? "(root)" : child) | \(signedSize(totals.bytes)) | \(totals.count.formatted()) |")
        }

        if !processes.isEmpty {
            lines.append("")
            lines.append("## Processes seen")
            lines.append("")
            for (process, count) in processes.sorted(by: { $0.value > $1.value }) {
                lines.append("- \(process): \(count.formatted()) events")
            }
        }

        lines.append("")
        lines.append("## Events")
        lines.append("")
        for event in session.events.suffix(eventLimit) {
            var text = "- \(PathlightFormatters.date(event.timestamp)) \(event.kindName) `\(event.path.path)`"
            if let previous = event.previousPath {
                text += " (from `\(previous.path)`)"
            }
            text += " \(event.byteDelta.map(signedSize) ?? "unknown size")"
            if let process = event.processName {
                text += " · \(process)"
            }
            lines.append(text)
        }
        if session.events.count > eventLimit {
            lines.append("- … \(session.events.count - eventLimit) earlier events omitted")
        }
        return lines.joined(separator: "\n") + "\n"
    }

    nonisolated static func csvField(_ value: String) -> String {
        guard value.contains(where: { $0 == "," || $0 == "\"" || $0 == "\n" }) else {
            return value
        }
        return "\"" + value.replacingOccurrences(of: "\"", with: "\"\"") + "\""
    }

    nonisolated static func signedSize(_ bytes: Int64) -> String {
        if bytes > 0 { return "+\(PathlightFormatters.size(bytes))" }
        if bytes < 0 { return "-\(PathlightFormatters.size(abs(bytes)))" }
        return PathlightFormatters.size(0)
    }
}

extension DiskActivityEvent {
    nonisolated var kindName: String {
        switch kind {
        case .created: return "created"
        case .modified: return "modified"
        case .deleted: return "deleted"
        case .moved: return "moved"
        case .aggregate: return "aggregate"
        }
    }

    nonisolated var confidenceName: String {
        switch confidence {
        case .confirmed: return "confirmed"
        case .estimated: return "estimated"
        case .unknown: return "unknown"
        }
    }
}
